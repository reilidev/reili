//! Connector implementations for each external source. The transport/tool adapters live in
//! `mcp/<svc>` (MCP-backed sources) or `outbound/<svc>` (port-backed sources); here we wrap them as
//! uniform [`ConnectorFactory`](super::connector::ConnectorFactory) implementations.

mod datadog;
mod esa;
mod github;

pub use datadog::DatadogConnector;
pub use esa::EsaConnector;
pub use github::GitHubConnector;
