use std::sync::Arc;

use reili_adapters::inbound::slack::SlackSignatureVerifier;
use reili_adapters::logger::TracingLogger;
use reili_adapters::outbound::agents::{
    AnthropicTaskRunner, AnthropicTaskRunnerInput, BedrockTaskRunner, BedrockTaskRunnerInput,
    ConnectorSet, DatadogConnector, DatadogMcpToolConfig, EsaConnector, GitHubConnector,
    OpenAiTaskRunner, OpenAiTaskRunnerInput, VertexAiGeminiClient, VertexAiTaskRunner,
    VertexAiTaskRunnerInput,
};
use reili_adapters::outbound::anthropic::{
    AnthropicWebSearchAdapter, AnthropicWebSearchAdapterConfig,
};
use reili_adapters::outbound::auto_response_judge::{
    CreateBedrockAutoResponseJudgePortInput, create_anthropic_auto_response_judge_port,
    create_bedrock_auto_response_judge_port, create_openai_auto_response_judge_port,
    create_vertex_ai_auto_response_judge_port,
};
use reili_adapters::outbound::bedrock::{BedrockWebSearchAdapter, BedrockWebSearchAdapterConfig};
use reili_adapters::outbound::esa::{EsaClient, EsaClientConfig, EsaPostSearchPort};
use reili_adapters::outbound::github::GitHubMcpConfig;
use reili_adapters::outbound::openai::{OpenAiWebSearchAdapter, OpenAiWebSearchAdapterConfig};
use reili_adapters::outbound::slack::{
    SlackChannelLookupAdapter, SlackEphemeralMessageAdapter, SlackMessageSearchAdapter,
    SlackProgressReporter, SlackProgressReporterInput, SlackReactionAdapter,
    SlackTaskControlMessageAdapter, SlackThreadHistoryAdapter, SlackThreadReplyAdapter,
    SlackUserGroupMembershipAdapter, SlackWebApiClient, SlackWebApiClientConfig,
};
use reili_adapters::outbound::vertex_ai::{
    VertexAiWebSearchAdapter, VertexAiWebSearchAdapterConfig,
};
use reili_adapters::queue::InMemoryJobQueue;
use reili_application::{
    EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, HandleSlackInteractionUseCase,
    HandleSlackInteractionUseCaseDeps, InFlightJobRegistry, SlackAutoResponseGate,
    SlackAutoResponseGateDeps, SlackAutoResponsePolicy, SlackInboundRouter,
    SlackMentionAuthorizationGate, SlackMentionAuthorizationService, StartTaskWorkerRunnerUseCase,
    StartTaskWorkerRunnerUseCaseDeps, TaskExecutionDeps, TaskLogger,
};
use reili_core::error::PortError;
use reili_core::knowledge::WebSearchPort;
use reili_core::messaging::slack::{
    AutoResponseJudgePort, SlackAuthorizationPolicy, SlackChannelLookupPort,
    SlackEphemeralMessagePort, SlackInteractionHandlerPort, SlackMessageHandlerPort,
    SlackTaskControlMessagePort, SlackUserGroupMembershipPort,
};
use reili_core::messaging::slack::{
    SlackMessageSearchPort, SlackReactionPort, SlackThreadHistoryPort, SlackThreadReplyPort,
};
use reili_core::queue::TaskJobQueuePort;
use reili_core::task::TaskJob;
use reili_core::task::{TaskProgressSessionFactoryPort, TaskResources, TaskRunnerPort};
use serde_json::{Value, json};
use thiserror::Error;

use crate::config::{
    AppConfig, EsaConfig, JudgeProviderConfig, LlmProviderConfig, SecretString, SlackConnectionMode,
};

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
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

struct BuildConnectorSetInput {
    datadog: DatadogMcpToolConfig,
    github_mcp: GitHubMcpConfig,
    github_scope_org: String,
    esa: Option<EsaConfig>,
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
    let connectors = build_connector_set(BuildConnectorSetInput {
        datadog: DatadogMcpToolConfig {
            api_key: config.datadog_api_key.clone(),
            app_key: config.datadog_app_key.clone(),
            site: config.datadog_site.clone(),
        },
        github_mcp: GitHubMcpConfig {
            url: config.github.url.clone(),
            app_id: config.github.app_id.clone(),
            private_key: config.github.private_key.clone(),
            installation_id: config.github.installation_id,
        },
        github_scope_org: config.github.scope_org.clone(),
        esa: config.esa.clone(),
    })?;
    let job_queue: Arc<TaskJobQueuePort> = Arc::new(InMemoryJobQueue::<TaskJob>::new());
    let in_flight_job_registry = InFlightJobRegistry::new();
    let provider_ports = create_provider_ports(CreateProviderPortsInput {
        llm_provider: &config.llm.provider,
        connectors,
        language: config.language.clone(),
        additional_system_prompt: config.additional_system_prompt.clone(),
    })
    .await?;

    let task_resources = TaskResources {
        slack_message_search_port,
        web_search_port: provider_ports.web_search_port,
    };
    let task_runner = provider_ports.task_runner;
    let enqueue_slack_message_handler: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            slack_reaction_port,
            slack_task_control_message_port: Arc::clone(&slack_task_control_message_port),
            slack_reply_port: Arc::clone(&slack_reply_port),
            logger: Arc::clone(&logger),
        }),
    );
    let slack_message_handler = build_slack_message_handler(BuildSlackMessageHandlerInput {
        config,
        next_handler: enqueue_slack_message_handler,
        slack_web_api_client: Arc::clone(&slack_web_api_client),
        slack_thread_history_port: Arc::clone(&slack_thread_history_port),
        logger: Arc::clone(&logger),
    })
    .await?;
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
            slack_bot_user_id: bot_user_id.clone(),
        },
        worker_concurrency: config.worker_concurrency,
        job_max_retry: config.job_max_retry,
        job_backoff_ms: config.job_backoff_ms,
    });
    let slack_signature_verifier = build_slack_signature_verifier(
        &config.slack_connection_mode,
        config
            .slack_signing_secret
            .as_ref()
            .map(SecretString::expose),
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

struct BuildSlackMessageHandlerInput<'a> {
    config: &'a AppConfig,
    next_handler: Arc<dyn SlackMessageHandlerPort>,
    slack_web_api_client: Arc<SlackWebApiClient>,
    slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    logger: Arc<dyn TaskLogger>,
}

struct SlackActorAuthorization {
    user_ids: Option<Vec<String>>,
    user_group_ids: Option<Vec<String>>,
    allow_bot: bool,
}

async fn build_slack_message_handler(
    input: BuildSlackMessageHandlerInput<'_>,
) -> Result<Arc<dyn SlackMessageHandlerPort>, RuntimeBootstrapError> {
    let channel_lookup_port: Arc<dyn SlackChannelLookupPort> = Arc::new(
        SlackChannelLookupAdapter::new(Arc::clone(&input.slack_web_api_client)),
    );
    let user_group_membership_port: Arc<dyn SlackUserGroupMembershipPort> = Arc::new(
        SlackUserGroupMembershipAdapter::new(Arc::clone(&input.slack_web_api_client)),
    );
    let actors = resolve_slack_actor_authorization(input.config);

    let mention_service = SlackMentionAuthorizationService::new(
        build_mention_authorization_policy(input.config, &actors),
        Arc::clone(&channel_lookup_port),
        Arc::clone(&user_group_membership_port),
        Arc::new(SlackEphemeralMessageAdapter::new(Arc::clone(
            &input.slack_web_api_client,
        ))) as Arc<dyn SlackEphemeralMessagePort>,
        Arc::clone(&input.logger),
    );
    let mention_gate: Arc<dyn SlackMessageHandlerPort> = Arc::new(
        SlackMentionAuthorizationGate::new(mention_service, Arc::clone(&input.next_handler)),
    );

    let auto_response_gate = build_slack_auto_response_gate(BuildSlackAutoResponseGateInput {
        config: input.config,
        actors: &actors,
        channel_lookup_port,
        user_group_membership_port,
        thread_history_port: input.slack_thread_history_port,
        next_handler: input.next_handler,
        logger: input.logger,
    })
    .await?;

    Ok(Arc::new(SlackInboundRouter::new(
        mention_gate,
        auto_response_gate,
    )))
}

fn resolve_slack_actor_authorization(config: &AppConfig) -> SlackActorAuthorization {
    let actors = config
        .slack_authorization
        .as_ref()
        .and_then(|authorization| authorization.actors.as_ref());

    SlackActorAuthorization {
        user_ids: actors.and_then(|actors| actors.user_ids.clone()),
        user_group_ids: actors.and_then(|actors| actors.user_group_ids.clone()),
        allow_bot: actors.is_some_and(|actors| actors.allow_bot),
    }
}

fn build_mention_authorization_policy(
    config: &AppConfig,
    actors: &SlackActorAuthorization,
) -> SlackAuthorizationPolicy {
    // An empty pattern list denies every mention, keeping the channels table opt-in.
    let channel_name_patterns = Some(
        config
            .mention_channel_patterns()
            .iter()
            .map(|pattern| pattern.as_str().to_string())
            .collect::<Vec<_>>(),
    );

    SlackAuthorizationPolicy::new(
        channel_name_patterns,
        actors.user_ids.clone(),
        actors.user_group_ids.clone(),
        actors.allow_bot,
    )
}

struct BuildSlackAutoResponseGateInput<'a> {
    config: &'a AppConfig,
    actors: &'a SlackActorAuthorization,
    channel_lookup_port: Arc<dyn SlackChannelLookupPort>,
    user_group_membership_port: Arc<dyn SlackUserGroupMembershipPort>,
    thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    next_handler: Arc<dyn SlackMessageHandlerPort>,
    logger: Arc<dyn TaskLogger>,
}

async fn build_slack_auto_response_gate(
    input: BuildSlackAutoResponseGateInput<'_>,
) -> Result<Option<Arc<dyn SlackMessageHandlerPort>>, RuntimeBootstrapError> {
    let channels = input
        .config
        .auto_response_channels()
        .map(|channel| SlackAutoResponsePolicy {
            names: channel.names.clone(),
            policy: channel.auto_response_policy.clone(),
        })
        .collect::<Vec<_>>();
    let Some(judge_llm) = input.config.judge_llm.as_ref() else {
        return Ok(None);
    };
    if channels.is_empty() {
        return Ok(None);
    }

    let actor_policy = SlackAuthorizationPolicy::new(
        None,
        input.actors.user_ids.clone(),
        input.actors.user_group_ids.clone(),
        input.actors.allow_bot,
    );

    Ok(Some(Arc::new(SlackAutoResponseGate::new(
        SlackAutoResponseGateDeps {
            channels,
            actor_policy,
            channel_lookup_port: input.channel_lookup_port,
            user_group_membership_port: input.user_group_membership_port,
            thread_history_port: input.thread_history_port,
            judge_port: create_auto_response_judge_port(judge_llm).await?,
            language: input.config.language.clone(),
            next_handler: input.next_handler,
            logger: input.logger,
        },
    ))))
}

async fn create_auto_response_judge_port(
    judge_llm: &JudgeProviderConfig,
) -> Result<Arc<dyn AutoResponseJudgePort>, RuntimeBootstrapError> {
    match judge_llm {
        JudgeProviderConfig::OpenAi { api_key, model } => Ok(
            create_openai_auto_response_judge_port(api_key.clone(), model.clone()),
        ),
        JudgeProviderConfig::Anthropic { api_key, model } => Ok(
            create_anthropic_auto_response_judge_port(api_key.clone(), model.clone()),
        ),
        JudgeProviderConfig::Bedrock {
            model_id,
            aws_profile,
            aws_region,
        } => Ok(
            create_bedrock_auto_response_judge_port(CreateBedrockAutoResponseJudgePortInput {
                model_id: model_id.clone(),
                aws_profile: aws_profile.clone(),
                aws_region: aws_region.clone(),
            })
            .await,
        ),
        JudgeProviderConfig::VertexAi {
            project_id,
            location,
            model_id,
        } => {
            let client = build_vertex_ai_client(project_id, location)?;

            Ok(create_vertex_ai_auto_response_judge_port(
                client,
                model_id.clone(),
            ))
        }
    }
}

fn build_slack_signature_verifier(
    connection_mode: &SlackConnectionMode,
    slack_signing_secret: Option<&str>,
) -> Result<Option<Arc<SlackSignatureVerifier>>, PortError> {
    match connection_mode {
        SlackConnectionMode::Http => slack_signing_secret
            .filter(|value| !value.trim().is_empty())
            .map(|value| SlackSignatureVerifier::new(SecretString::from(value)))
            .transpose()
            .map(|verifier| verifier.map(Arc::new)),
        SlackConnectionMode::SocketMode { .. } => Ok(None),
    }
}

fn build_connector_set(input: BuildConnectorSetInput) -> Result<ConnectorSet, PortError> {
    let mut connectors = ConnectorSet::default();
    connectors.push(Arc::new(DatadogConnector::new(input.datadog)));
    connectors.push(Arc::new(GitHubConnector::new(
        input.github_mcp,
        input.github_scope_org,
    )));
    if let Some(esa_config) = input.esa {
        connectors.push(Arc::new(build_esa_connector(esa_config)?));
    }

    Ok(connectors)
}

fn build_esa_connector(config: EsaConfig) -> Result<EsaConnector, PortError> {
    let post_search_port = build_esa_post_search_port(&config)?;

    Ok(EsaConnector::new(config.team_name, post_search_port))
}

fn build_esa_post_search_port(config: &EsaConfig) -> Result<Arc<dyn EsaPostSearchPort>, PortError> {
    let client = EsaClient::new(EsaClientConfig {
        access_token: config.access_token.clone(),
        team_name: config.team_name.clone(),
    })?;

    Ok(Arc::new(client))
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
                model: config.model.clone(),
                sub_agent_model: config.sub_agent_model.clone(),
                reasoning_effort: config.reasoning_effort.clone(),
                connectors: input.connectors,
                language: input.language,
                additional_system_prompt: input.additional_system_prompt,
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
                sub_agent_model: config.sub_agent_model.clone(),
                connectors: input.connectors,
                language: input.language,
                additional_system_prompt: input.additional_system_prompt,
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
                sub_agent_model_id: config.sub_agent_model_id.clone(),
                aws_profile: config.aws_profile.clone(),
                aws_region: config.aws_region.clone(),
                connectors: input.connectors,
                language: input.language,
                additional_system_prompt: input.additional_system_prompt,
            })),
        }),
        LlmProviderConfig::VertexAi(config) => {
            let client = build_vertex_ai_client(&config.project_id, &config.location)?;

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
                    sub_agent_model_id: config.sub_agent_model_id.clone(),
                    connectors: input.connectors,
                    language: input.language,
                    additional_system_prompt: input.additional_system_prompt,
                })),
            })
        }
    }
}

fn build_vertex_ai_client(
    project_id: &str,
    location: &str,
) -> Result<VertexAiGeminiClient, RuntimeBootstrapError> {
    VertexAiGeminiClient::builder()
        .with_project(project_id)
        .with_location(location)
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
    use reili_adapters::outbound::agents::DatadogMcpToolConfig;
    use reili_adapters::outbound::github::GitHubMcpConfig;

    use super::{BuildConnectorSetInput, build_connector_set, build_slack_signature_verifier};
    use crate::config::{EsaConfig, SecretString, SlackConnectionMode};

    fn sample_connector_set_input(esa: Option<EsaConfig>) -> BuildConnectorSetInput {
        BuildConnectorSetInput {
            datadog: DatadogMcpToolConfig {
                api_key: SecretString::from("api"),
                app_key: SecretString::from("app"),
                site: "datadoghq.com".to_string(),
            },
            github_mcp: GitHubMcpConfig {
                url: "https://api.githubcopilot.com/mcp/".to_string(),
                app_id: "12345".to_string(),
                private_key: SecretString::from("private-key"),
                installation_id: 99,
            },
            github_scope_org: "example-org".to_string(),
            esa,
        }
    }

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

    #[test]
    fn connector_set_omits_esa_when_not_configured() {
        let connectors =
            build_connector_set(sample_connector_set_input(None)).expect("build connector set");

        assert_eq!(connectors.len(), 2);
    }

    #[test]
    fn connector_set_includes_esa_when_configured() {
        let esa = EsaConfig {
            team_name: "docs".to_string(),
            access_token: SecretString::from("esa-token"),
        };
        let connectors = build_connector_set(sample_connector_set_input(Some(esa)))
            .expect("build connector set");

        assert_eq!(connectors.len(), 3);
    }
}
