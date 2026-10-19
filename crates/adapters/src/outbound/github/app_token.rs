use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use octocrab::auth::create_jwt;
use octocrab::models::{AppId, InstallationToken};
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use tokio::sync::Mutex;

use super::github_mcp_client::{build_bearer_auth_header, parse_github_app_id, truncate_for_error};

const REFRESH_SKEW_MINUTES: i64 = 5;

pub(crate) struct GitHubAppTokenProvider {
    app_id: AppId,
    installation_id: u32,
    key: Arc<jsonwebtoken::EncodingKey>,
    api_base_url: String,
    http: reqwest::Client,
    cached: Mutex<Option<(SecretString, DateTime<Utc>)>>,
}

impl GitHubAppTokenProvider {
    pub(crate) fn new(
        app_id: &str,
        private_key: &SecretString,
        installation_id: u32,
        api_base_url: &str,
        user_agent: &str,
    ) -> Result<Self, PortError> {
        let key =
            jsonwebtoken::EncodingKey::from_rsa_pem(private_key.as_bytes()).map_err(|error| {
                PortError::invalid_input(format!("Failed to parse GitHub App private key: {error}"))
            })?;
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|error| {
                PortError::new(format!("Failed to build GitHub HTTP client: {error}"))
            })?;
        Ok(Self {
            app_id: parse_github_app_id(app_id)?,
            installation_id,
            key: Arc::new(key),
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
            http,
            cached: Mutex::new(None),
        })
    }

    pub(crate) async fn token(&self) -> Result<SecretString, PortError> {
        let mut cached = self.cached.lock().await;
        if let Some((token, refresh_at)) = cached.as_ref()
            && Utc::now() < *refresh_at
        {
            return Ok(token.clone());
        }
        let (token, expires_at) = self.request().await?;
        *cached = Some((
            token.clone(),
            expires_at - Duration::minutes(REFRESH_SKEW_MINUTES),
        ));
        Ok(token)
    }

    async fn request(&self) -> Result<(SecretString, DateTime<Utc>), PortError> {
        let jwt = create_jwt(self.app_id, self.key.as_ref()).map_err(|error| {
            PortError::invalid_input(format!("Failed to sign GitHub App JWT: {error}"))
        })?;
        let response = self
            .http
            .post(format!(
                "{}/app/installations/{}/access_tokens",
                self.api_base_url, self.installation_id
            ))
            .header(AUTHORIZATION, build_bearer_auth_header(&jwt)?)
            .header(ACCEPT, "application/vnd.github+json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|error| {
                PortError::connection_failed(format!(
                    "GitHub installation token request failed: {error}"
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            PortError::invalid_response(format!("Failed to read GitHub token response: {error}"))
        })?;
        if !status.is_success() {
            return Err(PortError::http_status(
                status.as_u16(),
                format!(
                    "GitHub installation token request failed: status={} body={}",
                    status.as_u16(),
                    truncate_for_error(&body)
                ),
            ));
        }
        let token: InstallationToken = serde_json::from_str(&body).map_err(|error| {
            PortError::invalid_response(format!("Invalid GitHub token response: {error}"))
        })?;
        let expires_at = token
            .expires_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map_or_else(
                || Utc::now() + Duration::minutes(30),
                |value| value.with_timezone(&Utc),
            );
        Ok((SecretString::from(token.token), expires_at))
    }
}

#[cfg(test)]
pub(crate) fn test_app_private_key() -> SecretString {
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};

    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SecretString::from(
        KEY.get_or_init(|| {
            rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)
                .expect("generate test RSA key")
                .to_pkcs8_pem(LineEnding::LF)
                .expect("encode test RSA key")
                .to_string()
        })
        .as_str(),
    )
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{GitHubAppTokenProvider, test_app_private_key};

    async fn provider_issuing(
        expires_in: Duration,
        expected_requests: u64,
    ) -> (MockServer, GitHubAppTokenProvider) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/7/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "token": "ghs_test",
                "permissions": {},
                "expires_at": (Utc::now() + expires_in).to_rfc3339(),
            })))
            .expect(expected_requests)
            .mount(&server)
            .await;
        let provider = GitHubAppTokenProvider::new(
            "123",
            &test_app_private_key(),
            7,
            &server.uri(),
            "reili-test",
        )
        .expect("provider");
        (server, provider)
    }

    #[tokio::test]
    async fn reuses_the_token_until_it_nears_expiry() {
        let (server, provider) = provider_issuing(Duration::hours(1), 1).await;

        let first = provider.token().await.expect("first token");
        let second = provider.token().await.expect("second token");

        assert_eq!(first.expose(), second.expose());
        server.verify().await;
    }

    #[tokio::test]
    async fn requests_a_new_token_when_the_cached_one_nears_expiry() {
        let (server, provider) = provider_issuing(Duration::minutes(4), 2).await;

        provider.token().await.expect("first token");
        provider.token().await.expect("second token");

        server.verify().await;
    }
}
