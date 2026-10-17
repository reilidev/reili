//! The Objective Ledger is the per-objective durable state (one Work State record per objective):
//! cycle checkpoints and lease, budget usage, and report de-duplication state.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::item::keep_last;
use crate::objective::{ObjectiveDefinition, ObjectiveId};

pub const OBJECTIVE_LEDGER_SCHEMA_VERSION: u32 = 1;
const MAX_CYCLE_RECORDS: usize = 20;
const MAX_REPORT_KEYS: usize = 50;
const MAX_LEARNED_FINGERPRINTS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveLifecycle {
    Active,
    /// `enabled = false` in config: no cycles run; existing work items are kept untouched.
    Disabled,
    /// Removed from config: no cycles run; the ledger and work items stay for humans.
    Retired,
}

impl ObjectiveLifecycle {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CyclePhase {
    Observing,
    Investigating,
    /// Investigation results are persisted to work items; LLM work is done.
    Reporting,
    Learning,
}

impl CyclePhase {
    /// Phases from which a resumed cycle can continue without repeating the investigation.
    #[must_use]
    pub fn investigation_persisted(self) -> bool {
        matches!(self, Self::Reporting | Self::Learning)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeDraft {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    pub shared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveCycle {
    pub run_id: String,
    #[serde(default)]
    pub scheduled_for: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub attempt: u32,
    pub phase: CyclePhase,
    pub lease_holder: String,
    pub lease_expires_at: DateTime<Utc>,
    /// Keys of work item records touched by this cycle and awaiting report.
    #[serde(default)]
    pub pending_notifications: Vec<String>,
    /// Gated knowledge awaiting persistence in the Learn phase.
    #[serde(default)]
    pub pending_knowledge: Vec<KnowledgeDraft>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub no_new_findings: bool,
    #[serde(default)]
    pub llm_tokens: u64,
}

impl ActiveCycle {
    #[must_use]
    pub fn lease_is_live(&self, now: DateTime<Utc>) -> bool {
        self.lease_expires_at > now
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CycleResult {
    Completed,
    NoNewFindings,
    Failed { reason: String },
    Skipped { reason: String },
    BudgetExhausted { reason: String },
    Abandoned { reason: String },
}

impl CycleResult {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Completed => "completed".to_string(),
            Self::NoNewFindings => "completed (no new findings)".to_string(),
            Self::Failed { reason } => format!("failed: {reason}"),
            Self::Skipped { reason } => format!("skipped: {reason}"),
            Self::BudgetExhausted { reason } => format!("budget exhausted: {reason}"),
            Self::Abandoned { reason } => format!("abandoned: {reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleRecord {
    pub run_id: String,
    #[serde(default)]
    pub scheduled_for: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub attempts: u32,
    pub result: CycleResult,
    #[serde(default)]
    pub llm_tokens: u64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub touched_items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyUsage {
    pub day: NaiveDate,
    pub llm_tokens: u64,
    pub cycles: u32,
    pub reports_posted: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportState {
    #[serde(default)]
    pub last_report_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub recent_report_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveLedgerState {
    pub schema_version: u32,
    pub objective_id: ObjectiveId,
    pub definition_fingerprint: String,
    pub description: String,
    pub lifecycle: ObjectiveLifecycle,
    #[serde(default)]
    pub active_cycle: Option<ActiveCycle>,
    #[serde(default)]
    pub last_scheduled_for: Option<DateTime<Utc>>,
    #[serde(default)]
    pub cycles: Vec<CycleRecord>,
    #[serde(default)]
    pub usage: Option<DailyUsage>,
    #[serde(default)]
    pub reports: ReportState,
    #[serde(default)]
    pub learned_fact_fingerprints: Vec<String>,
    #[serde(default)]
    pub definition_changes: Vec<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub revision: u64,
}

impl ObjectiveLedgerState {
    #[must_use]
    pub fn new(objective: &ObjectiveDefinition, now: DateTime<Utc>) -> Self {
        Self {
            schema_version: OBJECTIVE_LEDGER_SCHEMA_VERSION,
            objective_id: objective.id.clone(),
            definition_fingerprint: objective.fingerprint(),
            description: objective.description.clone(),
            lifecycle: if objective.enabled {
                ObjectiveLifecycle::Active
            } else {
                ObjectiveLifecycle::Disabled
            },
            active_cycle: None,
            last_scheduled_for: None,
            cycles: Vec::new(),
            usage: None,
            reports: ReportState::default(),
            learned_fact_fingerprints: Vec::new(),
            definition_changes: Vec::new(),
            created_at: now,
            updated_at: now,
            revision: 0,
        }
    }

    /// Usage for `today`, resetting the counters when the day rolled over.
    pub fn usage_for(&mut self, today: NaiveDate) -> &mut DailyUsage {
        if self.usage.as_ref().is_none_or(|usage| usage.day != today) {
            self.usage = Some(DailyUsage {
                day: today,
                llm_tokens: 0,
                cycles: 0,
                reports_posted: 0,
            });
        }
        self.usage.get_or_insert(DailyUsage {
            day: today,
            llm_tokens: 0,
            cycles: 0,
            reports_posted: 0,
        })
    }

    #[must_use]
    pub fn tokens_used_on(&self, today: NaiveDate) -> u64 {
        self.usage
            .as_ref()
            .filter(|usage| usage.day == today)
            .map_or(0, |usage| usage.llm_tokens)
    }

    #[must_use]
    pub fn reports_posted_on(&self, today: NaiveDate) -> u32 {
        self.usage
            .as_ref()
            .filter(|usage| usage.day == today)
            .map_or(0, |usage| usage.reports_posted)
    }

    pub fn record_cycle(&mut self, record: CycleRecord) {
        self.cycles.push(record);
        keep_last(&mut self.cycles, MAX_CYCLE_RECORDS);
    }

    #[must_use]
    pub fn last_cycle(&self) -> Option<&CycleRecord> {
        self.cycles.last()
    }

    pub fn remember_report_key(&mut self, key: String, at: DateTime<Utc>) {
        self.reports.recent_report_keys.push(key);
        keep_last(&mut self.reports.recent_report_keys, MAX_REPORT_KEYS);
        self.reports.last_report_at = Some(at);
    }

    #[must_use]
    pub fn has_learned(&self, fingerprint: &str) -> bool {
        self.learned_fact_fingerprints
            .iter()
            .any(|known| known == fingerprint)
    }

    pub fn remember_learned(&mut self, fingerprint: String) {
        self.learned_fact_fingerprints.push(fingerprint);
        keep_last(
            &mut self.learned_fact_fingerprints,
            MAX_LEARNED_FINGERPRINTS,
        );
    }

    /// Consecutive failed or abandoned cycles at the end of the history.
    #[must_use]
    pub fn consecutive_failures(&self) -> usize {
        self.cycles
            .iter()
            .rev()
            .take_while(|record| {
                matches!(
                    record.result,
                    CycleResult::Failed { .. } | CycleResult::Abandoned { .. }
                )
            })
            .count()
    }

    /// Exponential back-off after consecutive failures (capped at 6 hours), so a broken
    /// dependency does not burn budget on every tick.
    #[must_use]
    pub fn failure_backoff_until(&self) -> Option<DateTime<Utc>> {
        let failures = self.consecutive_failures();
        if failures == 0 {
            return None;
        }
        let last = self.last_cycle()?;
        let exponent = u32::try_from(failures.saturating_sub(1).min(8)).unwrap_or(8);
        let minutes = (5_i64 * 2_i64.pow(exponent)).min(360);
        Some(last.finished_at + Duration::minutes(minutes))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate, TimeZone, Utc};

    use super::{CycleRecord, CycleResult, ObjectiveLedgerState, ObjectiveLifecycle};
    use crate::objective::test_support::sample_objective;

    fn record(result: CycleResult, minutes: i64) -> CycleRecord {
        let at = Utc.timestamp_opt(1_700_000_000, 0).single().expect("time")
            + Duration::minutes(minutes);
        CycleRecord {
            run_id: format!("run-{minutes}"),
            scheduled_for: None,
            started_at: at,
            finished_at: at,
            attempts: 1,
            result,
            llm_tokens: 0,
            summary: String::new(),
            touched_items: Vec::new(),
        }
    }

    #[test]
    fn new_ledger_reflects_enabled_flag() {
        let mut objective = sample_objective("checkout");
        objective.enabled = false;
        let now = Utc.timestamp_opt(1_700_000_000, 0).single().expect("time");

        let ledger = ObjectiveLedgerState::new(&objective, now);

        assert_eq!(ledger.lifecycle, ObjectiveLifecycle::Disabled);
        assert_eq!(ledger.definition_fingerprint, objective.fingerprint());
    }

    #[test]
    fn usage_resets_on_new_day() {
        let objective = sample_objective("checkout");
        let now = Utc.timestamp_opt(1_700_000_000, 0).single().expect("time");
        let mut ledger = ObjectiveLedgerState::new(&objective, now);
        let day1 = NaiveDate::from_ymd_opt(2026, 10, 15).expect("date");
        let day2 = NaiveDate::from_ymd_opt(2026, 10, 16).expect("date");

        ledger.usage_for(day1).llm_tokens += 100;
        assert_eq!(ledger.tokens_used_on(day1), 100);
        assert_eq!(ledger.tokens_used_on(day2), 0);
        ledger.usage_for(day2).llm_tokens += 5;
        assert_eq!(ledger.tokens_used_on(day2), 5);
    }

    #[test]
    fn backs_off_exponentially_after_failures() {
        let objective = sample_objective("checkout");
        let now = Utc.timestamp_opt(1_700_000_000, 0).single().expect("time");
        let mut ledger = ObjectiveLedgerState::new(&objective, now);
        ledger.record_cycle(record(CycleResult::Completed, 0));
        assert!(ledger.failure_backoff_until().is_none());

        ledger.record_cycle(record(
            CycleResult::Failed {
                reason: "x".to_string(),
            },
            10,
        ));
        ledger.record_cycle(record(
            CycleResult::Failed {
                reason: "x".to_string(),
            },
            20,
        ));

        assert_eq!(ledger.consecutive_failures(), 2);
        let last = ledger.last_cycle().expect("last").finished_at;
        assert_eq!(
            ledger.failure_backoff_until(),
            Some(last + Duration::minutes(10))
        );
    }
}
