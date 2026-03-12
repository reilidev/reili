use std::time::Duration;

use async_trait::async_trait;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::WorkerJobDispatcherPort;
use sre_shared::types::InvestigationJob;

const DEFAULT_TIMEOUT_MS: u64 = 3_000;
const WORKER_INTERNAL_JOBS_PATH: &str = "/internal/jobs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpWorkerJobDispatcherConfig {
    pub worker_base_url: String,
    pub worker_internal_token: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct HttpWorkerJobDispatcher {
    client: reqwest::Client,
    endpoint: String,
    worker_internal_token: String,
    timeout: Duration,
}

impl HttpWorkerJobDispatcher {
    pub fn new(config: HttpWorkerJobDispatcherConfig) -> Result<Self, PortError> {
        let endpoint = build_worker_endpoint(BuildWorkerEndpointInput {
            worker_base_url: config.worker_base_url,
        })?;

        let worker_internal_token = config.worker_internal_token.trim().to_string();
        if worker_internal_token.is_empty() {
            return Err(PortError::new("Worker internal token must not be empty"));
        }

        let timeout_ms = if config.timeout_ms == 0 {
            DEFAULT_TIMEOUT_MS
        } else {
            config.timeout_ms
        };
        let timeout = Duration::from_millis(timeout_ms);
        let client = reqwest::Client::builder().build().map_err(|error| {
            PortError::new(format!("Failed to build worker HTTP client: {error}"))
        })?;

        Ok(Self {
            client,
            endpoint,
            worker_internal_token,
            timeout,
        })
    }
}

#[async_trait]
impl WorkerJobDispatcherPort for HttpWorkerJobDispatcher {
    async fn dispatch(&self, job: InvestigationJob) -> Result<(), PortError> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.worker_internal_token)
            .timeout(self.timeout)
            .json(&job)
            .send()
            .await
            .map_err(|error| {
                PortError::new(format!("Failed to dispatch investigation job: {error}"))
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        Err(PortError::new(format!(
            "Failed to dispatch investigation job: status={}",
            response.status().as_u16()
        )))
    }
}

struct BuildWorkerEndpointInput {
    worker_base_url: String,
}

fn build_worker_endpoint(input: BuildWorkerEndpointInput) -> Result<String, PortError> {
    let normalized_base_url = input
        .worker_base_url
        .trim()
        .trim_end_matches('/')
        .to_string();
    if normalized_base_url.is_empty() {
        return Err(PortError::new("Worker base URL must not be empty"));
    }

    Ok(format!("{normalized_base_url}{WORKER_INTERNAL_JOBS_PATH}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sre_shared::ports::outbound::WorkerJobDispatcherPort;
    use sre_shared::types::{
        InvestigationJob, InvestigationJobPayload, SlackMessage, SlackTriggerType,
    };
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{HttpWorkerJobDispatcher, HttpWorkerJobDispatcherConfig};

    #[tokio::test]
    async fn dispatch_posts_job_to_internal_endpoint() {
        let server = MockServer::start().await;
        let job = sample_job();
        Mock::given(method("POST"))
            .and(path("/internal/jobs"))
            .and(header("authorization", "Bearer internal-token"))
            .and(body_json(json!(job.clone())))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;

        let dispatcher = HttpWorkerJobDispatcher::new(HttpWorkerJobDispatcherConfig {
            worker_base_url: server.uri(),
            worker_internal_token: "internal-token".to_string(),
            timeout_ms: 3_000,
        })
        .expect("create dispatcher");
        dispatcher.dispatch(job).await.expect("dispatch job");
    }

    #[tokio::test]
    async fn dispatch_returns_error_for_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/internal/jobs"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let dispatcher = HttpWorkerJobDispatcher::new(HttpWorkerJobDispatcherConfig {
            worker_base_url: server.uri(),
            worker_internal_token: "internal-token".to_string(),
            timeout_ms: 3_000,
        })
        .expect("create dispatcher");
        let error = dispatcher
            .dispatch(sample_job())
            .await
            .expect_err("dispatch should fail");

        assert!(error.message.contains("status=500"));
    }

    fn sample_job() -> InvestigationJob {
        InvestigationJob {
            job_id: "job-1".to_string(),
            received_at: "2026-03-05T00:00:00.000Z".to_string(),
            payload: InvestigationJobPayload {
                slack_event_id: "evt-1".to_string(),
                message: SlackMessage {
                    slack_event_id: "evt-1".to_string(),
                    team_id: Some("T001".to_string()),
                    trigger: SlackTriggerType::AppMention,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    text: "investigate".to_string(),
                    ts: "1710000000.000001".to_string(),
                    thread_ts: Some("1710000000.000000".to_string()),
                },
            },
            retry_count: 0,
        }
    }
}
