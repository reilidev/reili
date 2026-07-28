use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderName, HeaderValue};
use rmcp::model::Tool;
use serde_json::json;

use crate::outbound::mcp_streamable_http::{
    StaticMcpAuth, StreamableHttpMcpClient, build_client_implementation,
};

const DATADOG_MCP_SOURCE_LABEL: &str = "Datadog";
const DATADOG_MCP_TOOLSETS: &str = "core,security,dashboards,synthetics";
const DATADOG_API_KEY_HEADER: &str = "DD_API_KEY";
const DATADOG_APPLICATION_KEY_HEADER: &str = "DD_APPLICATION_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatadogMcpToolConfig {
    pub api_key: SecretString,
    pub app_key: SecretString,
    pub site: String,
}

pub(crate) type DatadogMcpHttpClient = StreamableHttpMcpClient<StaticMcpAuth>;

pub(crate) async fn connect(
    config: &DatadogMcpToolConfig,
) -> Result<(DatadogMcpHttpClient, Vec<Tool>), PortError> {
    let client = StreamableHttpMcpClient::new(
        DATADOG_MCP_SOURCE_LABEL,
        datadog_mcp_url(&config.site),
        StaticMcpAuth(build_datadog_mcp_http_client(config)?),
    );
    let tools = match client.list_tools().await {
        Ok(tools) => tools,
        Err(error) => {
            let diagnostic = diagnose_datadog_mcp_initialize(config).await;
            return Err(create_datadog_mcp_connect_error(error.message, diagnostic));
        }
    };

    Ok((client, tools))
}

fn build_datadog_mcp_http_client(
    config: &DatadogMcpToolConfig,
) -> Result<reqwest::Client, PortError> {
    reqwest::Client::builder()
        .default_headers(build_datadog_mcp_headers(config)?)
        .build()
        .map_err(|error| {
            PortError::new(format!(
                "Failed to build Datadog MCP HTTP client with default headers: {error}"
            ))
        })
}

async fn diagnose_datadog_mcp_initialize(
    config: &DatadogMcpToolConfig,
) -> Result<DatadogMcpInitializeDiagnostic, PortError> {
    let client_implementation = build_client_implementation();
    let response = reqwest::Client::new()
        .post(datadog_mcp_url(&config.site))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header(DATADOG_API_KEY_HEADER, config.api_key.expose())
        .header(DATADOG_APPLICATION_KEY_HEADER, config.app_key.expose())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": client_implementation.name,
                    "version": client_implementation.version,
                }
            }
        }))
        .send()
        .await
        .map_err(|error| {
            PortError::new(format!(
                "Failed to run Datadog MCP initialize diagnostic request: {error}"
            ))
        })?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.map_err(|error| {
        PortError::new(format!(
            "Failed to read Datadog MCP initialize diagnostic response body: {error}"
        ))
    })?;

    Ok(DatadogMcpInitializeDiagnostic {
        status,
        content_type,
        body,
    })
}

fn create_datadog_mcp_connect_error(
    base_error: String,
    diagnostic: Result<DatadogMcpInitializeDiagnostic, PortError>,
) -> PortError {
    match diagnostic {
        Ok(diagnostic) => PortError::connection_failed(format!(
            "Failed to connect to Datadog MCP server: {base_error}. Diagnostic initialize response: status={} content_type={} body={}",
            diagnostic.status, diagnostic.content_type, diagnostic.body
        )),
        Err(diagnostic_error) => PortError::connection_failed(format!(
            "Failed to connect to Datadog MCP server: {base_error}. Diagnostic request also failed: {}",
            diagnostic_error.message
        )),
    }
}

fn build_datadog_mcp_headers(config: &DatadogMcpToolConfig) -> Result<ReqwestHeaderMap, PortError> {
    let mut default_headers = ReqwestHeaderMap::new();
    default_headers.insert(
        HeaderName::from_static("dd_api_key"),
        HeaderValue::from_str(config.api_key.expose())
            .map_err(|error| PortError::new(format!("Invalid Datadog API key header: {error}")))?,
    );
    default_headers.insert(
        HeaderName::from_static("dd_application_key"),
        HeaderValue::from_str(config.app_key.expose()).map_err(|error| {
            PortError::new(format!("Invalid Datadog application key header: {error}"))
        })?,
    );

    Ok(default_headers)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DatadogMcpInitializeDiagnostic {
    status: u16,
    content_type: String,
    body: String,
}

fn datadog_mcp_url(site: &str) -> String {
    let site_domain = datadog_site_domain(site);

    format!("https://mcp.{site_domain}/api/unstable/mcp-server/mcp?toolsets={DATADOG_MCP_TOOLSETS}")
}

fn datadog_site_domain(site: &str) -> &str {
    let site = site.trim();
    let site = site
        .strip_prefix("https://")
        .or_else(|| site.strip_prefix("http://"))
        .unwrap_or(site);

    site.split('/').next().unwrap_or(site)
}

#[cfg(test)]
mod tests {
    use reili_core::secret::SecretString;
    use reqwest::header::HeaderValue;

    use super::{
        DatadogMcpToolConfig, build_datadog_mcp_headers, datadog_mcp_url, datadog_site_domain,
    };

    #[test]
    fn builds_datadog_mcp_url_from_site() {
        assert_eq!(
            datadog_mcp_url("datadoghq.eu"),
            "https://mcp.datadoghq.eu/api/unstable/mcp-server/mcp?toolsets=core,security,dashboards,synthetics"
        );
        assert_eq!(
            datadog_mcp_url("ap1.datadoghq.com"),
            "https://mcp.ap1.datadoghq.com/api/unstable/mcp-server/mcp?toolsets=core,security,dashboards,synthetics"
        );
    }

    #[test]
    fn extracts_domain_from_datadog_site_env_value() {
        assert_eq!(
            datadog_site_domain("ap1.datadoghq.com"),
            "ap1.datadoghq.com"
        );
        assert_eq!(
            datadog_site_domain("https://ap1.datadoghq.com"),
            "ap1.datadoghq.com"
        );
        assert_eq!(
            datadog_site_domain("https://ap1.datadoghq.com/"),
            "ap1.datadoghq.com"
        );
    }

    #[test]
    fn builds_datadog_mcp_headers_with_underscore_header_names() {
        let headers = build_datadog_mcp_headers(&DatadogMcpToolConfig {
            api_key: SecretString::from("api-key"),
            app_key: SecretString::from("app-key"),
            site: "datadoghq.com".to_string(),
        })
        .expect("build headers");

        assert_eq!(
            headers.get("dd_api_key"),
            Some(&HeaderValue::from_static("api-key"))
        );
        assert_eq!(
            headers.get("dd_application_key"),
            Some(&HeaderValue::from_static("app-key"))
        );
        assert!(headers.get("dd-api-key").is_none());
        assert!(headers.get("dd-application-key").is_none());
    }
}
