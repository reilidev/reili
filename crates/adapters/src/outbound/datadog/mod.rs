mod datadog_api_retry_config;
pub mod datadog_http_client;
pub mod event_search_adapter;
pub mod log_aggregate_adapter;
pub mod log_search_adapter;
pub mod metric_catalog_adapter;
pub mod metric_query_adapter;

pub use datadog_api_retry_config::DatadogApiRetryConfig;
pub use datadog_http_client::{
    DatadogApiVersion, DatadogHttpClient, DatadogHttpClientConfig, DatadogRequestInput,
};
pub use event_search_adapter::DatadogEventSearchAdapter;
pub use log_aggregate_adapter::DatadogLogAggregateAdapter;
pub use log_search_adapter::DatadogLogSearchAdapter;
pub use metric_catalog_adapter::DatadogMetricCatalogAdapter;
pub use metric_query_adapter::DatadogMetricQueryAdapter;
