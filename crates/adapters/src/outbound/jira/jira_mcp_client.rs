use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{AUTHORIZATION, HeaderMap as ReqwestHeaderMap, HeaderValue};
use rmcp::model::Tool;

use crate::outbound::mcp_streamable_http::{StaticMcpAuth, StreamableHttpMcpClient};

const JIRA_MCP_SOURCE_LABEL: &str = "JIRA";
/// Atlassian's Rovo MCP server. One fixed endpoint serves every Cloud tenant; the target site is
/// selected per tool call via the `cloudId` argument, not the connection URL.
const ROVO_MCP_URL: &str = "https://mcp.atlassian.com/v1/mcp";

#[derive(Clone, PartialEq, Eq)]
pub struct JiraMcpConfig {
    /// Atlassian Cloud site hostname, e.g. `acme.atlassian.net`. Used only as the `cloudId`
    /// argument stamped onto every tool call, not as part of the connection URL.
    pub site: String,
    pub service_account_api_token: SecretString,
}

impl std::fmt::Debug for JiraMcpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraMcpConfig")
            .field("site", &self.site)
            .field("service_account_api_token", &"[REDACTED]")
            .finish()
    }
}

pub(crate) type JiraMcpHttpClient = StreamableHttpMcpClient<StaticMcpAuth>;

pub(crate) async fn connect(
    config: &JiraMcpConfig,
) -> Result<(JiraMcpHttpClient, Vec<Tool>), PortError> {
    let client = StreamableHttpMcpClient::new(
        JIRA_MCP_SOURCE_LABEL,
        ROVO_MCP_URL,
        StaticMcpAuth(build_jira_mcp_http_client(config)?),
    );
    let tools = client.list_tools().await.map_err(|error| {
        PortError::connection_failed(format!(
            "Failed to connect to JIRA (Atlassian Rovo) MCP server: {}",
            error.message
        ))
    })?;

    Ok((client, tools))
}

fn build_jira_mcp_http_client(config: &JiraMcpConfig) -> Result<reqwest::Client, PortError> {
    reqwest::Client::builder()
        .default_headers(build_jira_mcp_headers(config)?)
        .build()
        .map_err(|error| {
            PortError::new(format!(
                "Failed to build JIRA MCP HTTP client with default headers: {error}"
            ))
        })
}

fn build_jira_mcp_headers(config: &JiraMcpConfig) -> Result<ReqwestHeaderMap, PortError> {
    let mut default_headers = ReqwestHeaderMap::new();
    default_headers.insert(AUTHORIZATION, build_bearer_auth_header(config)?);

    Ok(default_headers)
}

fn build_bearer_auth_header(config: &JiraMcpConfig) -> Result<HeaderValue, PortError> {
    let token = config.service_account_api_token.expose().trim();
    let header_value = if token.starts_with("Bearer ") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    };

    HeaderValue::from_str(&header_value)
        .map_err(|error| PortError::new(format!("Invalid JIRA MCP authorization header: {error}")))
}

#[cfg(test)]
mod tests {
    use reili_core::secret::SecretString;
    use reqwest::header::{AUTHORIZATION, HeaderValue};

    use super::{JiraMcpConfig, build_jira_mcp_headers};

    fn config(token: &str) -> JiraMcpConfig {
        JiraMcpConfig {
            site: "acme.atlassian.net".to_string(),
            service_account_api_token: SecretString::from(token),
        }
    }

    #[test]
    fn builds_bearer_authorization_header_when_token_has_no_prefix() {
        let headers = build_jira_mcp_headers(&config("api-key")).expect("build headers");

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer api-key"))
        );
    }

    #[test]
    fn does_not_double_prefix_bearer_token() {
        let headers = build_jira_mcp_headers(&config("Bearer api-key")).expect("build headers");

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer api-key"))
        );
    }

    #[test]
    fn debug_redacts_service_account_api_token() {
        let debug_output = format!("{:?}", config("super-secret"));

        assert!(!debug_output.contains("super-secret"));
        assert!(debug_output.contains("[REDACTED]"));
    }
}
