//! Objective work on the team's tracker: work items, the structured report vocabulary, and the
//! port to the tracker.

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod evidence;
pub mod item;
pub mod port;
pub mod record;
pub mod report;
pub mod sanitize;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use evidence::{
    EvidenceRef, EvidenceSource, Finding, Hypothesis, HypothesisStatus, Recommendation, TimeRange,
};
pub use item::{
    CloseReason, NewWorkItem, Severity, WorkItem, WorkItemComment, WorkItemKey, WorkItemState,
    WorkItemStatus,
};
#[cfg(any(test, feature = "test-support"))]
pub use port::MockWorkStatePort;
pub use port::{WorkStateError, WorkStatePort};
pub use record::{RecordKey, RecordRef};
pub use report::{CycleReport, KnowledgeCandidate, KnowledgeDraft, WorkItemProposal};
