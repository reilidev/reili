use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::queue::{
    CompleteJobInput, FailJobInput, JobFailResult, JobFailStatus, JobQueuePort, QueueJob,
};
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Debug, Clone)]
struct DelayedJob<TJob>
where
    TJob: QueueJob + Clone,
{
    job: TJob,
    available_at: Instant,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DeadLetterJob<TJob>
where
    TJob: QueueJob + Clone,
{
    job: TJob,
    reason: String,
    failed_at: String,
}

#[derive(Debug)]
struct InnerState<TJob>
where
    TJob: QueueJob + Clone,
{
    pending_jobs: VecDeque<TJob>,
    delayed_jobs: Vec<DelayedJob<TJob>>,
    claimed_jobs: HashMap<String, TJob>,
    dead_letter_jobs: Vec<DeadLetterJob<TJob>>,
}

impl<TJob> Default for InnerState<TJob>
where
    TJob: QueueJob + Clone,
{
    fn default() -> Self {
        Self {
            pending_jobs: VecDeque::new(),
            delayed_jobs: Vec::new(),
            claimed_jobs: HashMap::new(),
            dead_letter_jobs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryJobQueue<TJob>
where
    TJob: QueueJob + Clone,
{
    state: Arc<Mutex<InnerState<TJob>>>,
}

impl<TJob> InMemoryJobQueue<TJob>
where
    TJob: QueueJob + Clone,
{
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(InnerState::default())),
        }
    }

    fn promote_ready_delayed_jobs(state: &mut InnerState<TJob>) {
        if state.delayed_jobs.is_empty() {
            return;
        }

        let now = Instant::now();
        let delayed_jobs = std::mem::take(&mut state.delayed_jobs);
        let mut remaining_delayed_jobs = Vec::with_capacity(delayed_jobs.len());

        for delayed_job in delayed_jobs {
            if delayed_job.available_at <= now {
                state.pending_jobs.push_back(delayed_job.job);
                continue;
            }

            remaining_delayed_jobs.push(delayed_job);
        }

        state.delayed_jobs = remaining_delayed_jobs;
    }
}

impl<TJob> Default for InMemoryJobQueue<TJob>
where
    TJob: QueueJob + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<TJob> JobQueuePort<TJob> for InMemoryJobQueue<TJob>
where
    TJob: QueueJob + Clone + 'static,
{
    async fn enqueue(&self, job: TJob) -> Result<(), PortError> {
        let mut state = self.state.lock().await;
        state.pending_jobs.push_back(job);
        Ok(())
    }

    async fn claim(&self) -> Result<Option<TJob>, PortError> {
        let mut state = self.state.lock().await;
        Self::promote_ready_delayed_jobs(&mut state);

        let next_job = match state.pending_jobs.pop_front() {
            Some(job) => job,
            None => return Ok(None),
        };

        state
            .claimed_jobs
            .insert(next_job.job_id().to_string(), next_job.clone());

        Ok(Some(next_job))
    }

    async fn complete(&self, input: CompleteJobInput) -> Result<(), PortError> {
        let mut state = self.state.lock().await;
        state.claimed_jobs.remove(&input.job_id);
        Ok(())
    }

    async fn fail(&self, input: FailJobInput) -> Result<JobFailResult<TJob>, PortError> {
        let mut state = self.state.lock().await;
        let claimed_job = state.claimed_jobs.remove(&input.job_id).ok_or_else(|| {
            PortError::new(format!("Claimed job not found: jobId={}", input.job_id))
        })?;

        if claimed_job.retry_count() >= input.max_retry {
            state.dead_letter_jobs.push(DeadLetterJob {
                job: claimed_job.clone(),
                reason: input.reason,
                failed_at: current_timestamp(),
            });

            return Ok(JobFailResult {
                status: JobFailStatus::DeadLetter,
                job: claimed_job,
            });
        }

        let retried_job = claimed_job.with_retry_count(claimed_job.retry_count() + 1);
        state.delayed_jobs.push(DelayedJob {
            available_at: Instant::now()
                + Duration::from_millis(compute_backoff_ms(&input, &retried_job)),
            job: retried_job.clone(),
        });

        Ok(JobFailResult {
            status: JobFailStatus::Requeued,
            job: retried_job,
        })
    }

    async fn get_depth(&self) -> Result<usize, PortError> {
        let state = self.state.lock().await;
        Ok(state.pending_jobs.len() + state.delayed_jobs.len())
    }
}

fn compute_backoff_ms<TJob>(input: &FailJobInput, job: &TJob) -> u64
where
    TJob: QueueJob,
{
    let exponent = job.retry_count().saturating_sub(1);
    let multiplier = 2_u64.saturating_pow(exponent);
    input.backoff_ms.saturating_mul(multiplier)
}

fn current_timestamp() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    elapsed.as_millis().to_string()
}

#[cfg(test)]
mod tests {
    use super::InMemoryJobQueue;
    use reili_core::queue::{
        CompleteJobInput, FailJobInput, JobFailStatus, JobQueuePort, QueueJob,
    };
    use tokio::time::{Duration, advance, pause};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestJob {
        job_id: String,
        retry_count: u32,
    }

    impl QueueJob for TestJob {
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

    #[tokio::test]
    async fn enqueue_claim_complete_and_get_depth_work() {
        let queue = InMemoryJobQueue::new();
        let job = test_job("job-1", 0);

        queue.enqueue(job.clone()).await.expect("enqueue");
        assert_eq!(queue.get_depth().await.expect("depth"), 1);

        let claimed = queue
            .claim()
            .await
            .expect("claim result")
            .expect("claimed job");
        assert_eq!(claimed, job);
        assert_eq!(queue.get_depth().await.expect("depth"), 0);

        queue
            .complete(CompleteJobInput {
                job_id: claimed.job_id.clone(),
            })
            .await
            .expect("complete");

        assert!(queue.claim().await.expect("claim").is_none());
    }

    #[tokio::test]
    async fn fail_requeues_job_with_exponential_backoff() {
        pause();
        let queue = InMemoryJobQueue::new();

        queue.enqueue(test_job("job-1", 0)).await.expect("enqueue");
        let claimed = queue
            .claim()
            .await
            .expect("claim result")
            .expect("claimed job");

        let first_fail = queue
            .fail(FailJobInput {
                job_id: claimed.job_id,
                reason: "temporary issue".to_string(),
                max_retry: 3,
                backoff_ms: 1_000,
            })
            .await
            .expect("fail");

        assert_eq!(first_fail.status, JobFailStatus::Requeued);
        assert_eq!(first_fail.job.retry_count, 1);
        assert_eq!(queue.get_depth().await.expect("depth"), 1);
        assert!(queue.claim().await.expect("claim").is_none());

        advance(Duration::from_millis(999)).await;
        assert!(queue.claim().await.expect("claim").is_none());

        advance(Duration::from_millis(1)).await;
        let retry_one = queue
            .claim()
            .await
            .expect("claim result")
            .expect("claimed job");
        assert_eq!(retry_one.retry_count, 1);

        let second_fail = queue
            .fail(FailJobInput {
                job_id: retry_one.job_id,
                reason: "still temporary".to_string(),
                max_retry: 3,
                backoff_ms: 1_000,
            })
            .await
            .expect("fail");

        assert_eq!(second_fail.status, JobFailStatus::Requeued);
        assert_eq!(second_fail.job.retry_count, 2);
        assert!(queue.claim().await.expect("claim").is_none());

        advance(Duration::from_millis(1_999)).await;
        assert!(queue.claim().await.expect("claim").is_none());

        advance(Duration::from_millis(1)).await;
        let retry_two = queue
            .claim()
            .await
            .expect("claim result")
            .expect("claimed job");
        assert_eq!(retry_two.retry_count, 2);
    }

    #[tokio::test]
    async fn fail_moves_job_to_dead_letter_when_retry_limit_reached() {
        let queue = InMemoryJobQueue::new();

        queue.enqueue(test_job("job-1", 2)).await.expect("enqueue");
        let claimed = queue
            .claim()
            .await
            .expect("claim result")
            .expect("claimed job");

        let fail_result = queue
            .fail(FailJobInput {
                job_id: claimed.job_id,
                reason: "fatal".to_string(),
                max_retry: 2,
                backoff_ms: 1_000,
            })
            .await
            .expect("fail");

        assert_eq!(fail_result.status, JobFailStatus::DeadLetter);
        assert_eq!(fail_result.job.retry_count, 2);
        assert_eq!(queue.get_depth().await.expect("depth"), 0);

        let state = queue.state.lock().await;
        assert_eq!(state.dead_letter_jobs.len(), 1);
        assert_eq!(state.dead_letter_jobs[0].reason, "fatal");
        assert_eq!(state.dead_letter_jobs[0].job.job_id, "job-1");
        assert!(!state.dead_letter_jobs[0].failed_at.is_empty());
    }

    #[tokio::test]
    async fn fail_returns_error_when_claimed_job_is_missing() {
        let queue: InMemoryJobQueue<TestJob> = InMemoryJobQueue::new();

        let error = queue
            .fail(FailJobInput {
                job_id: "missing".to_string(),
                reason: "failed".to_string(),
                max_retry: 2,
                backoff_ms: 1_000,
            })
            .await
            .expect_err("expected missing claimed job error");

        assert_eq!(error.message, "Claimed job not found: jobId=missing");
    }

    fn test_job(job_id: &str, retry_count: u32) -> TestJob {
        TestJob {
            job_id: job_id.to_string(),
            retry_count,
        }
    }
}
