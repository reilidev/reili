pub mod context;
pub mod job;
pub mod progress_event;
pub mod progress_reporting;
pub mod request;
pub mod task_runner;
pub mod telemetry;

pub use context::{TaskContext, TaskResources, TaskRuntime};
pub use job::{TaskJob, TaskJobPayload};
pub use progress_event::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort,
};
pub use progress_reporting::{
    CompleteTaskProgressSessionInput, StartTaskProgressSessionInput, TaskProgressScopeStatus,
    TaskProgressSessionCompletionStatus, TaskProgressSessionFactoryPort, TaskProgressSessionPort,
    TaskProgressUpdate,
};
pub use request::TaskRequest;
pub use task_runner::{LlmExecutionMetadata, RunTaskInput, TaskRunReport, TaskRunnerPort};
pub use telemetry::LlmUsageSnapshot;

#[cfg(any(test, feature = "test-support"))]
pub use progress_event::MockTaskProgressEventPort;
#[cfg(any(test, feature = "test-support"))]
pub use progress_reporting::{MockTaskProgressSessionFactoryPort, MockTaskProgressSessionPort};
#[cfg(any(test, feature = "test-support"))]
pub use task_runner::MockTaskRunnerPort;
