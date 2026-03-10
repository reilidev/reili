use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;
use sre_adapters::inbound::slack::{
    ParsedSlackEvent, parse_slack_event, verify_slack_signature_middleware,
};
use sre_adapters::observability::logger::init_json_logger;
use sre_application::investigation::InvestigationLogger;

use crate::bootstrap::{RuntimeBootstrapError, build_ingress_runtime_deps};
use crate::config::env::{EnvConfigError, load_ingress_config};

const INGRESS_EVENTS_ENDPOINT: &str = "/slack/events";

#[derive(Debug, thiserror::Error)]
pub enum IngressRunError {
    #[error("{0}")]
    Config(#[from] EnvConfigError),
    #[error("{0}")]
    Bootstrap(#[from] RuntimeBootstrapError),
    #[error("Failed to initialize logger: {0}")]
    Logger(#[from] tracing::subscriber::SetGlobalDefaultError),
    #[error("Failed to bind ingress server: {0}")]
    Bind(std::io::Error),
    #[error("Ingress server failed: {0}")]
    Serve(std::io::Error),
}

#[derive(Clone)]
struct IngressHttpState {
    slack_message_handler: Arc<dyn sre_shared::ports::inbound::SlackMessageHandlerPort>,
    bot_user_id: String,
    logger: Arc<dyn InvestigationLogger>,
}

pub async fn run_ingress() -> Result<(), IngressRunError> {
    init_json_logger()?;

    let config = load_ingress_config()?;
    let deps = build_ingress_runtime_deps(&config).await?;
    let http_state = Arc::new(IngressHttpState {
        slack_message_handler: deps.slack_message_handler,
        bot_user_id: deps.bot_user_id,
        logger: deps.logger,
    });

    let app = Router::new()
        .route(INGRESS_EVENTS_ENDPOINT, post(handle_slack_events))
        .route_layer(middleware::from_fn_with_state(
            deps.slack_signature_verifier,
            verify_slack_signature_middleware,
        ))
        .with_state(http_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port))
        .await
        .map_err(IngressRunError::Bind)?;
    tracing::info!(
        port = config.port,
        events_endpoint = INGRESS_EVENTS_ENDPOINT,
        worker_base_url = config.worker_base_url,
        "Ingress app is running"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(IngressRunError::Serve)
}

async fn handle_slack_events(State(state): State<Arc<IngressHttpState>>, body: Bytes) -> Response {
    let parsed = match parse_slack_event(&body, &state.bot_user_id) {
        Ok(value) => value,
        Err(error) => {
            state.logger.warn(
                "Failed to parse Slack event payload",
                BTreeMap::from([("error".to_string(), error.message)]),
            );
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    match parsed {
        ParsedSlackEvent::UrlVerification { challenge } => {
            (StatusCode::OK, Json(json!({ "challenge": challenge }))).into_response()
        }
        ParsedSlackEvent::Message(message) => {
            if let Err(error) = state.slack_message_handler.handle(message).await {
                state.logger.error(
                    "Failed to handle Slack message event",
                    BTreeMap::from([("error".to_string(), error.message)]),
                );
            }

            StatusCode::OK.into_response()
        }
        ParsedSlackEvent::Ignored => StatusCode::OK.into_response(),
    }
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
