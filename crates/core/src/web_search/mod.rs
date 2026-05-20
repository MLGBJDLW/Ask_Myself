//! Native public web search support.
//!
//! This module is intentionally separate from `crate::search`, which owns the
//! local knowledge-base search index.

pub mod model;
pub mod providers;
pub mod router;

pub use model::{
    SearchCacheInfo, SearchEngine, SearchLanguage, SearchProviderFailure,
    SearchProviderHealthState, SearchProviderRunInfo, SearchRegion, SearchRequest, SearchResponse,
    SearchResultItem, SearchTimeRangeInfo, TimeRange, WebSearchArgs, WebSearchProviderProfile,
    WebSearchProviderStatus, WebSearchReranker,
};
pub use providers::{provider_for_engine, SearchProvider, SearchProviderContext};
pub use router::build_search_request;
pub(crate) use router::default_engines_for_profile;
