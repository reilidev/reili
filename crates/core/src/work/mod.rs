//! Durable Work State: goal ledgers, work items, structured investigation records, and the
//! repository port that persists them.

pub mod feedback;
pub mod investigation;
pub mod item;
pub mod ledger;
pub mod report;
pub mod repository;
pub mod sanitize;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use feedback::{HumanCommand, HumanComment, ParsedHumanComment, parse_human_comment};
pub use investigation::{
    EvidenceRef, EvidenceSource, Finding, Hypothesis, HypothesisStatus, OutcomeMetric,
    Recommendation, TimeRange,
};
pub use item::{
    FeedbackNote, HumanDecisionRequest, NotificationState, OutcomeAssessment, OutcomeTracking,
    OutcomeVerdict, Severity, StatusActor, StatusChange, WORK_ITEM_SCHEMA_VERSION, WorkItemKey,
    WorkItemKind, WorkItemState, WorkItemStatus, token_jaccard,
};
pub use ledger::{
    ActiveCycle, CriterionHistory, CriterionObservation, CyclePhase, CycleRecord, CycleResult,
    CycleTrigger, DailyUsage, GOAL_LEDGER_SCHEMA_VERSION, GoalLedgerState, GoalLifecycle,
    KnowledgeDraft, ReportState,
};
pub use report::{InvestigationReport, KnowledgeCandidate, WorkItemProposal};
#[cfg(any(test, feature = "test-support"))]
pub use repository::MockWorkStateRepositoryPort;
pub use repository::{
    IssueRef, StoredLedger, StoredWorkItem, WorkStateComment, WorkStateCommentKind, WorkStateError,
    WorkStateRepositoryPort,
};
