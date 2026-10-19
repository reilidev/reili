use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::report_slot::SUBMIT_CYCLE_REPORT_TOOL_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub max_tool_calls: u32,
    pub max_llm_tokens: u64,
    pub max_sub_agents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDecision {
    Allow,
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
    pub fn unlimited() -> Self {
        Self::new(RunLimits {
            max_tool_calls: u32::MAX,
            max_llm_tokens: u64::MAX,
            max_sub_agents: u32::MAX,
        })
    }

    #[must_use]
    pub fn limits(&self) -> RunLimits {
        self.limits
    }

    /// Called before every tool call. The report submission tool is always allowed so the agent
    /// can hand in partial results.
    pub fn on_tool_call(&self, tool_name: &str) -> BudgetDecision {
        if tool_name == SUBMIT_CYCLE_REPORT_TOOL_NAME {
            return BudgetDecision::Allow;
        }
        if self.tokens_exhausted() {
            return submit_now(&format!(
                "LLM token budget ({})",
                self.limits.max_llm_tokens
            ));
        }
        let used = self
            .tool_calls
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        if used > self.limits.max_tool_calls {
            return submit_now(&format!(
                "Tool call budget ({})",
                self.limits.max_tool_calls
            ));
        }
        BudgetDecision::Allow
    }

    pub fn on_spawn_sub_agent(&self) -> BudgetDecision {
        let spawned = self
            .sub_agents
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
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

    /// Either the token or the tool call budget is used up.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.tokens_exhausted()
            || self.tool_calls.load(Ordering::SeqCst) >= self.limits.max_tool_calls
    }

    fn tokens_exhausted(&self) -> bool {
        self.tokens_used() >= self.limits.max_llm_tokens
    }
}

fn submit_now(exhausted_budget: &str) -> BudgetDecision {
    BudgetDecision::Deny(format!(
        "{exhausted_budget} exhausted. Call {SUBMIT_CYCLE_REPORT_TOOL_NAME} now with what you have; keep uncertain conclusions as hypotheses."
    ))
}

#[cfg(test)]
mod tests {
    use super::{BudgetDecision, RunBudget, RunLimits};
    use crate::cycle_agent::SUBMIT_CYCLE_REPORT_TOOL_NAME;

    fn budget() -> RunBudget {
        RunBudget::new(RunLimits {
            max_tool_calls: 10,
            max_llm_tokens: 1_000,
            max_sub_agents: 1,
        })
    }

    #[test]
    fn denies_tool_calls_beyond_limit_but_allows_report() {
        let budget = budget();
        for _ in 0..10 {
            assert_eq!(budget.on_tool_call("search"), BudgetDecision::Allow);
        }
        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::Deny(_)
        ));
        assert_eq!(
            budget.on_tool_call(SUBMIT_CYCLE_REPORT_TOOL_NAME),
            BudgetDecision::Allow
        );
    }

    #[test]
    fn token_exhaustion_denies_tools() {
        let budget = budget();
        budget.record_tokens(950);
        assert_eq!(budget.on_tool_call("search"), BudgetDecision::Allow);
        budget.record_tokens(50);

        assert!(matches!(
            budget.on_tool_call("search"),
            BudgetDecision::Deny(_)
        ));
    }

    #[test]
    fn is_exhausted_when_tokens_run_out() {
        let budget = budget();
        budget.record_tokens(1_000);

        assert!(budget.is_exhausted());
    }

    #[test]
    fn is_exhausted_when_tool_calls_run_out() {
        let budget = budget();
        for _ in 0..10 {
            budget.on_tool_call("search");
        }

        assert!(budget.is_exhausted());
    }

    #[test]
    fn is_not_exhausted_below_both_limits() {
        let budget = budget();
        budget.record_tokens(999);
        for _ in 0..9 {
            budget.on_tool_call("search");
        }

        assert!(!budget.is_exhausted());
    }

    #[test]
    fn unlimited_never_denies() {
        let budget = RunBudget::unlimited();
        budget.record_tokens(u64::MAX / 2);

        for _ in 0..1_000 {
            assert_eq!(budget.on_tool_call("search"), BudgetDecision::Allow);
            assert_eq!(budget.on_spawn_sub_agent(), BudgetDecision::Allow);
        }
        assert!(!budget.is_exhausted());
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
