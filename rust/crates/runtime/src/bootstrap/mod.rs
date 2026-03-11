use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use sre_adapters::inbound::slack::SlackSignatureVerifier;
use sre_adapters::outbound::agents::{
    OpenAiInvestigationCoordinatorRunner, OpenAiInvestigationCoordinatorRunnerInput,
    OpenAiInvestigationSynthesizerRunner, OpenAiInvestigationSynthesizerRunnerInput,
};
use sre_adapters::outbound::datadog::DatadogEventSearchAdapter;
use sre_adapters::outbound::datadog::{
    DatadogHttpClient, DatadogHttpClientConfig, DatadogLogAggregateAdapter,
    DatadogLogSearchAdapter, DatadogMetricCatalogAdapter, DatadogMetricQueryAdapter,
};
use sre_adapters::outbound::github::{GitHubSearchAdapter, GitHubSearchAdapterConfig};
use sre_adapters::outbound::openai::{OpenAiWebSearchAdapter, OpenAiWebSearchAdapterConfig};
use sre_adapters::outbound::slack::{
    SlackProgressStreamAdapter, SlackThreadHistoryAdapter, SlackThreadReplyAdapter,
    SlackWebApiClient, SlackWebApiClientConfig,
};
use sre_adapters::outbound::worker::{HttpWorkerJobDispatcher, HttpWorkerJobDispatcherConfig};
use sre_application::EnqueueSlackEventUseCase;
use sre_application::EnqueueSlackEventUseCaseDeps;
use sre_application::investigation::InvestigationLogger;
use sre_shared::errors::PortError;
use sre_shared::ports::inbound::SlackMessageHandlerPort;
use sre_shared::ports::outbound::{
    DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
    DatadogMetricCatalogPort, DatadogMetricQueryPort, GithubSearchPort,
    InvestigationCoordinatorRunnerPort, InvestigationResources, InvestigationSynthesizerRunnerPort,
    SlackProgressStreamPort, SlackThreadHistoryPort, SlackThreadReplyPort, WebSearchPort,
};
use sre_shared::types::DatadogApiRetryConfig;
use thiserror::Error;

use crate::config::env::{IngressConfig, WorkerConfig};

const DATADOG_API_RETRY: DatadogApiRetryConfig = DatadogApiRetryConfig {
    enabled: true,
    max_retries: 3,
    backoff_base_seconds: 2,
    backoff_multiplier: 2,
};

pub struct IngressRuntimeDeps {
    pub slack_message_handler: Arc<dyn SlackMessageHandlerPort>,
    pub slack_signature_verifier: Arc<SlackSignatureVerifier>,
    pub bot_user_id: String,
    pub logger: Arc<dyn InvestigationLogger>,
}

pub struct WorkerRuntimeDeps {
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub slack_progress_stream_port: Arc<dyn SlackProgressStreamPort>,
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub investigation_resources: InvestigationResources,
    pub coordinator_runner: Arc<dyn InvestigationCoordinatorRunnerPort>,
    pub synthesizer_runner: Arc<dyn InvestigationSynthesizerRunnerPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

#[derive(Debug, Error)]
pub enum RuntimeBootstrapError {
    #[error("{0}")]
    Port(#[from] PortError),
    #[error("Slack auth.test response did not contain user_id")]
    MissingSlackBotUserId,
}

pub async fn build_ingress_runtime_deps(
    config: &IngressConfig,
) -> Result<IngressRuntimeDeps, RuntimeBootstrapError> {
    let logger = create_investigation_logger();
    let slack_web_api_client = Arc::new(SlackWebApiClient::new(SlackWebApiClientConfig {
        bot_token: config.slack_bot_token.clone(),
        base_url: None,
    })?);
    let bot_user_id = resolve_slack_bot_user_id(&slack_web_api_client).await?;
    let slack_reply_port: Arc<dyn SlackThreadReplyPort> = Arc::new(SlackThreadReplyAdapter::new(
        Arc::clone(&slack_web_api_client),
    ));
    let worker_dispatcher = Arc::new(HttpWorkerJobDispatcher::new(
        HttpWorkerJobDispatcherConfig {
            worker_base_url: config.worker_base_url.clone(),
            worker_internal_token: config.worker_internal_token.clone(),
            timeout_ms: config.worker_dispatch_timeout_ms,
        },
    )?);
    let slack_message_handler: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            worker_job_dispatcher: worker_dispatcher,
            slack_reply_port,
            logger: Arc::clone(&logger),
            job_max_retry: config.job_max_retry,
            job_backoff_ms: config.job_backoff_ms,
        }),
    );
    let slack_signature_verifier = Arc::new(SlackSignatureVerifier::new(
        config.slack_signing_secret.clone(),
    )?);

    Ok(IngressRuntimeDeps {
        slack_message_handler,
        slack_signature_verifier,
        bot_user_id,
        logger,
    })
}

pub fn build_worker_runtime_deps(
    config: &WorkerConfig,
) -> Result<WorkerRuntimeDeps, RuntimeBootstrapError> {
    let logger = create_investigation_logger();
    let slack_web_api_client = Arc::new(SlackWebApiClient::new(SlackWebApiClientConfig {
        bot_token: config.slack_bot_token.clone(),
        base_url: None,
    })?);
    let slack_reply_port: Arc<dyn SlackThreadReplyPort> = Arc::new(SlackThreadReplyAdapter::new(
        Arc::clone(&slack_web_api_client),
    ));
    let slack_progress_stream_port: Arc<dyn SlackProgressStreamPort> = Arc::new(
        SlackProgressStreamAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let slack_thread_history_port: Arc<dyn SlackThreadHistoryPort> = Arc::new(
        SlackThreadHistoryAdapter::new(Arc::clone(&slack_web_api_client)),
    );

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
    let github_search_port: Arc<dyn GithubSearchPort> =
        Arc::new(GitHubSearchAdapter::new(GitHubSearchAdapterConfig {
            app_id: config.github.app_id.clone(),
            private_key: config.github.private_key.clone(),
            installation_id: config.github.installation_id,
            scope_org: config.github.scope_org.clone(),
            base_url: None,
        })?);

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
        datadog_site: config.datadog_site.clone(),
        github_scope_org: config.github.scope_org.clone(),
        github_search_port,
        web_search_port,
    };
    let coordinator_runner: Arc<dyn InvestigationCoordinatorRunnerPort> = Arc::new(
        OpenAiInvestigationCoordinatorRunner::new(OpenAiInvestigationCoordinatorRunnerInput {
            openai_api_key: config.openai_api_key.clone(),
            language: config.language.clone(),
        }),
    );
    let synthesizer_runner: Arc<dyn InvestigationSynthesizerRunnerPort> = Arc::new(
        OpenAiInvestigationSynthesizerRunner::new(OpenAiInvestigationSynthesizerRunnerInput {
            openai_api_key: config.openai_api_key.clone(),
            language: config.language.clone(),
        }),
    );

    Ok(WorkerRuntimeDeps {
        slack_reply_port,
        slack_progress_stream_port,
        slack_thread_history_port,
        investigation_resources,
        coordinator_runner,
        synthesizer_runner,
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
    fn info(&self, message: &str, meta: BTreeMap<String, String>) {
        tracing::info!(message = message, meta = ?meta);
    }

    fn warn(&self, message: &str, meta: BTreeMap<String, String>) {
        tracing::warn!(message = message, meta = ?meta);
    }

    fn error(&self, message: &str, meta: BTreeMap<String, String>) {
        tracing::error!(message = message, meta = ?meta);
    }
}
