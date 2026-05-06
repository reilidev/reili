use std::collections::HashMap;
use std::sync::Arc;

use reili_core::task::TaskCancellation;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightJobCancellationInfo {
    pub job_id: String,
    pub cancel_requested_by_user_id: Option<String>,
    pub cancel_requested_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachCancellationResult {
    Running(InFlightJobCancellationInfo),
    CancelRequested(InFlightJobCancellationInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestCancelInFlightJobResult {
    Accepted(InFlightJobCancellationInfo),
    AlreadyRequested(InFlightJobCancellationInfo),
}

#[derive(Debug)]
struct InFlightJobRecord {
    job_id: String,
    cancel_requested_by_user_id: Option<String>,
    cancel_requested_at: Option<String>,
}

impl InFlightJobRecord {
    fn cancellation_info(&self) -> InFlightJobCancellationInfo {
        InFlightJobCancellationInfo {
            job_id: self.job_id.clone(),
            cancel_requested_by_user_id: self.cancel_requested_by_user_id.clone(),
            cancel_requested_at: self.cancel_requested_at.clone(),
        }
    }
}

#[derive(Debug, Default)]
struct InFlightJobState {
    cancellation_requests: HashMap<String, InFlightJobRecord>,
    cancellations: HashMap<String, TaskCancellation>,
}

#[derive(Debug, Clone, Default)]
pub struct InFlightJobRegistry {
    state: Arc<Mutex<InFlightJobState>>,
}

impl InFlightJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register_claimed(&self, job_id: String) -> InFlightJobCancellationInfo {
        InFlightJobCancellationInfo {
            job_id,
            cancel_requested_by_user_id: None,
            cancel_requested_at: None,
        }
    }

    pub async fn attach_cancellation(
        &self,
        job_id: &str,
        cancellation: TaskCancellation,
    ) -> AttachCancellationResult {
        let mut state = self.state.lock().await;
        state
            .cancellations
            .insert(job_id.to_string(), cancellation.clone());
        if let Some(record) = state.cancellation_requests.get(job_id) {
            cancellation.cancel();
            return AttachCancellationResult::CancelRequested(record.cancellation_info());
        }
        AttachCancellationResult::Running(InFlightJobCancellationInfo {
            job_id: job_id.to_string(),
            cancel_requested_by_user_id: None,
            cancel_requested_at: None,
        })
    }

    pub async fn request_cancel(
        &self,
        job_id: &str,
        requested_by_user_id: String,
        requested_at: String,
    ) -> RequestCancelInFlightJobResult {
        let mut state = self.state.lock().await;
        if let Some(record) = state.cancellation_requests.get(job_id) {
            return RequestCancelInFlightJobResult::AlreadyRequested(record.cancellation_info());
        }

        let record = InFlightJobRecord {
            job_id: job_id.to_string(),
            cancel_requested_by_user_id: Some(requested_by_user_id),
            cancel_requested_at: Some(requested_at),
        };
        let cancellation_info = record.cancellation_info();
        state
            .cancellation_requests
            .insert(job_id.to_string(), record);
        if let Some(cancellation) = state.cancellations.get(job_id).cloned() {
            cancellation.cancel();
        }

        RequestCancelInFlightJobResult::Accepted(cancellation_info)
    }

    pub async fn remove(&self, job_id: &str) -> Option<InFlightJobCancellationInfo> {
        let mut state = self.state.lock().await;
        let removed_request = state.cancellation_requests.remove(job_id);
        let removed_cancellation = state.cancellations.remove(job_id);
        removed_request
            .map(|record| record.cancellation_info())
            .or_else(|| {
                removed_cancellation.map(|_| InFlightJobCancellationInfo {
                    job_id: job_id.to_string(),
                    cancel_requested_by_user_id: None,
                    cancel_requested_at: None,
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{AttachCancellationResult, InFlightJobRegistry, RequestCancelInFlightJobResult};
    use reili_core::task::TaskCancellation;

    #[tokio::test]
    async fn request_cancel_before_source_attach_is_observed_on_attach() {
        let registry = InFlightJobRegistry::new();

        assert!(matches!(
            registry
                .request_cancel(
                    "job-1",
                    "U001".to_string(),
                    "2026-03-31T00:00:00.000Z".to_string(),
                )
                .await,
            RequestCancelInFlightJobResult::Accepted(_)
        ));

        let source = TaskCancellation::new();
        let result = registry.attach_cancellation("job-1", source.clone()).await;

        assert!(source.is_cancelled());
        assert!(matches!(
            result,
            AttachCancellationResult::CancelRequested(_)
        ));
    }

    #[tokio::test]
    async fn request_cancel_cancels_attached_source_immediately() {
        let registry = InFlightJobRegistry::new();
        let source = TaskCancellation::new();

        let _ = registry.register_claimed("job-1".to_string()).await;
        let _ = registry.attach_cancellation("job-1", source.clone()).await;
        let _ = registry
            .request_cancel(
                "job-1",
                "U001".to_string(),
                "2026-03-31T00:00:00.000Z".to_string(),
            )
            .await;

        assert!(source.is_cancelled());
    }

    #[tokio::test]
    async fn request_cancel_after_register_claimed_is_accepted() {
        let registry = InFlightJobRegistry::new();

        let _ = registry.register_claimed("job-1".to_string()).await;
        let result = registry
            .request_cancel(
                "job-1",
                "U001".to_string(),
                "2026-03-31T00:00:00.000Z".to_string(),
            )
            .await;

        assert!(matches!(
            result,
            RequestCancelInFlightJobResult::Accepted(_)
        ));
    }

    #[tokio::test]
    async fn remove_drops_registered_job() {
        let registry = InFlightJobRegistry::new();
        let source = TaskCancellation::new();

        let _ = registry.register_claimed("job-1".to_string()).await;
        let _ = registry.attach_cancellation("job-1", source).await;
        assert_eq!(
            registry.remove("job-1").await.expect("removed job").job_id,
            "job-1"
        );
        assert!(registry.remove("job-1").await.is_none());
    }
}
