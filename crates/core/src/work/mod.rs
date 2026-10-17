//! Durable Work State: objective ledgers, work items, structured investigation records, and the
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

pub use feedback::HumanComment;
pub use investigation::{
    EvidenceRef, EvidenceSource, Finding, Hypothesis, HypothesisStatus, Recommendation, TimeRange,
};
pub use item::{
    FeedbackNote, HumanDecisionRequest, NotificationState, Severity, StatusActor, StatusChange,
    WORK_ITEM_SCHEMA_VERSION, WorkItemKey, WorkItemKind, WorkItemState, WorkItemStatus,
    token_jaccard,
};
pub use ledger::{
    ActiveCycle, CyclePhase, CycleRecord, CycleResult, DailyUsage, KnowledgeDraft,
    OBJECTIVE_LEDGER_SCHEMA_VERSION, ObjectiveLedgerState, ObjectiveLifecycle, ReportState,
};
pub use report::{InvestigationReport, KnowledgeCandidate, WorkItemProposal};
#[cfg(any(test, feature = "test-support"))]
pub use repository::MockWorkStateRepositoryPort;
pub use repository::{
    IssueRef, StoredLedger, StoredWorkItem, WorkStateComment, WorkStateCommentKind, WorkStateError,
    WorkStateRepositoryPort,
};
