//! Durable Work State: objective ledgers, work items, structured investigation records, and the
//! repository port that persists them.

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod feedback;
pub mod investigation;
pub mod item;
pub mod ledger;
pub mod record;
pub mod report;
pub mod repository;
pub mod sanitize;
pub mod schema;
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
pub use record::{CommentId, RecordKey, RecordRef};
pub use report::{InvestigationReport, KnowledgeCandidate, WorkItemProposal};
#[cfg(any(test, feature = "test-support"))]
pub use repository::MockWorkStateRepositoryPort;
pub use repository::{
    StoredLedger, StoredWorkItem, WorkStateComment, WorkStateCommentKind, WorkStateError,
    WorkStateRepositoryPort,
};
pub use schema::{StateDecodeError, VersionedState, decode_state_json};
