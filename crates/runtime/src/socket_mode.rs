use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use reili_adapters::inbound::slack::{
    ParsedSlackEvent, ParsedSlackInteraction, parse_slack_event, parse_slack_interaction_value,
};
use reili_application::{TaskLogger, string_log_meta};
use reili_core::messaging::slack::{SlackInteractionHandlerPort, SlackMessageHandlerPort};
use reili_core::secret::SecretString;
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = SplitSink<WsStream, Message>;

const CLIENT_PING_INTERVAL: Duration = Duration::from_secs(10);
const CLIENT_PONG_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SocketHealthConfig {
    ping_interval: Duration,
    pong_timeout: Duration,
}

impl Default for SocketHealthConfig {
    fn default() -> Self {
        Self {
            ping_interval: CLIENT_PING_INTERVAL,
            pong_timeout: CLIENT_PONG_TIMEOUT,
        }
    }
}

#[derive(Debug, Default)]
struct SocketHealthState {
    awaiting_pong_deadline: Option<tokio::time::Instant>,
}

impl SocketHealthState {
    fn on_ping_sent(&mut self, now: tokio::time::Instant, pong_timeout: Duration) {
        self.awaiting_pong_deadline = Some(now + pong_timeout);
    }

    fn on_pong_received(&mut self) {
        self.awaiting_pong_deadline = None;
    }

    fn awaiting_pong_deadline(&self) -> Option<tokio::time::Instant> {
        self.awaiting_pong_deadline
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SocketModeError {
    #[error("apps.connections.open failed: {0}")]
    ConnectionsOpen(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Socket Mode link disabled by Slack")]
    LinkDisabled,
}

pub struct SocketModeConfig {
    pub app_token: SecretString,
    pub bot_user_id: String,
    pub slack_message_handler: Arc<dyn SlackMessageHandlerPort>,
    pub slack_interaction_handler: Arc<dyn SlackInteractionHandlerPort>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct SocketModeClient {
    config: SocketModeConfig,
}

impl SocketModeClient {
    pub fn new(config: SocketModeConfig) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), SocketModeError> {
        let mut consecutive_failures: u32 = 0;

        loop {
            if *shutdown.borrow() {
                break;
            }

            if consecutive_failures > 0 {
                let delay = Duration::from_secs(2u64.saturating_pow(consecutive_failures.min(5)));
                self.config.logger.warn(
                    "Reconnecting after backoff",
                    string_log_meta([
                        ("delay_secs", delay.as_secs().to_string()),
                        ("consecutive_failures", consecutive_failures.to_string()),
                    ]),
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => { break; }
                }
            }

            let wss_url = match self.request_wss_url().await {
                Ok(url) => {
                    consecutive_failures = 0;
                    url
                }
                Err(error) => {
                    consecutive_failures += 1;
                    self.config.logger.error(
                        "Failed to obtain WSS URL",
                        string_log_meta([("error", error.to_string())]),
                    );
                    continue;
                }
            };

            match self.run_connection(&wss_url, &mut shutdown).await {
                Ok(DisconnectReason::SlackRefresh) => {
                    consecutive_failures = 0;
                }
                Ok(DisconnectReason::LinkDisabled) => {
                    return Err(SocketModeError::LinkDisabled);
                }
                Ok(DisconnectReason::PongTimeout) => {
                    consecutive_failures += 1;
                    let health_config = SocketHealthConfig::default();
                    self.config.logger.warn(
                        "Socket Mode connection timed out waiting for pong",
                        string_log_meta([
                            (
                                "ping_interval_secs",
                                health_config.ping_interval.as_secs().to_string(),
                            ),
                            (
                                "pong_timeout_secs",
                                health_config.pong_timeout.as_secs().to_string(),
                            ),
                            ("consecutive_failures", consecutive_failures.to_string()),
                        ]),
                    );
                }
                Ok(DisconnectReason::Shutdown) => {
                    break;
                }
                Err(error) => {
                    consecutive_failures += 1;
                    self.config.logger.error(
                        "WebSocket connection error",
                        string_log_meta([("error", error.to_string())]),
                    );
                }
            }
        }

        Ok(())
    }

    async fn run_connection(
        &self,
        wss_url: &str,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<DisconnectReason, SocketModeError> {
        let (ws_stream, _) = connect_async(wss_url).await?;
        let (mut ws_sink, mut ws_recv) = ws_stream.split();

        let health_config = SocketHealthConfig::default();
        let mut health_state = SocketHealthState::default();
        let mut ping_ticker = tokio::time::interval(health_config.ping_interval);

        loop {
            let pong_deadline = health_state.awaiting_pong_deadline();

            tokio::select! {
                msg = ws_recv.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(reason) = self.handle_text_message(&text, &mut ws_sink).await? {
                                return Ok(reason);
                            }
                        }
                        Some(Ok(Message::Ping(_))) => {
                            // tokio-tungstenite auto-responds with Pong
                        }
                        Some(Ok(Message::Pong(_))) => {
                            health_state.on_pong_received();
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            return Ok(DisconnectReason::SlackRefresh);
                        }
                        Some(Err(error)) => {
                            return Err(SocketModeError::WebSocket(error));
                        }
                        _ => {}
                    }
                }
                _ = ping_ticker.tick() => {
                    ws_sink.send(Message::Ping(vec![].into())).await?;
                    health_state.on_ping_sent(tokio::time::Instant::now(), health_config.pong_timeout);
                }
                _ = wait_for_pong_timeout(pong_deadline) => {
                    return Ok(DisconnectReason::PongTimeout);
                }
                _ = shutdown.changed() => {
                    let _ = ws_sink.send(Message::Close(None)).await;
                    return Ok(DisconnectReason::Shutdown);
                }
            }
        }
    }

    async fn handle_text_message(
        &self,
        text: &str,
        ws_sink: &mut WsSink,
    ) -> Result<Option<DisconnectReason>, SocketModeError> {
        let envelope: SocketModeEnvelope = serde_json::from_str(text)?;

        match envelope.envelope_type.as_str() {
            "hello" => {
                self.config
                    .logger
                    .info("Socket Mode connection established", BTreeMap::new());
                Ok(None)
            }
            "disconnect" => Ok(Some(parse_disconnect_reason(text))),
            "events_api" => {
                if let Some(envelope_id) = &envelope.envelope_id {
                    let ack = json!({"envelope_id": envelope_id});
                    ws_sink.send(Message::Text(ack.to_string().into())).await?;
                }

                if let Some(payload) = envelope.payload {
                    let payload_bytes = serde_json::to_vec(&payload)?;
                    let parsed = parse_slack_event(&payload_bytes, &self.config.bot_user_id);
                    match parsed {
                        Ok(ParsedSlackEvent::Message(message)) => {
                            let handler = Arc::clone(&self.config.slack_message_handler);
                            let logger = Arc::clone(&self.config.logger);
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle(message).await {
                                    logger.error(
                                        "Failed to handle Socket Mode message event",
                                        string_log_meta([("error", e.message)]),
                                    );
                                }
                            });
                        }
                        Ok(ParsedSlackEvent::UrlVerification { .. }) => {
                            // Does not occur in Socket Mode
                        }
                        Ok(ParsedSlackEvent::Ignored) => {}
                        Err(e) => {
                            self.config.logger.warn(
                                "Failed to parse Socket Mode event payload",
                                string_log_meta([("error", e.message)]),
                            );
                        }
                    }
                }
                Ok(None)
            }
            "interactive" => {
                if let Some(envelope_id) = &envelope.envelope_id {
                    let ack = json!({"envelope_id": envelope_id});
                    ws_sink.send(Message::Text(ack.to_string().into())).await?;
                }

                if let Some(payload) = envelope.payload {
                    match parse_slack_interaction_value(payload) {
                        Ok(ParsedSlackInteraction::Interaction(interaction)) => {
                            let handler = Arc::clone(&self.config.slack_interaction_handler);
                            let logger = Arc::clone(&self.config.logger);
                            tokio::spawn(async move {
                                if let Err(error) = handler.handle(interaction).await {
                                    logger.error(
                                        "Failed to handle Socket Mode interaction",
                                        string_log_meta([("error", error.message)]),
                                    );
                                }
                            });
                        }
                        Ok(ParsedSlackInteraction::Ignored) => {}
                        Err(error) => {
                            self.config.logger.warn(
                                "Failed to parse Socket Mode interaction payload",
                                string_log_meta([("error", error.message)]),
                            );
                        }
                    }
                }
                Ok(None)
            }
            _ => {
                if let Some(envelope_id) = &envelope.envelope_id {
                    let ack = json!({"envelope_id": envelope_id});
                    ws_sink.send(Message::Text(ack.to_string().into())).await?;
                }
                Ok(None)
            }
        }
    }

    async fn request_wss_url(&self) -> Result<String, SocketModeError> {
        let client = reqwest::Client::new();
        let response = client
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(self.config.app_token.expose())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;

        #[derive(Deserialize)]
        struct ConnectionsOpenResponse {
            ok: bool,
            url: Option<String>,
            error: Option<String>,
        }

        let body: ConnectionsOpenResponse = response.json().await?;
        match (body.ok, body.url) {
            (true, Some(url)) => Ok(url),
            _ => Err(SocketModeError::ConnectionsOpen(
                body.error.unwrap_or_else(|| "unknown error".to_string()),
            )),
        }
    }
}

async fn wait_for_pong_timeout(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

#[derive(Debug, Deserialize)]
struct SocketModeEnvelope {
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    envelope_type: String,
    payload: Option<serde_json::Value>,
    #[allow(dead_code)]
    retry_attempt: Option<u32>,
    #[allow(dead_code)]
    retry_reason: Option<String>,
}

enum DisconnectReason {
    SlackRefresh,
    LinkDisabled,
    PongTimeout,
    Shutdown,
}

#[derive(Deserialize)]
struct DisconnectPayload {
    reason: Option<String>,
}

fn parse_disconnect_reason(text: &str) -> DisconnectReason {
    let payload: Result<DisconnectPayload, _> = serde_json::from_str(text);
    match payload.ok().and_then(|p| p.reason) {
        Some(reason) if reason == "link_disabled" => DisconnectReason::LinkDisabled,
        _ => DisconnectReason::SlackRefresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_state_records_deadline_when_ping_is_sent() {
        let now = tokio::time::Instant::now();
        let mut health_state = SocketHealthState::default();

        health_state.on_ping_sent(now, Duration::from_secs(5));

        assert_eq!(
            health_state.awaiting_pong_deadline(),
            Some(now + Duration::from_secs(5))
        );
    }

    #[test]
    fn health_state_clears_deadline_when_pong_is_received() {
        let now = tokio::time::Instant::now();
        let mut health_state = SocketHealthState::default();
        health_state.on_ping_sent(now, Duration::from_secs(5));

        health_state.on_pong_received();

        assert_eq!(health_state.awaiting_pong_deadline(), None);
    }

    #[tokio::test]
    async fn wait_for_pong_timeout_completes_when_deadline_exists() {
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_pong_timeout(Some(tokio::time::Instant::now())),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_pong_timeout_stays_pending_without_deadline() {
        let result =
            tokio::time::timeout(Duration::from_millis(20), wait_for_pong_timeout(None)).await;

        assert!(result.is_err());
    }

    #[test]
    fn parses_hello_envelope() {
        let json = r#"{"type":"hello","connection_info":{},"debug_info":{}}"#;
        let envelope: SocketModeEnvelope = serde_json::from_str(json).expect("parse hello");

        assert_eq!(envelope.envelope_type, "hello");
        assert!(envelope.envelope_id.is_none());
        assert!(envelope.payload.is_none());
    }

    #[test]
    fn parses_events_api_envelope() {
        let json = r#"{
            "envelope_id": "env-id-123",
            "type": "events_api",
            "payload": {"event": {"type": "message", "text": "hello"}},
            "retry_attempt": 0,
            "retry_reason": null
        }"#;
        let envelope: SocketModeEnvelope = serde_json::from_str(json).expect("parse events_api");

        assert_eq!(envelope.envelope_type, "events_api");
        assert_eq!(envelope.envelope_id.as_deref(), Some("env-id-123"));
        assert!(envelope.payload.is_some());
        assert_eq!(envelope.retry_attempt, Some(0));
    }

    #[test]
    fn parses_disconnect_envelope() {
        let json = r#"{"type":"disconnect","reason":"refresh_requested"}"#;
        let envelope: SocketModeEnvelope = serde_json::from_str(json).expect("parse disconnect");

        assert_eq!(envelope.envelope_type, "disconnect");
        assert!(envelope.envelope_id.is_none());
    }

    #[test]
    fn disconnect_reason_link_disabled() {
        let json = r#"{"type":"disconnect","reason":"link_disabled"}"#;
        let reason = parse_disconnect_reason(json);

        assert!(matches!(reason, DisconnectReason::LinkDisabled));
    }

    #[test]
    fn disconnect_reason_refresh_requested() {
        let json = r#"{"type":"disconnect","reason":"refresh_requested"}"#;
        let reason = parse_disconnect_reason(json);

        assert!(matches!(reason, DisconnectReason::SlackRefresh));
    }

    #[test]
    fn disconnect_reason_warning() {
        let json = r#"{"type":"disconnect","reason":"warning"}"#;
        let reason = parse_disconnect_reason(json);

        assert!(matches!(reason, DisconnectReason::SlackRefresh));
    }

    #[test]
    fn disconnect_reason_unknown_defaults_to_refresh() {
        let json = r#"{"type":"disconnect"}"#;
        let reason = parse_disconnect_reason(json);

        assert!(matches!(reason, DisconnectReason::SlackRefresh));
    }

    #[test]
    fn ack_message_format() {
        let envelope_id = "abc-123";
        let ack = json!({"envelope_id": envelope_id});

        assert_eq!(ack.to_string(), r#"{"envelope_id":"abc-123"}"#);
    }

    #[test]
    fn parses_unknown_type_with_envelope_id() {
        let json = r#"{"envelope_id":"env-456","type":"slash_commands","payload":{}}"#;
        let envelope: SocketModeEnvelope = serde_json::from_str(json).expect("parse unknown type");

        assert_eq!(envelope.envelope_type, "slash_commands");
        assert_eq!(envelope.envelope_id.as_deref(), Some("env-456"));
    }
}
