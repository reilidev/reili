//! Port for the agent's work in an objective cycle, run by the agent stack, plus the shared run budget the
//! agent tools report into.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::AgentRunFailedError;
use crate::logger::Logger;
use crate::task::{
    LlmExecutionMetadata, LlmUsageSnapshot, TaskCancellation, TaskProgressEventPort, TaskResources,
};
use crate::work::CycleReport;

/// Whether a section of the brief was authored by Reili/config (trusted) or comes from external
/// systems and humans (data only, never instructions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefTrust {
    Trusted,
    Untrusted { source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefSection {
    pub heading: String,
    pub body: String,
    pub trust: BriefTrust,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleBrief {
    pub run_id: String,
    pub objective_id: String,
    pub sections: Vec<BriefSection>,
    pub now: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub max_tool_calls: u32,
    pub max_llm_tokens: u64,
    pub max_sub_agents: u32,
    pub max_parallel_tools: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDecision {
    Allow,
    /// Close to the limit: the agent should wrap up and submit its report.
    WrapUp(String),
    Deny(String),
}

/// Shared, thread-safe counters enforcing a run's limits across the lead agent and every spawned
/// sub-agent. Exceeding a limit stops further tool use; the agent is told to submit what it has.
#[derive(Debug, Clone)]
pub struct RunBudget {
    limits: RunLimits,
    tool_calls: Arc<AtomicU32>,
    sub_agents: Arc<AtomicU32>,
    llm_tokens: Arc<AtomicU64>,
}

const REPORT_TOOL_NAME: &str = "submit_cycle_report";
const WRAP_UP_RATIO_NUMERATOR: u64 = 9;
const WRAP_UP_RATIO_DENOMINATOR: u64 = 10;

impl RunBudget {
    #[must_use]
    pub fn new(limits: RunLimits) -> Self {
        Self {
            limits,
            tool_calls: Arc::new(AtomicU32::new(0)),
            sub_agents: Arc::new(AtomicU32::new(0)),
            llm_tokens: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn limits(&self) -> RunLimits {
        self.limits
    }

    /// Called before every tool call. The report submission tool is always allowed so the agent
    /// can hand in partial results.
    pub fn on_tool_call(&self, tool_name: &str) -> BudgetDecision {
        if tool_name == REPORT_TOOL_NAME {
            return BudgetDecision::Allow;
        }
        if self.tokens_exhausted() {
            return BudgetDecision::Deny(format!(
                "LLM token budget ({}) exhausted. Call {REPORT_TOOL_NAME} now with what you have; keep uncertain conclusions as hypotheses.",
                self.limits.max_llm_tokens
            ));
        }
        let used = self.tool_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if used > self.limits.max_tool_calls {
            return BudgetDecision::Deny(format!(
                "Tool call budget ({}) exhausted. Call {REPORT_TOOL_NAME} now with what you have; keep uncertain conclusions as hypotheses.",
                self.limits.max_tool_calls
            ));
        }
        if u64::from(used) * WRAP_UP_RATIO_DENOMINATOR
            >= u64::from(self.limits.max_tool_calls) * WRAP_UP_RATIO_NUMERATOR
        {
            return BudgetDecision::WrapUp(format!(
                "{used}/{} tool calls used; finish the current step and submit the report soon.",
                self.limits.max_tool_calls
            ));
        }
        BudgetDecision::Allow
    }

    /// Called before spawning a sub-agent.
    pub fn on_spawn_sub_agent(&self) -> BudgetDecision {
        let spawned = self.sub_agents.fetch_add(1, Ordering::SeqCst) + 1;
        if spawned > self.limits.max_sub_agents {
            return BudgetDecision::Deny(format!(
                "Sub-agent budget ({}) exhausted; continue with the evidence already gathered.",
                self.limits.max_sub_agents
            ));
        }
        BudgetDecision::Allow
    }

    pub fn record_tokens(&self, tokens: u64) {
        self.llm_tokens.fetch_add(tokens, Ordering::SeqCst);
    }

    #[must_use]
    pub fn tokens_used(&self) -> u64 {
        self.llm_tokens.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn tool_calls_used(&self) -> u32 {
        self.tool_calls.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn tokens_exhausted(&self) -> bool {
        self.tokens_used() >= self.limits.max_llm_tokens
    }
}

/// Captures the single structured report the agent submits.
#[derive(Debug, Clone, Default)]
pub struct ReportSlot {
    report: Arc<Mutex<Option<CycleReport>>>,
}

impl ReportSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&self, report: CycleReport) {
        let mut slot = match self.report.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(report);
    }

    #[must_use]
    pub fn take(&self) -> Option<CycleReport> {
        match self.report.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

pub struct RunCycleAgentInput {
    pub brief: CycleBrief,
    pub budget: RunBudget,
    pub resources: TaskResources,
    pub cancellation: TaskCancellation,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub logger: Arc<dyn Logger>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CycleAgentRun {
    /// `None` when the agent stopped without submitting a report.
    pub report: Option<CycleReport>,
    pub usage: LlmUsageSnapshot,
    pub execution: LlmExecutionMetadata,
    /// Final free-text answer, kept for diagnostics only.
    pub final_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CycleAgentOutcome {
    Completed(Box<CycleAgentRun>),
    Cancelled,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait CycleAgentPort: Send + Sync {
    async fn run(
        &self,
        input: RunCycleAgentInput,
    ) -> Result<CycleAgentOutcome, AgentRunFailedError>;
}

#[cfg(test)]
mod tests {
    use super::{BudgetDecision, RunBudget, RunLimits};

    fn budget() -> RunBudget {
        RunBudget::new(RunLimits {
            max_tool_calls: 10,
            max_llm_tokens: 1_000,
            max_sub_agents: 1,
            max_parallel_tools: 2,
        })
    }

    #[test]
    fn denies_tool_calls_beyond_limit_but_allows_report() {
        let budget = budget();
        for _ in 0..8 {
            assert_eq!(budget.on_tool_call("search"), BudgetDecision::Allow);
        }
        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::WrapUp(_)
        ));
        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::WrapUp(_)
        ));
        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::Deny(_)
        ));
        assert_eq!(
            budget.on_tool_call("submit_cycle_report"),
            BudgetDecision::Allow
        );
    }

    #[test]
    fn token_exhaustion_denies_tools() {
        let budget = budget();
        budget.record_tokens(950);
        assert!(!budget.tokens_exhausted());
        budget.record_tokens(50);

        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::Deny(_)
        ));
    }

    #[test]
    fn limits_sub_agents() {
        let budget = budget();

        assert_eq!(budget.on_spawn_sub_agent(), BudgetDecision::Allow);
        assert!(matches!(
            budget.on_spawn_sub_agent(),
            BudgetDecision::Deny(_)
        ));
    }
}
