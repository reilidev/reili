use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use reili_adapters::inbound::slack::{
    ParsedSlackEvent, ParsedSlackInteraction, SlackInteractionForm, parse_slack_event,
    verify_slack_signature_middleware,
};
use reili_adapters::logger::init_json_logger;
use reili_application::task::{TaskLogger, string_log_meta};
use reili_core::messaging::slack::{SlackInteractionHandlerPort, SlackMessageHandlerPort};
use serde_json::json;

use crate::bootstrap::{RuntimeBootstrapError, build_runtime_deps};
use crate::config::env::{EnvConfigError, SlackConnectionMode, load_app_config};
use crate::socket_mode::{SocketModeClient, SocketModeConfig, SocketModeError};

#[derive(Debug, thiserror::Error)]
pub enum AppRunError {
    #[error("{0}")]
    Config(#[from] EnvConfigError),
    #[error("{0}")]
    Bootstrap(#[from] RuntimeBootstrapError),
    #[error("Failed to initialize logger: {0}")]
    Logger(#[from] tracing::subscriber::SetGlobalDefaultError),
    #[error("Failed to bind app server: {0}")]
    Bind(std::io::Error),
    #[error("App server failed: {0}")]
    Serve(std::io::Error),
    #[error("Socket Mode failed: {0}")]
    SocketMode(#[from] SocketModeError),
    #[error("Missing SLACK_SIGNING_SECRET for HTTP mode")]
    MissingSigningSecret,
}

#[derive(Clone)]
struct AppHttpState {
    slack_message_handler: Arc<dyn SlackMessageHandlerPort>,
    slack_interaction_handler: Arc<dyn SlackInteractionHandlerPort>,
    bot_user_id: String,
    logger: Arc<dyn TaskLogger>,
}

pub async fn run_app() -> Result<(), AppRunError> {
    init_json_logger()?;

    let config = load_app_config()?;
    let deps = build_runtime_deps(&config).await?;
    deps.worker_runner.start();

    let serve_result = match &config.slack_connection_mode {
        SlackConnectionMode::Http => run_http_server(&config, &deps).await,
        SlackConnectionMode::SocketMode { app_token } => {
            run_socket_mode(app_token.expose(), &deps).await
        }
    };

    deps.worker_runner.stop();
    serve_result
}

async fn run_http_server(
    config: &crate::config::env::AppConfig,
    deps: &crate::bootstrap::RuntimeDeps,
) -> Result<(), AppRunError> {
    let slack_signature_verifier = deps
        .slack_signature_verifier
        .clone()
        .ok_or(AppRunError::MissingSigningSecret)?;

    let http_state = Arc::new(AppHttpState {
        slack_message_handler: Arc::clone(&deps.slack_message_handler),
        slack_interaction_handler: Arc::clone(&deps.slack_interaction_handler),
        bot_user_id: deps.bot_user_id.clone(),
        logger: Arc::clone(&deps.logger),
    });
    let app = Router::new()
        .route("/slack/events", post(handle_slack_events))
        .route("/slack/interactions", post(handle_slack_interactions))
        .route_layer(middleware::from_fn_with_state(
            slack_signature_verifier,
            verify_slack_signature_middleware,
        ))
        .route("/healthz", get(handle_healthz))
        .with_state(http_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port))
        .await
        .map_err(AppRunError::Bind)?;
    tracing::info!(
        port = config.port,
        worker_concurrency = config.worker_concurrency,
        "App is running"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(AppRunError::Serve)
}

async fn run_socket_mode(
    app_token: &str,
    deps: &crate::bootstrap::RuntimeDeps,
) -> Result<(), AppRunError> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let client = SocketModeClient::new(SocketModeConfig {
        app_token: app_token.to_string(),
        bot_user_id: deps.bot_user_id.clone(),
        slack_message_handler: Arc::clone(&deps.slack_message_handler),
        slack_interaction_handler: Arc::clone(&deps.slack_interaction_handler),
        logger: Arc::clone(&deps.logger),
    });

    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    client
        .run(shutdown_rx)
        .await
        .map_err(AppRunError::SocketMode)
}

async fn handle_slack_events(State(state): State<Arc<AppHttpState>>, body: Bytes) -> Response {
    let parsed = match parse_slack_event(&body, &state.bot_user_id) {
        Ok(value) => value,
        Err(error) => {
            state.logger.warn(
                "Failed to parse Slack event payload",
                string_log_meta([("error", error.message)]),
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
                    string_log_meta([("error", error.message)]),
                );
            }

            StatusCode::OK.into_response()
        }
        ParsedSlackEvent::Ignored => StatusCode::OK.into_response(),
    }
}

async fn handle_slack_interactions(
    State(state): State<Arc<AppHttpState>>,
    form: Result<Form<SlackInteractionForm>, FormRejection>,
) -> Response {
    let Form(form) = match form {
        Ok(form) => form,
        Err(error) => {
            state.logger.warn(
                "Failed to parse Slack interaction payload",
                string_log_meta([("error", error.body_text())]),
            );
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    match form.payload {
        ParsedSlackInteraction::Interaction(interaction) => {
            let handler = Arc::clone(&state.slack_interaction_handler);
            let logger = Arc::clone(&state.logger);
            tokio::spawn(async move {
                if let Err(error) = handler.handle(interaction).await {
                    logger.error(
                        "Failed to handle Slack interaction",
                        string_log_meta([("error", error.message)]),
                    );
                }
            });
            StatusCode::OK.into_response()
        }
        ParsedSlackInteraction::Ignored => StatusCode::OK.into_response(),
    }
}

async fn handle_healthz() -> StatusCode {
    StatusCode::OK
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
    use axum::http::StatusCode;

    use super::handle_healthz;

    #[tokio::test]
    async fn healthz_returns_ok() {
        assert_eq!(handle_healthz().await, StatusCode::OK);
    }
}
