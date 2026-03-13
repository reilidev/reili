use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use reili_shared::errors::PortError;
use ring::hmac;

const SLACK_SIGNATURE_HEADER: &str = "x-slack-signature";
const SLACK_REQUEST_TIMESTAMP_HEADER: &str = "x-slack-request-timestamp";
const DEFAULT_ALLOWED_TIMESTAMP_AGE_SECONDS: i64 = 5 * 60;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackSignatureVerifierConfig {
    pub signing_secret: String,
    pub allowed_timestamp_age_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct SlackSignatureVerifier {
    signing_secret: String,
    allowed_timestamp_age_seconds: i64,
}

impl SlackSignatureVerifier {
    pub fn new(signing_secret: String) -> Result<Self, PortError> {
        Self::new_with_config(SlackSignatureVerifierConfig {
            signing_secret,
            allowed_timestamp_age_seconds: DEFAULT_ALLOWED_TIMESTAMP_AGE_SECONDS,
        })
    }

    pub fn new_with_config(config: SlackSignatureVerifierConfig) -> Result<Self, PortError> {
        if config.signing_secret.trim().is_empty() {
            return Err(PortError::new("Slack signing secret must not be empty"));
        }
        if config.allowed_timestamp_age_seconds <= 0 {
            return Err(PortError::new(
                "Slack allowed timestamp age must be greater than zero",
            ));
        }

        Ok(Self {
            signing_secret: config.signing_secret,
            allowed_timestamp_age_seconds: config.allowed_timestamp_age_seconds,
        })
    }

    pub fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), PortError> {
        self.verify_with_timestamp(headers, body, current_unix_timestamp_seconds())
    }

    fn verify_with_timestamp(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now_unix_seconds: i64,
    ) -> Result<(), PortError> {
        let timestamp = read_required_header(headers, SLACK_REQUEST_TIMESTAMP_HEADER)?;
        let signature = read_required_header(headers, SLACK_SIGNATURE_HEADER)?;
        let signature_bytes = parse_signature_header(&signature)?;
        let request_unix_seconds = timestamp.parse::<i64>().map_err(|_| {
            PortError::new("Slack request timestamp header is not a valid unix timestamp")
        })?;

        let age_seconds = (now_unix_seconds - request_unix_seconds).abs();
        if age_seconds > self.allowed_timestamp_age_seconds {
            return Err(PortError::new(
                "Slack request timestamp is outside the allowed tolerance window",
            ));
        }

        let key = hmac::Key::new(hmac::HMAC_SHA256, self.signing_secret.as_bytes());
        let mut signed_payload = format!("v0:{timestamp}:").into_bytes();
        signed_payload.extend_from_slice(body);
        hmac::verify(&key, &signed_payload, &signature_bytes)
            .map_err(|_| PortError::new("Slack signature verification failed"))?;

        Ok(())
    }
}

pub async fn verify_slack_signature_middleware(
    State(verifier): State<Arc<SlackSignatureVerifier>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();
    let body_bytes = to_bytes(body, MAX_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    verifier
        .verify(&parts.headers, &body_bytes)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let request = Request::from_parts(parts, Body::from(body_bytes));
    Ok(next.run(request).await)
}

fn read_required_header(headers: &HeaderMap, name: &str) -> Result<String, PortError> {
    let header_value = headers
        .get(name)
        .ok_or_else(|| PortError::new(format!("Missing required Slack header: {name}")))?;
    let text = header_value
        .to_str()
        .map_err(|_| PortError::new(format!("Slack header is not valid UTF-8: {name}")))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(PortError::new(format!(
            "Slack header must not be empty: {name}"
        )));
    }

    Ok(text)
}

fn current_unix_timestamp_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64
}

fn parse_signature_header(signature: &str) -> Result<Vec<u8>, PortError> {
    let raw_signature = signature
        .strip_prefix("v0=")
        .ok_or_else(|| PortError::new("Slack signature header must start with v0="))?;

    decode_hex(raw_signature)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, PortError> {
    if !value.len().is_multiple_of(2) {
        return Err(PortError::new("Slack signature header is not valid hex"));
    }

    let mut decoded = Vec::with_capacity(value.len() / 2);
    let bytes = value.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        let high = decode_hex_nibble(bytes[index])?;
        let low = decode_hex_nibble(bytes[index + 1])?;
        decoded.push((high << 4) | low);
        index += 2;
    }

    Ok(decoded)
}

fn decode_hex_nibble(value: u8) -> Result<u8, PortError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(PortError::new("Slack signature header is not valid hex")),
    }
}

#[cfg(test)]
fn hex_encode_lowercase(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        output.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }

    output
}

#[cfg(test)]
impl SlackSignatureVerifier {
    fn compute_signature(&self, timestamp: &str, body: &[u8]) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.signing_secret.as_bytes());
        let mut signed_payload = format!("v0:{timestamp}:").into_bytes();
        signed_payload.extend_from_slice(body);
        let digest = hmac::sign(&key, &signed_payload);

        format!("v0={}", hex_encode_lowercase(digest.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::SlackSignatureVerifier;

    #[test]
    fn verifies_valid_signature() {
        let verifier =
            SlackSignatureVerifier::new("slack-signing-secret".to_string()).expect("verifier");
        let body = br#"{"type":"event_callback"}"#;
        let timestamp = "1710000000";
        let signature = verifier.compute_signature(timestamp, body);
        let headers = headers(timestamp, &signature);

        verifier
            .verify_with_timestamp(&headers, body, 1710000001)
            .expect("signature should be valid");
    }

    #[test]
    fn rejects_missing_required_headers() {
        let verifier =
            SlackSignatureVerifier::new("slack-signing-secret".to_string()).expect("verifier");
        let error = verifier
            .verify_with_timestamp(&HeaderMap::new(), br#"{}"#, 1710000001)
            .expect_err("missing header should fail");

        assert!(error.message.contains("Missing required Slack header"));
    }

    #[test]
    fn rejects_expired_timestamp() {
        let verifier =
            SlackSignatureVerifier::new("slack-signing-secret".to_string()).expect("verifier");
        let body = br#"{}"#;
        let timestamp = "1710000000";
        let signature = verifier.compute_signature(timestamp, body);
        let headers = headers(timestamp, &signature);

        let error = verifier
            .verify_with_timestamp(&headers, body, 1710009999)
            .expect_err("expired timestamp should fail");

        assert!(
            error
                .message
                .contains("outside the allowed tolerance window")
        );
    }

    #[test]
    fn rejects_invalid_signature() {
        let verifier =
            SlackSignatureVerifier::new("slack-signing-secret".to_string()).expect("verifier");
        let headers = headers("1710000000", "v0=deadbeef");
        let error = verifier
            .verify_with_timestamp(&headers, br#"{}"#, 1710000001)
            .expect_err("invalid signature should fail");

        assert!(error.message.contains("verification failed"));
    }

    fn headers(timestamp: &str, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-slack-request-timestamp",
            HeaderValue::from_str(timestamp).expect("timestamp header"),
        );
        headers.insert(
            "x-slack-signature",
            HeaderValue::from_str(signature).expect("signature header"),
        );

        headers
    }
}
