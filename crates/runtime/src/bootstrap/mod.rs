use std::sync::Arc;

use reili_adapters::inbound::slack::SlackSignatureVerifier;
use reili_adapters::logger::TracingLogger;
use reili_adapters::outbound::agents::{
    AnthropicTaskRunner, AnthropicTaskRunnerInput, BedrockTaskRunner, BedrockTaskRunnerInput,
    DatadogMcpToolConfig, OpenAiTaskRunner, OpenAiTaskRunnerInput, VertexAiGeminiClient,
    VertexAiTaskRunner, VertexAiTaskRunnerInput,
};
use reili_adapters::outbound::anthropic::{
    AnthropicWebSearchAdapter, AnthropicWebSearchAdapterConfig,
};
use reili_adapters::outbound::bedrock::{BedrockWebSearchAdapter, BedrockWebSearchAdapterConfig};
use reili_adapters::outbound::github::GitHubMcpConfig;
use reili_adapters::outbound::openai::{OpenAiWebSearchAdapter, OpenAiWebSearchAdapterConfig};
use reili_adapters::outbound::slack::{
    SlackMessageSearchAdapter, SlackProgressReporter, SlackProgressReporterInput,
    SlackReactionAdapter, SlackTaskControlMessageAdapter, SlackThreadHistoryAdapter,
    SlackThreadReplyAdapter, SlackWebApiClient, SlackWebApiClientConfig,
};
use reili_adapters::outbound::vertex_ai::{
    VertexAiWebSearchAdapter, VertexAiWebSearchAdapterConfig,
};
use reili_adapters::queue::InMemoryJobQueue;
use reili_application::task::{TaskExecutionDeps, TaskLogger};
use reili_application::{
    EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, HandleSlackInteractionUseCase,
    HandleSlackInteractionUseCaseDeps, StartTaskWorkerRunnerUseCase,
    StartTaskWorkerRunnerUseCaseDeps,
};
use reili_core::error::PortError;
use reili_core::knowledge::WebSearchPort;
use reili_core::messaging::slack::{
    SlackInteractionHandlerPort, SlackMessageHandlerPort, SlackTaskControlMessagePort,
};
use reili_core::messaging::slack::{
    SlackMessageSearchPort, SlackReactionPort, SlackThreadHistoryPort, SlackThreadReplyPort,
};
use reili_core::queue::TaskJobQueuePort;
use reili_core::task::TaskJob;
use reili_core::task::{TaskProgressSessionFactoryPort, TaskResources, TaskRunnerPort};
use serde_json::{Value, json};
use thiserror::Error;

use crate::config::env::{AppConfig, LlmProviderConfig, SlackConnectionMode};

pub struct RuntimeDeps {
    pub slack_signature_verifier: Option<Arc<SlackSignatureVerifier>>,
    pub bot_user_id: String,
    pub slack_message_handler: Arc<dyn SlackMessageHandlerPort>,
    pub slack_interaction_handler: Arc<dyn SlackInteractionHandlerPort>,
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
    github_mcp: GitHubMcpConfig,
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
    let slack_reaction_port: Arc<dyn SlackReactionPort> =
        Arc::new(SlackReactionAdapter::new(Arc::clone(&slack_web_api_client)));
    let slack_task_control_message_port: Arc<dyn SlackTaskControlMessagePort> = Arc::new(
        SlackTaskControlMessageAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let task_progress_session_factory_port: Arc<dyn TaskProgressSessionFactoryPort> =
        Arc::new(SlackProgressReporter::new(SlackProgressReporterInput {
            client: Arc::clone(&slack_web_api_client),
            logger: Arc::clone(&logger),
        }));
    let slack_thread_history_port: Arc<dyn SlackThreadHistoryPort> = Arc::new(
        SlackThreadHistoryAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let slack_message_search_port: Arc<dyn SlackMessageSearchPort> = Arc::new(
        SlackMessageSearchAdapter::new(Arc::clone(&slack_web_api_client)),
    );
    let job_queue: Arc<TaskJobQueuePort> = Arc::new(InMemoryJobQueue::<TaskJob>::new());
    let in_flight_job_registry = reili_application::task::services::InFlightJobRegistry::new();
    let provider_ports = create_provider_ports(CreateProviderPortsInput {
        llm_provider: &config.llm.provider,
        datadog_api_key: config.datadog_api_key.clone(),
        datadog_app_key: config.datadog_app_key.clone(),
        datadog_site: config.datadog_site.clone(),
        github_mcp: GitHubMcpConfig {
            url: config.github.url.clone(),
            app_id: config.github.app_id.clone(),
            private_key: config.github.private_key.expose().to_string(),
            installation_id: config.github.installation_id,
        },
        github_scope_org: config.github.scope_org.clone(),
        language: config.language.clone(),
    })
    .await?;

    let task_resources = TaskResources {
        slack_message_search_port,
        web_search_port: provider_ports.web_search_port,
    };
    let task_runner = provider_ports.task_runner;
    let slack_message_handler: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            slack_reaction_port,
            slack_task_control_message_port: Arc::clone(&slack_task_control_message_port),
            slack_reply_port: Arc::clone(&slack_reply_port),
            logger: Arc::clone(&logger),
        }),
    );
    let slack_interaction_handler: Arc<dyn SlackInteractionHandlerPort> = Arc::new(
        HandleSlackInteractionUseCase::new(HandleSlackInteractionUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            in_flight_job_registry: in_flight_job_registry.clone(),
            slack_task_control_message_port: Arc::clone(&slack_task_control_message_port),
            logger: Arc::clone(&logger),
        }),
    );
    let worker_runner = StartTaskWorkerRunnerUseCase::new(StartTaskWorkerRunnerUseCaseDeps {
        job_queue,
        in_flight_job_registry,
        slack_task_control_message_port,
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
    let slack_signature_verifier = build_slack_signature_verifier(
        &config.slack_connection_mode,
        config.slack_signing_secret.as_deref(),
    )?;

    Ok(RuntimeDeps {
        slack_signature_verifier,
        bot_user_id,
        slack_message_handler,
        slack_interaction_handler,
        worker_runner,
        logger,
    })
}

fn build_slack_signature_verifier(
    connection_mode: &SlackConnectionMode,
    slack_signing_secret: Option<&str>,
) -> Result<Option<Arc<SlackSignatureVerifier>>, PortError> {
    match connection_mode {
        SlackConnectionMode::Http => slack_signing_secret
            .filter(|value| !value.trim().is_empty())
            .map(|value| SlackSignatureVerifier::new(value.to_string()))
            .transpose()
            .map(|verifier| verifier.map(Arc::new)),
        SlackConnectionMode::SocketMode { .. } => Ok(None),
    }
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
                github_mcp: input.github_mcp,
                github_scope_org: input.github_scope_org,
                language: input.language,
            })),
        }),
        LlmProviderConfig::Anthropic(config) => Ok(ProviderPorts {
            web_search_port: Arc::new(AnthropicWebSearchAdapter::new(
                AnthropicWebSearchAdapterConfig {
                    api_key: config.api_key.clone(),
                    model: config.model.clone(),
                },
            )),
            task_runner: Arc::new(AnthropicTaskRunner::new(AnthropicTaskRunnerInput {
                api_key: config.api_key.clone(),
                model: config.model.clone(),
                datadog_mcp: DatadogMcpToolConfig {
                    api_key: input.datadog_api_key,
                    app_key: input.datadog_app_key,
                    site: input.datadog_site,
                },
                github_mcp: input.github_mcp,
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
                github_mcp: input.github_mcp,
                github_scope_org: input.github_scope_org,
                language: input.language,
            })),
        }),
        LlmProviderConfig::VertexAi(config) => {
            let client = build_vertex_ai_client(config)?;

            Ok(ProviderPorts {
                web_search_port: Arc::new(VertexAiWebSearchAdapter::new(
                    VertexAiWebSearchAdapterConfig {
                        client: client.clone(),
                        model_id: config.model_id.clone(),
                    },
                )),
                task_runner: Arc::new(VertexAiTaskRunner::new(VertexAiTaskRunnerInput {
                    client,
                    model_id: config.model_id.clone(),
                    datadog_mcp: DatadogMcpToolConfig {
                        api_key: input.datadog_api_key,
                        app_key: input.datadog_app_key,
                        site: input.datadog_site,
                    },
                    github_mcp: input.github_mcp,
                    github_scope_org: input.github_scope_org,
                    language: input.language,
                })),
            })
        }
    }
}

fn build_vertex_ai_client(
    config: &crate::config::env::VertexAiLlmConfig,
) -> Result<VertexAiGeminiClient, RuntimeBootstrapError> {
    VertexAiGeminiClient::builder()
        .with_project(&config.project_id)
        .with_location(&config.location)
        .build()
        .map_err(
            |error| RuntimeBootstrapError::ProviderClientInitialization {
                provider: "vertexai".to_string(),
                message: error,
            },
        )
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

#[cfg(test)]
mod tests {
    use super::build_slack_signature_verifier;
    use crate::config::env::{SecretString, SlackConnectionMode};

    #[test]
    fn socket_mode_does_not_build_signature_verifier_even_with_secret() {
        let verifier = build_slack_signature_verifier(
            &SlackConnectionMode::SocketMode {
                app_token: SecretString::new("xapp-test-token".to_string()),
            },
            Some("signing-secret"),
        )
        .expect("build verifier");

        assert!(verifier.is_none());
    }

    #[test]
    fn http_mode_builds_signature_verifier_with_non_empty_secret() {
        let verifier =
            build_slack_signature_verifier(&SlackConnectionMode::Http, Some("signing-secret"))
                .expect("build verifier");

        assert!(verifier.is_some());
    }

    #[test]
    fn http_mode_ignores_whitespace_only_secret() {
        let verifier = build_slack_signature_verifier(&SlackConnectionMode::Http, Some("   "))
            .expect("build verifier");

        assert!(verifier.is_none());
    }
}
