use std::sync::Arc;

use reili_core::cycle_agent::{CycleAgentOutcome, CycleAgentRun, ReportSlot, RunCycleAgentInput};
use reili_core::error::AgentRunFailedError;
use reili_core::task::{ObjectiveCycleRuntimeInput, TASK_RUNNER_PROGRESS_OWNER_ID, TaskRuntime};
use rig::completion::{Prompt, PromptError};
use rig::prelude::CompletionClient;

use super::environment::{AgentEnvironment, prompt_facts};
use super::usage_collector::LlmUsageCollector;
use crate::outbound::agents::task_agent::{
    BuildObjectiveAgentInput, BuildObjectivePromptInput, OBJECTIVE_TOOL_CONCURRENCY,
    TaskAgentExecutionContext, TaskAgentRunContext, build_objective_prompt,
};

pub struct RunCycleAgentLlmInput<C> {
    pub environment: AgentEnvironment<C>,
    pub run: RunCycleAgentInput,
}

pub async fn run_cycle_agent<C>(
    input: RunCycleAgentLlmInput<C>,
) -> Result<CycleAgentOutcome, AgentRunFailedError>
where
    C: CompletionClient + Clone + Send + Sync + 'static,
    C::CompletionModel: 'static,
{
    let run = input.run;
    if run.cancellation.is_cancelled() {
        return Ok(CycleAgentOutcome::Cancelled);
    }
    let usage_collector = LlmUsageCollector::new();
    let runtime = TaskRuntime::for_objective_cycle(ObjectiveCycleRuntimeInput {
        objective_id: &run.brief.objective_id,
        run_id: &run.brief.run_id,
        started_at: run.brief.now,
    });
    let prepared_connectors = input
        .environment
        .prepare_connectors(&usage_collector)
        .await?;
    let prompt = build_objective_prompt(&BuildObjectivePromptInput {
        brief: &run.brief,
        language: &input.environment.language,
        prompt_facts: &prompt_facts(&prepared_connectors),
    });
    let report_slot = ReportSlot::new();
    let execution = TaskAgentExecutionContext {
        logger: Arc::clone(&run.logger),
        runtime,
        cancellation: run.cancellation.clone(),
        on_progress_event: Arc::clone(&run.on_progress_event),
        usage_collector: usage_collector.clone(),
        budget: run.budget.clone(),
    };
    let hook = execution.hook(TASK_RUNNER_PROGRESS_OWNER_ID.to_string());
    let agent = input
        .environment
        .agent_factory()
        .build_objective_agent(BuildObjectiveAgentInput {
            run_context: TaskAgentRunContext {
                resources: Arc::new(run.resources),
                execution,
                slack_action_token: None,
                memory_items: Vec::new(),
            },
            prepared_connectors,
            report_slot: report_slot.clone(),
        });
    let response = agent
        .prompt(prompt)
        .max_turns(input.environment.settings.task_runner_max_turns)
        .tool_concurrency(OBJECTIVE_TOOL_CONCURRENCY)
        .add_hook(hook)
        .extended_details()
        .await;

    match response {
        Ok(_) => {}
        Err(PromptError::PromptCancelled { .. }) if run.cancellation.is_cancelled() => {
            return Ok(CycleAgentOutcome::Cancelled);
        }
        // A budget stop or turn limit still returns whatever report was submitted before it.
        Err(PromptError::PromptCancelled { .. } | PromptError::MaxTurnsError { .. }) => {}
        Err(error) => {
            if let Some(report) = report_slot.take() {
                return Ok(completed(Some(report), &usage_collector));
            }
            return Err(AgentRunFailedError::new(
                usage_collector.snapshot(),
                error.to_string(),
            ));
        }
    }
    if run.cancellation.is_cancelled() {
        return Ok(CycleAgentOutcome::Cancelled);
    }
    Ok(completed(report_slot.take(), &usage_collector))
}

fn completed(
    report: Option<reili_core::work::CycleReport>,
    usage_collector: &LlmUsageCollector,
) -> CycleAgentOutcome {
    CycleAgentOutcome::Completed(Box::new(CycleAgentRun {
        report,
        usage: usage_collector.snapshot(),
    }))
}
