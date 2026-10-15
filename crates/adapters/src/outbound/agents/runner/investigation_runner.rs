//! Runs a goal cycle investigation on any rig completion client: the lead agent works the brief,
//! sub-agents and verification tools share one budget and one verification ledger, and the
//! structured report submitted through `submit_investigation_report` is returned.

use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    InvestigationRunOutcome, InvestigationRunReport, ReportSlot, RunInvestigationInput,
};
use reili_core::task::{LlmExecutionMetadata, TASK_RUNNER_PROGRESS_OWNER_ID, TaskRuntime};
use rig::completion::{Prompt, PromptError};
use rig::prelude::CompletionClient;

use super::execution_hook::AgentExecutionHook;
use super::provider_settings::LlmProviderSettings;
use super::usage_collector::LlmUsageCollector;
use crate::outbound::agents::connector::{
    ConnectorPrepareError, ConnectorPromptFact, ConnectorSet,
};
use crate::outbound::agents::task_agent::{
    AgentInstructionsConfig, BuildGoalAgentInput, BuildGoalPromptInput,
    CreateTaskAgentFactoryInput, TaskAgentConfig, TaskAgentExecutionContext, TaskAgentFactory,
    TaskAgentRunContext, build_goal_prompt,
};
use crate::outbound::agents::tools::VerificationToolset;

pub struct RunLlmInvestigationInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub sub_agent_client: C,
    pub settings: LlmProviderSettings,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
    pub run: RunInvestigationInput,
}

pub async fn run_investigation<C>(
    input: RunLlmInvestigationInput<C>,
) -> Result<InvestigationRunOutcome, AgentRunFailedError>
where
    C: CompletionClient + Clone + Send + Sync + 'static,
    C::CompletionModel: 'static,
{
    let run = input.run;
    if run.cancellation.is_cancelled() {
        return Ok(InvestigationRunOutcome::Cancelled);
    }
    let usage_collector = LlmUsageCollector::new();
    let runtime = TaskRuntime {
        started_at_iso: run.brief.now.to_rfc3339(),
        channel: format!("goal:{}", run.brief.goal_id),
        thread_ts: run.brief.run_id.clone(),
        retry_count: 0,
    };
    let prepared_connectors = input.connectors.prepare_all().await.map_err(|error| {
        let usage = usage_collector.snapshot();
        match error {
            ConnectorPrepareError::ConnectionFailed { message } => {
                AgentRunFailedError::new_permanent(usage, message)
            }
            ConnectorPrepareError::Other(port_error) => {
                AgentRunFailedError::new(usage, port_error.message)
            }
        }
    })?;
    let prompt_facts: Vec<ConnectorPromptFact> = prepared_connectors
        .iter()
        .flat_map(|connector| connector.prompt_facts())
        .collect();
    let prompt = build_goal_prompt(&BuildGoalPromptInput {
        brief: &run.brief,
        language: &input.language,
        prompt_facts: &prompt_facts,
    });
    let report_slot = ReportSlot::new();
    let verification = VerificationToolset::new(
        run.verifications.clone(),
        run.resources.metric_query_port.clone(),
    );
    let max_parallel_tools = run.budget.limits().max_parallel_tools;
    let execution = TaskAgentExecutionContext {
        logger: Arc::clone(&run.logger),
        runtime: runtime.clone(),
        cancellation: run.cancellation.clone(),
        on_progress_event: Arc::clone(&run.on_progress_event),
        usage_collector: usage_collector.clone(),
        budget: Some(run.budget.clone()),
    };
    let factory = TaskAgentFactory::new(CreateTaskAgentFactoryInput {
        client: input.client,
        sub_agent_client: input.sub_agent_client,
        config: TaskAgentConfig {
            settings: input.settings.clone(),
            instructions: AgentInstructionsConfig {
                language: input.language,
                additional_system_prompt: input.additional_system_prompt,
            },
        },
    });
    let agent = factory.build_goal_agent(BuildGoalAgentInput {
        run_context: TaskAgentRunContext {
            resources: Arc::new(run.resources),
            execution,
            slack_action_token: None,
            memory_items: Vec::new(),
            verification,
        },
        prepared_connectors,
        report_slot: report_slot.clone(),
        max_parallel_tools,
    });
    let hook = AgentExecutionHook::new(
        TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
        runtime,
        run.cancellation.clone(),
        Arc::clone(&run.logger),
        Arc::clone(&run.on_progress_event),
        usage_collector.clone(),
    )
    .with_budget(Some(run.budget.clone()));
    let tool_concurrency = usize::try_from(max_parallel_tools)
        .unwrap_or(1)
        .clamp(1, input.settings.tool_concurrency.max(1));

    let response = agent
        .prompt(prompt)
        .max_turns(input.settings.task_runner_max_turns)
        .tool_concurrency(tool_concurrency)
        .add_hook(hook)
        .extended_details()
        .await;

    let final_text = match response {
        Ok(response) => response.output().to_string(),
        Err(PromptError::PromptCancelled { .. }) if run.cancellation.is_cancelled() => {
            return Ok(InvestigationRunOutcome::Cancelled);
        }
        // A budget stop or turn limit still returns whatever report was submitted before it.
        Err(PromptError::PromptCancelled { .. } | PromptError::MaxTurnsError { .. }) => {
            String::new()
        }
        Err(error) => {
            if let Some(report) = report_slot.take() {
                return Ok(completed(
                    Some(report),
                    &usage_collector,
                    &input.settings,
                    String::new(),
                ));
            }
            return Err(AgentRunFailedError::new(
                usage_collector.snapshot(),
                error.to_string(),
            ));
        }
    };
    if run.cancellation.is_cancelled() {
        return Ok(InvestigationRunOutcome::Cancelled);
    }
    Ok(completed(
        report_slot.take(),
        &usage_collector,
        &input.settings,
        final_text,
    ))
}

fn completed(
    report: Option<reili_core::work::InvestigationReport>,
    usage_collector: &LlmUsageCollector,
    settings: &LlmProviderSettings,
    final_text: String,
) -> InvestigationRunOutcome {
    InvestigationRunOutcome::Completed(Box::new(InvestigationRunReport {
        report,
        usage: usage_collector.snapshot(),
        execution: LlmExecutionMetadata {
            provider: settings.provider.clone(),
            model: settings.task_runner_model.clone(),
        },
        final_text,
    }))
}
