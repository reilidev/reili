use std::sync::Arc;

use reili_adapters::inbound::slack::SlackSignatureVerifier;
use reili_adapters::logger::TracingLogger;
use reili_adapters::outbound::agents::{
    BedrockTaskRunner, BedrockTaskRunnerInput, DatadogMcpToolConfig, OpenAiTaskRunner,
    OpenAiTaskRunnerInput, VertexAiAnthropicClient, VertexAiAnthropicClientInput,
    VertexAiTaskRunner, VertexAiTaskRunnerInput,
};
use reili_adapters::outbound::bedrock::{BedrockWebSearchAdapter, BedrockWebSearchAdapterConfig};
use reili_adapters::outbound::datadog::DatadogEventSearchAdapter;
use reili_adapters::outbound::datadog::{
    DatadogApiRetryConfig, DatadogHttpClient, DatadogHttpClientConfig, DatadogLogAggregateAdapter,
    DatadogLogSearchAdapter, DatadogMetricCatalogAdapter, DatadogMetricQueryAdapter,
};
use reili_adapters::outbound::github::{GitHubSearchAdapter, GitHubSearchAdapterConfig};
use reili_adapters::outbound::openai::{OpenAiWebSearchAdapter, OpenAiWebSearchAdapterConfig};
use reili_adapters::outbound::slack::{
    SlackProgressReporter, SlackProgressReporterInput, SlackThreadHistoryAdapter,
    SlackThreadReplyAdapter, SlackWebApiClient, SlackWebApiClientConfig,
};
use reili_adapters::outbound::vertex_ai::{
    VertexAiWebSearchAdapter, VertexAiWebSearchAdapterConfig,
};
use reili_adapters::queue::InMemoryJobQueue;
use reili_application::task::{
    ScopedGithubCodeSearchPort, ScopedGithubPullRequestPort, ScopedGithubRepositoryContentPort,
    TaskExecutionDeps, TaskLogger,
};
use reili_application::{
    EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, StartTaskWorkerRunnerUseCase,
    StartTaskWorkerRunnerUseCaseDeps,
};
use reili_core::error::PortError;
use reili_core::knowledge::WebSearchPort;
use reili_core::messaging::slack::SlackMessageHandlerPort;
use reili_core::messaging::slack::{SlackThreadHistoryPort, SlackThreadReplyPort};
use reili_core::monitoring::datadog::{
    DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
    DatadogMetricCatalogPort, DatadogMetricQueryPort,
};
use reili_core::queue::TaskJobQueuePort;
use reili_core::source_code::github::{
    GithubCodeSearchPort, GithubPullRequestPort, GithubRepositoryContentPort, GithubScopePolicy,
};
use reili_core::task::TaskJob;
use reili_core::task::{TaskProgressSessionFactoryPort, TaskResources, TaskRunnerPort};
use serde_json::{Value, json};
use thiserror::Error;

use crate::config::env::{AppConfig, LlmProviderConfig};

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
    pub worker_runner: StartTaskWorkerRunnerUseCase,
    pub logger: Arc<dyn TaskLogger>,
}

#[derive(Debug, Error)]
pub enum RuntimeBootstrapError {
    #[error("{0}")]
    Port(#[from] PortError),
    #[error("Slack auth.test response did not contain user_id")]
    MissingSlackBotUserId,
    #[error("Failed to initialize {provider} client: {message}")]
    ProviderClientInitialization { provider: String, message: String },
}

struct ProviderPorts {
    web_search_port: Arc<dyn WebSearchPort>,
    task_runner: Arc<dyn TaskRunnerPort>,
}

struct CreateProviderPortsInput<'a> {
    llm_provider: &'a LlmProviderConfig,
    datadog_api_key: String,
    datadog_app_key: String,
    datadog_site: String,
    github_scope_org: String,
    language: String,
}

pub async fn build_runtime_deps(config: &AppConfig) -> Result<RuntimeDeps, RuntimeBootstrapError> {
    let logger = create_task_logger();
    let slack_web_api_client = Arc::new(SlackWebApiClient::new(SlackWebApiClientConfig {
        bot_token: config.slack_bot_token.clone(),
        base_url: None,
    })?);
    let bot_user_id = resolve_slack_bot_user_id(&slack_web_api_client).await?;
    let slack_reply_port: Arc<dyn SlackThreadReplyPort> = Arc::new(SlackThreadReplyAdapter::new(
        Arc::clone(&slack_web_api_client),
    ));
    let task_progress_session_factory_port: Arc<dyn TaskProgressSessionFactoryPort> =
        Arc::new(SlackProgressReporter::new(SlackProgressReporterInput {
            client: Arc::clone(&slack_web_api_client),
            logger: Arc::clone(&logger),
        }));
    let slack_thread_history_port: Arc<dyn SlackThreadHistoryPort> = Arc::new(
        SlackThreadHistoryAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let job_queue: Arc<TaskJobQueuePort> = Arc::new(InMemoryJobQueue::<TaskJob>::new());

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
    let github_scope_policy = Arc::new(GithubScopePolicy::new(config.github.scope_org.clone())?);
    let github_adapter = Arc::new(GitHubSearchAdapter::new(GitHubSearchAdapterConfig {
        app_id: config.github.app_id.clone(),
        private_key: config.github.private_key.clone(),
        installation_id: config.github.installation_id,
        base_url: None,
    })?);
    let github_code_search_port: Arc<dyn GithubCodeSearchPort> = Arc::new(
        ScopedGithubCodeSearchPort::new(github_adapter.clone(), Arc::clone(&github_scope_policy)),
    );
    let github_repository_content_port: Arc<dyn GithubRepositoryContentPort> =
        Arc::new(ScopedGithubRepositoryContentPort::new(
            github_adapter.clone(),
            Arc::clone(&github_scope_policy),
        ));
    let github_pull_request_port: Arc<dyn GithubPullRequestPort> = Arc::new(
        ScopedGithubPullRequestPort::new(github_adapter, github_scope_policy),
    );
    let provider_ports = create_provider_ports(CreateProviderPortsInput {
        llm_provider: &config.llm.provider,
        datadog_api_key: config.datadog_api_key.clone(),
        datadog_app_key: config.datadog_app_key.clone(),
        datadog_site: config.datadog_site.clone(),
        github_scope_org: config.github.scope_org.clone(),
        language: config.language.clone(),
    })
    .await?;

    let task_resources = TaskResources {
        log_aggregate_port,
        log_search_port,
        metric_catalog_port,
        metric_query_port,
        event_search_port,
        github_code_search_port,
        github_repository_content_port,
        github_pull_request_port,
        web_search_port: provider_ports.web_search_port,
    };
    let task_runner = provider_ports.task_runner;
    let slack_message_handler: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            slack_reply_port: Arc::clone(&slack_reply_port),
            logger: Arc::clone(&logger),
        }),
    );
    let worker_runner = StartTaskWorkerRunnerUseCase::new(StartTaskWorkerRunnerUseCaseDeps {
        job_queue,
        task_execution_deps: TaskExecutionDeps {
            slack_reply_port,
            task_progress_session_factory_port,
            slack_thread_history_port,
            task_resources,
            task_runner,
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

async fn create_provider_ports(
    input: CreateProviderPortsInput<'_>,
) -> Result<ProviderPorts, RuntimeBootstrapError> {
    match input.llm_provider {
        LlmProviderConfig::OpenAi(config) => Ok(ProviderPorts {
            web_search_port: Arc::new(OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
                api_key: config.api_key.clone(),
            })),
            task_runner: Arc::new(OpenAiTaskRunner::new(OpenAiTaskRunnerInput {
                api_key: config.api_key.clone(),
                task_runner_model: config.task_runner_model.clone(),
                datadog_mcp: DatadogMcpToolConfig {
                    api_key: input.datadog_api_key,
                    app_key: input.datadog_app_key,
                    site: input.datadog_site,
                },
                github_scope_org: input.github_scope_org,
                language: input.language,
            })),
        }),
        LlmProviderConfig::Bedrock(config) => Ok(ProviderPorts {
            web_search_port: Arc::new(BedrockWebSearchAdapter::new(
                BedrockWebSearchAdapterConfig {
                    model_id: config.model_id.clone(),
                },
            )),
            task_runner: Arc::new(BedrockTaskRunner::new(BedrockTaskRunnerInput {
                model_id: config.model_id.clone(),
                datadog_mcp: DatadogMcpToolConfig {
                    api_key: input.datadog_api_key,
                    app_key: input.datadog_app_key,
                    site: input.datadog_site,
                },
                github_scope_org: input.github_scope_org,
                language: input.language,
            })),
        }),
        LlmProviderConfig::VertexAi(config) => {
            let client = VertexAiAnthropicClient::new(VertexAiAnthropicClientInput {
                project_id: config.project_id.clone(),
                location: config.location.clone(),
            })
            .await
            .map_err(
                |error| RuntimeBootstrapError::ProviderClientInitialization {
                    provider: "vertexai".to_string(),
                    message: error.to_string(),
                },
            )?;

            Ok(ProviderPorts {
                web_search_port: Arc::new(
                    VertexAiWebSearchAdapter::new(VertexAiWebSearchAdapterConfig {
                        project_id: config.project_id.clone(),
                        location: config.location.clone(),
                        model_id: config.model_id.clone(),
                    })
                    .await
                    .map_err(|error| {
                        RuntimeBootstrapError::ProviderClientInitialization {
                            provider: "vertexai".to_string(),
                            message: error,
                        }
                    })?,
                ),
                task_runner: Arc::new(VertexAiTaskRunner::new(VertexAiTaskRunnerInput {
                    client,
                    model_id: config.model_id.clone(),
                    datadog_mcp: DatadogMcpToolConfig {
                        api_key: input.datadog_api_key,
                        app_key: input.datadog_app_key,
                        site: input.datadog_site,
                    },
                    github_scope_org: input.github_scope_org,
                    language: input.language,
                })),
            })
        }
    }
}

pub fn create_task_logger() -> Arc<dyn TaskLogger> {
    Arc::new(TracingLogger)
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
