pub mod context;
pub mod investigation_lead_runner;
pub mod job;
pub mod progress_event;
pub mod progress_reporting;
pub mod request;
pub mod telemetry;

pub use context::{InvestigationContext, InvestigationResources, InvestigationRuntime};
pub use investigation_lead_runner::{
    InvestigationLeadRunReport, InvestigationLeadRunnerPort, LlmExecutionMetadata,
    RunInvestigationLeadInput,
};
pub use job::{InvestigationJob, InvestigationJobPayload};
pub use progress_event::{
    INVESTIGATION_LEAD_PROGRESS_OWNER_ID, InvestigationProgressEvent,
    InvestigationProgressEventInput, InvestigationProgressEventPort,
};
pub use progress_reporting::{
    CompleteInvestigationProgressSessionInput, InvestigationProgressScopeStatus,
    InvestigationProgressSessionCompletionStatus, InvestigationProgressSessionFactoryPort,
    InvestigationProgressSessionPort, InvestigationProgressUpdate,
    StartInvestigationProgressSessionInput,
};
pub use request::InvestigationRequest;
pub use telemetry::LlmUsageSnapshot;

#[cfg(any(test, feature = "test-support"))]
pub use investigation_lead_runner::MockInvestigationLeadRunnerPort;
#[cfg(any(test, feature = "test-support"))]
pub use progress_event::MockInvestigationProgressEventPort;
#[cfg(any(test, feature = "test-support"))]
pub use progress_reporting::{
    MockInvestigationProgressSessionFactoryPort, MockInvestigationProgressSessionPort,
};
