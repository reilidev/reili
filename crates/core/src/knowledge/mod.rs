pub mod web_search;

pub use web_search::{
    WebCitation, WebSearchExecution, WebSearchInput, WebSearchPort, WebSearchResult,
    WebSearchUserLocation,
};

#[cfg(any(test, feature = "test-support"))]
pub use web_search::MockWebSearchPort;
