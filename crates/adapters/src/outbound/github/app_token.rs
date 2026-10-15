//! GitHub App installation tokens for Work State writes. Tokens are requested *down-scoped* to the
//! single management repository and to `issues: write` + `metadata: read`, so even an App
//! installation with broader permissions cannot write anywhere else through Reili.
//! API: <https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app>

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use octocrab::auth::create_jwt;
use octocrab::models::{AppId, InstallationToken};
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::Serialize;
use tokio::sync::Mutex;

use super::github_mcp_client::{build_bearer_auth_header, parse_github_app_id, truncate_for_error};

const REFRESH_SKEW_MINUTES: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InstallationTokenScope {
    pub repositories: Vec<String>,
    pub permissions: BTreeMap<String, String>,
}

impl InstallationTokenScope {
    /// Least privilege for Work State: issues and labels in one repository.
    pub(crate) fn work_state(repository_name: &str) -> Self {
        Self {
            repositories: vec![repository_name.to_string()],
            permissions: BTreeMap::from([
                ("issues".to_string(), "write".to_string()),
                ("metadata".to_string(), "read".to_string()),
            ]),
        }
    }
}

pub(crate) struct GitHubAppTokenProvider {
    app_id: AppId,
    installation_id: u32,
    key: Arc<jsonwebtoken::EncodingKey>,
    api_base_url: String,
    scope: InstallationTokenScope,
    http: reqwest::Client,
    cached: Mutex<Option<(SecretString, DateTime<Utc>)>>,
}

impl GitHubAppTokenProvider {
    pub(crate) fn new(
        app_id: &str,
        private_key: &SecretString,
        installation_id: u32,
        api_base_url: &str,
        scope: InstallationTokenScope,
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
            scope,
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
            .json(&self.scope)
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
mod tests {
    use super::InstallationTokenScope;

    #[test]
    fn work_state_scope_is_least_privilege() {
        let scope = InstallationTokenScope::work_state("reili-work");

        assert_eq!(
            serde_json::to_value(&scope).expect("json"),
            serde_json::json!({
                "repositories": ["reili-work"],
                "permissions": { "issues": "write", "metadata": "read" }
            })
        );
    }
}
