pub mod event_search;
pub mod log_aggregate;
pub mod log_search;
pub mod metric_catalog;
pub mod metric_query;

pub use event_search::{
    DatadogEventSearchParams, DatadogEventSearchPort, DatadogEventSearchResult,
};
pub use log_aggregate::{
    DatadogLogAggregateBucket, DatadogLogAggregateParams, DatadogLogAggregatePort,
};
pub use log_search::{DatadogLogSearchParams, DatadogLogSearchPort, DatadogLogSearchResult};
pub use metric_catalog::{DatadogMetricCatalogParams, DatadogMetricCatalogPort};
pub use metric_query::{
    DatadogMetricQueryParams, DatadogMetricQueryPoint, DatadogMetricQueryPort,
    DatadogMetricQueryResult,
};

#[cfg(any(test, feature = "test-support"))]
pub use event_search::MockDatadogEventSearchPort;
#[cfg(any(test, feature = "test-support"))]
pub use log_aggregate::MockDatadogLogAggregatePort;
#[cfg(any(test, feature = "test-support"))]
pub use log_search::MockDatadogLogSearchPort;
#[cfg(any(test, feature = "test-support"))]
pub use metric_catalog::MockDatadogMetricCatalogPort;
#[cfg(any(test, feature = "test-support"))]
pub use metric_query::MockDatadogMetricQueryPort;
