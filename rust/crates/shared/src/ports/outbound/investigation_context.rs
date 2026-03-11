use std::sync::Arc;

use super::{
    DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
    DatadogMetricCatalogPort, DatadogMetricQueryPort, GithubSearchPort, WebSearchPort,
};

pub struct InvestigationResources {
    pub log_aggregate_port: Arc<dyn DatadogLogAggregatePort>,
    pub log_search_port: Arc<dyn DatadogLogSearchPort>,
    pub metric_catalog_port: Arc<dyn DatadogMetricCatalogPort>,
    pub metric_query_port: Arc<dyn DatadogMetricQueryPort>,
    pub event_search_port: Arc<dyn DatadogEventSearchPort>,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub github_search_port: Arc<dyn GithubSearchPort>,
    pub web_search_port: Arc<dyn WebSearchPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationRuntime {
    pub started_at_iso: String,
    pub channel: String,
    pub thread_ts: String,
    pub retry_count: u32,
}

pub struct InvestigationContext {
    pub resources: InvestigationResources,
    pub runtime: InvestigationRuntime,
}
