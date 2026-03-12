use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use sre_adapters::observability::logger::init_json_logger;
use sre_adapters::queue::InMemoryJobQueue;
use sre_application::investigation::{InvestigationExecutionDeps, InvestigationLogger};
use sre_application::start_investigation_worker_runner::StartInvestigationWorkerRunnerUseCase;
use sre_application::start_investigation_worker_runner::StartInvestigationWorkerRunnerUseCaseDeps;
use sre_shared::ports::outbound::InvestigationJobQueuePort;
use sre_shared::types::InvestigationJob;

use crate::bootstrap::{RuntimeBootstrapError, build_worker_runtime_deps};
use crate::config::env::{EnvConfigError, load_worker_config};

const WORKER_INTERNAL_JOB_PATH: &str = "/internal/jobs";

#[derive(Debug, thiserror::Error)]
pub enum WorkerRunError {
    #[error("{0}")]
    Config(#[from] EnvConfigError),
    #[error("{0}")]
    Bootstrap(#[from] RuntimeBootstrapError),
    #[error("Failed to initialize logger: {0}")]
    Logger(#[from] tracing::subscriber::SetGlobalDefaultError),
    #[error("Failed to bind worker server: {0}")]
    Bind(std::io::Error),
    #[error("Worker server failed: {0}")]
    Serve(std::io::Error),
}

#[derive(Clone)]
struct WorkerHttpState {
    job_queue: Arc<InvestigationJobQueuePort>,
    logger: Arc<dyn InvestigationLogger>,
}

#[derive(Clone)]
struct WorkerAuthState {
    worker_internal_token: String,
}

pub async fn run_worker() -> Result<(), WorkerRunError> {
    init_json_logger()?;

    let config = load_worker_config()?;
    let deps = build_worker_runtime_deps(&config)?;
    let job_queue: Arc<InvestigationJobQueuePort> =
        Arc::new(InMemoryJobQueue::<InvestigationJob>::new());
    let worker_runner =
        StartInvestigationWorkerRunnerUseCase::new(StartInvestigationWorkerRunnerUseCaseDeps {
            job_queue: Arc::clone(&job_queue),
            investigation_execution_deps: InvestigationExecutionDeps {
                slack_reply_port: Arc::clone(&deps.slack_reply_port),
                slack_progress_stream_port: Arc::clone(&deps.slack_progress_stream_port),
                slack_thread_history_port: Arc::clone(&deps.slack_thread_history_port),
                investigation_resources: deps.investigation_resources,
                coordinator_runner: Arc::clone(&deps.coordinator_runner),
                synthesizer_runner: Arc::clone(&deps.synthesizer_runner),
                logger: Arc::clone(&deps.logger),
            },
            worker_concurrency: config.worker_concurrency,
            job_max_retry: config.job_max_retry,
            job_backoff_ms: config.job_backoff_ms,
        });
    worker_runner.start();

    let auth_state = Arc::new(WorkerAuthState {
        worker_internal_token: config.worker_internal_token.clone(),
    });
    let http_state = Arc::new(WorkerHttpState {
        job_queue,
        logger: Arc::clone(&deps.logger),
    });
    let app = Router::new()
        .route(
            WORKER_INTERNAL_JOB_PATH,
            post(enqueue_worker_internal_job).route_layer(middleware::from_fn_with_state(
                auth_state,
                verify_worker_internal_auth_middleware,
            )),
        )
        .fallback(not_found_handler)
        .with_state(http_state);

    let listener =
        tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.worker_internal_port))
            .await
            .map_err(WorkerRunError::Bind)?;
    tracing::info!(
        worker_internal_port = config.worker_internal_port,
        worker_concurrency = config.worker_concurrency,
        internal_api_path = WORKER_INTERNAL_JOB_PATH,
        "Worker app is running"
    );

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(WorkerRunError::Serve);

    worker_runner.stop();

    serve_result
}

async fn not_found_handler() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

async fn enqueue_worker_internal_job(
    State(state): State<Arc<WorkerHttpState>>,
    body: Bytes,
) -> impl IntoResponse {
    let job = match serde_json::from_slice::<InvestigationJob>(&body) {
        Ok(value) => value,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    if let Err(error) = state.job_queue.enqueue(job.clone()).await {
        state.logger.error(
            "Failed to enqueue investigation job",
            BTreeMap::from([("error".to_string(), error.message)]),
        );
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    let queue_depth = match state.job_queue.get_depth().await {
        Ok(value) => value,
        Err(error) => {
            state.logger.error(
                "Failed to read worker queue depth",
                BTreeMap::from([("error".to_string(), error.message)]),
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    state.logger.info(
        "Queued investigation job",
        BTreeMap::from([
            ("jobId".to_string(), job.job_id.clone()),
            (
                "slackEventId".to_string(),
                job.payload.slack_event_id.clone(),
            ),
            ("channel".to_string(), job.payload.message.channel.clone()),
            (
                "threadTs".to_string(),
                job.payload.message.thread_ts_or_ts().to_string(),
            ),
            ("worker_queue_depth".to_string(), queue_depth.to_string()),
        ]),
    );

    StatusCode::ACCEPTED
}

async fn verify_worker_internal_auth_middleware(
    State(state): State<Arc<WorkerAuthState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let bearer_token = read_bearer_token(request.headers());
    if bearer_token != Some(state.worker_internal_token.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

fn read_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let authorization = headers.get(axum::http::header::AUTHORIZATION)?;
    let header_text = authorization.to_str().ok()?.trim();
    let token = header_text.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }

    Some(token)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::read_bearer_token;

    #[test]
    fn reads_bearer_token_from_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token-1"),
        );

        assert_eq!(read_bearer_token(&headers), Some("token-1"));
    }

    #[test]
    fn returns_none_for_invalid_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic token-1"),
        );

        assert_eq!(read_bearer_token(&headers), None);
    }
}
