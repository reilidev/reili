use std::sync::Arc;

use reili_adapters::inbound::slack::SlackSignatureVerifier;
use reili_adapters::outbound::agents::{
    OpenAiInvestigationCoordinatorRunner, OpenAiInvestigationCoordinatorRunnerInput,
};
use reili_adapters::outbound::datadog::DatadogEventSearchAdapter;
use reili_adapters::outbound::datadog::{
    DatadogHttpClient, DatadogHttpClientConfig, DatadogLogAggregateAdapter,
    DatadogLogSearchAdapter, DatadogMetricCatalogAdapter, DatadogMetricQueryAdapter,
};
use reili_adapters::outbound::github::{GitHubSearchAdapter, GitHubSearchAdapterConfig};
use reili_adapters::outbound::openai::{OpenAiWebSearchAdapter, OpenAiWebSearchAdapterConfig};
use reili_adapters::outbound::slack::{
    SlackProgressStreamAdapter, SlackThreadHistoryAdapter, SlackThreadReplyAdapter,
    SlackWebApiClient, SlackWebApiClientConfig,
};
use reili_adapters::queue::InMemoryJobQueue;
use reili_application::investigation::{
    InvestigationExecutionDeps, InvestigationLogMeta, InvestigationLogger,
};
use reili_application::{
    EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, StartInvestigationWorkerRunnerUseCase,
    StartInvestigationWorkerRunnerUseCaseDeps,
};
use reili_shared::errors::PortError;
use reili_shared::ports::inbound::SlackMessageHandlerPort;
use reili_shared::ports::outbound::{
    DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
    DatadogMetricCatalogPort, DatadogMetricQueryPort, GithubCodeSearchPort, GithubPullRequestPort,
    GithubRepositoryContentPort, InvestigationCoordinatorRunnerPort, InvestigationJobQueuePort,
    InvestigationResources, SlackProgressStreamPort, SlackThreadHistoryPort, SlackThreadReplyPort,
    WebSearchPort,
};
use reili_shared::types::{DatadogApiRetryConfig, InvestigationJob};
use serde_json::{Value, json};
use thiserror::Error;

use crate::config::env::AppConfig;

const DATADOG_API_RETRY: DatadogApiRetryConfig = DatadogApiRetryConfig {
    enabled: true,
    max_retries: 3,
    backoff_base_seconds: 2,
    backoff_multiplier: 2,
};

pub struct RuntimeDeps {
    pub slack_signature_verifier: Arc<SlackSignatureVerifier>,
    pub bot_user_id: String,
    pub slack_message_handler: Arc<dyn SlackMessageHandlerPort>,
    pub worker_runner: StartInvestigationWorkerRunnerUseCase,
    pub logger: Arc<dyn InvestigationLogger>,
}

#[derive(Debug, Error)]
pub enum RuntimeBootstrapError {
    #[error("{0}")]
    Port(#[from] PortError),
    #[error("Slack auth.test response did not contain user_id")]
    MissingSlackBotUserId,
}

pub async fn build_runtime_deps(config: &AppConfig) -> Result<RuntimeDeps, RuntimeBootstrapError> {
    let logger = create_investigation_logger();
    let slack_web_api_client = Arc::new(SlackWebApiClient::new(SlackWebApiClientConfig {
        bot_token: config.slack_bot_token.clone(),
        base_url: None,
    })?);
    let bot_user_id = resolve_slack_bot_user_id(&slack_web_api_client).await?;
    let slack_reply_port: Arc<dyn SlackThreadReplyPort> = Arc::new(SlackThreadReplyAdapter::new(
        Arc::clone(&slack_web_api_client),
    ));
    let slack_progress_stream_port: Arc<dyn SlackProgressStreamPort> = Arc::new(
        SlackProgressStreamAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let slack_thread_history_port: Arc<dyn SlackThreadHistoryPort> = Arc::new(
        SlackThreadHistoryAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let job_queue: Arc<InvestigationJobQueuePort> =
        Arc::new(InMemoryJobQueue::<InvestigationJob>::new());

    let datadog_http_client = Arc::new(DatadogHttpClient::new(DatadogHttpClientConfig {
        api_key: config.datadog_api_key.clone(),
        app_key: config.datadog_app_key.clone(),
        site: config.datadog_site.clone(),
        retry: DATADOG_API_RETRY,
        max_response_bytes: 0,
        base_url: None,
    })?);
    let log_aggregate_port: Arc<dyn DatadogLogAggregatePort> = Arc::new(
        DatadogLogAggregateAdapter::new(Arc::clone(&datadog_http_client)),
    );
    let log_search_port: Arc<dyn DatadogLogSearchPort> = Arc::new(DatadogLogSearchAdapter::new(
        Arc::clone(&datadog_http_client),
    ));
    let metric_catalog_port: Arc<dyn DatadogMetricCatalogPort> = Arc::new(
        DatadogMetricCatalogAdapter::new(Arc::clone(&datadog_http_client)),
    );
    let metric_query_port: Arc<dyn DatadogMetricQueryPort> = Arc::new(
        DatadogMetricQueryAdapter::new(Arc::clone(&datadog_http_client)),
    );
    let event_search_port: Arc<dyn DatadogEventSearchPort> = Arc::new(
        DatadogEventSearchAdapter::new(Arc::clone(&datadog_http_client)),
    );
    let github_adapter = Arc::new(GitHubSearchAdapter::new(GitHubSearchAdapterConfig {
        app_id: config.github.app_id.clone(),
        private_key: config.github.private_key.clone(),
        installation_id: config.github.installation_id,
        scope_org: config.github.scope_org.clone(),
        base_url: None,
    })?);
    let github_code_search_port: Arc<dyn GithubCodeSearchPort> = github_adapter.clone();
    let github_repository_content_port: Arc<dyn GithubRepositoryContentPort> =
        github_adapter.clone();
    let github_pull_request_port: Arc<dyn GithubPullRequestPort> = github_adapter;
    let web_search_port: Arc<dyn WebSearchPort> =
        Arc::new(OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: config.openai_api_key.clone(),
            model: config.openai_web_search.model.clone(),
            timeout_ms: config.openai_web_search.timeout_ms,
        }));

    let investigation_resources = InvestigationResources {
        log_aggregate_port,
        log_search_port,
        metric_catalog_port,
        metric_query_port,
        event_search_port,
        github_code_search_port,
        github_repository_content_port,
        github_pull_request_port,
        web_search_port,
    };
    let coordinator_runner: Arc<dyn InvestigationCoordinatorRunnerPort> = Arc::new(
        OpenAiInvestigationCoordinatorRunner::new(OpenAiInvestigationCoordinatorRunnerInput {
            openai_api_key: config.openai_api_key.clone(),
            datadog_site: config.datadog_site.clone(),
            github_scope_org: config.github.scope_org.clone(),
            language: config.language.clone(),
        }),
    );
    let slack_message_handler: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            slack_reply_port: Arc::clone(&slack_reply_port),
            logger: Arc::clone(&logger),
        }),
    );
    let worker_runner =
        StartInvestigationWorkerRunnerUseCase::new(StartInvestigationWorkerRunnerUseCaseDeps {
            job_queue,
            investigation_execution_deps: InvestigationExecutionDeps {
                slack_reply_port,
                slack_progress_stream_port,
                slack_thread_history_port,
                investigation_resources,
                coordinator_runner,
                logger: Arc::clone(&logger),
            },
            worker_concurrency: config.worker_concurrency,
            job_max_retry: config.job_max_retry,
            job_backoff_ms: config.job_backoff_ms,
        });
    let slack_signature_verifier = Arc::new(SlackSignatureVerifier::new(
        config.slack_signing_secret.clone(),
    )?);

    Ok(RuntimeDeps {
        slack_signature_verifier,
        bot_user_id,
        slack_message_handler,
        worker_runner,
        logger,
    })
}

pub fn create_investigation_logger() -> Arc<dyn InvestigationLogger> {
    Arc::new(TracingInvestigationLogger)
}

async fn resolve_slack_bot_user_id(
    slack_web_api_client: &SlackWebApiClient,
) -> Result<String, RuntimeBootstrapError> {
    let response = slack_web_api_client.post("auth.test", &json!({})).await?;

    response
        .get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or(RuntimeBootstrapError::MissingSlackBotUserId)
}

#[derive(Debug, Default)]
struct TracingInvestigationLogger;

impl InvestigationLogger for TracingInvestigationLogger {
    fn info(&self, message: &str, meta: InvestigationLogMeta) {
        tracing::info!(
            message = message,
            meta = tracing::field::display(serde_json::Value::Object(meta)),
        );
    }

    fn warn(&self, message: &str, meta: InvestigationLogMeta) {
        tracing::warn!(
            message = message,
            meta = tracing::field::display(serde_json::Value::Object(meta)),
        );
    }

    fn error(&self, message: &str, meta: InvestigationLogMeta) {
        tracing::error!(
            message = message,
            meta = tracing::field::display(serde_json::Value::Object(meta)),
        );
    }
}
