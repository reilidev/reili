use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{error::PortError, task::TaskJob};

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
pub trait QueueJob: Send + Sync {
    fn job_id(&self) -> &str;
    fn retry_count(&self) -> u32;
    fn with_retry_count(&self, retry_count: u32) -> Self;
}

impl QueueJob for TaskJob {
    fn job_id(&self) -> &str {
        &self.job_id
    }

    fn retry_count(&self) -> u32 {
        self.retry_count
    }

    fn with_retry_count(&self, retry_count: u32) -> Self {
        Self {
            retry_count,
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteJobInput {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelJobInput {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailJobInput {
    pub job_id: String,
    pub reason: String,
    pub max_retry: u32,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobFailStatus {
    Requeued,
    DeadLetter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFailResult<TJob>
where
    TJob: QueueJob,
{
    pub status: JobFailStatus,
    pub job: TJob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelJobResult<TJob>
where
    TJob: QueueJob,
{
    Cancelled(TJob),
    AlreadyClaimed,
    NotFound,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait JobQueuePort<TJob>: Send + Sync
where
    TJob: QueueJob,
{
    async fn enqueue(&self, job: TJob) -> Result<(), PortError>;
    async fn claim(&self) -> Result<Option<TJob>, PortError>;
    async fn cancel(&self, input: CancelJobInput) -> Result<CancelJobResult<TJob>, PortError>;
    async fn complete(&self, input: CompleteJobInput) -> Result<(), PortError>;
    async fn fail(&self, input: FailJobInput) -> Result<JobFailResult<TJob>, PortError>;
    async fn get_depth(&self) -> Result<usize, PortError>;
}
