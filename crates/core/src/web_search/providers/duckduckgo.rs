use async_trait::async_trait;

use super::{
    blocked_by_challenge, fetch_search_html, parse_results, ParserConfig, SearchProvider,
    SearchProviderContext,
};
use crate::web_search::model::{
    SearchEngine, SearchProviderFailure, SearchRequest, SearchResultItem,
};

pub struct DuckDuckGoProvider;

const RESULT_SELECTORS: &[&str] = &["div.result", ".web-result"];
const TITLE_SELECTORS: &[&str] = &["a.result__a[href]", ".result__title a[href]", "h2 a[href]"];
const SNIPPET_SELECTORS: &[&str] = &[".result__snippet", ".result__body", "a.result__snippet"];
const CAPTCHA_NEEDLES: &[&str] = &["captcha", "anomaly", "automated requests"];

pub(crate) fn parse_duckduckgo_results(html: &str) -> Vec<SearchResultItem> {
    parse_results(
        html,
        ParserConfig {
            engine: SearchEngine::DuckDuckGo,
            base_url: "https://duckduckgo.com/",
            result_selectors: RESULT_SELECTORS,
            title_selectors: TITLE_SELECTORS,
            snippet_selectors: SNIPPET_SELECTORS,
        },
    )
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    fn engine(&self) -> SearchEngine {
        SearchEngine::DuckDuckGo
    }

    async fn search(
        &self,
        request: &SearchRequest,
        ctx: &SearchProviderContext<'_>,
    ) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
        let mut url = reqwest::Url::parse("https://duckduckgo.com/html/").map_err(|e| {
            SearchProviderFailure::new(
                SearchEngine::DuckDuckGo,
                "invalid_provider_url",
                e.to_string(),
            )
        })?;
        url.query_pairs_mut()
            .append_pair("q", &request.effective_query);

        let html = fetch_search_html(
            ctx.client,
            SearchEngine::DuckDuckGo,
            url,
            Some("https://duckduckgo.com/"),
            "en-US,en;q=0.9,zh-CN;q=0.6",
        )
        .await?;
        if blocked_by_challenge(&html, CAPTCHA_NEEDLES) {
            return Err(SearchProviderFailure::new(
                SearchEngine::DuckDuckGo,
                "captcha",
                "DuckDuckGo returned a verification page",
            ));
        }
        Ok(parse_duckduckgo_results(&html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duckduckgo_fixture() {
        let html = include_str!("fixtures/duckduckgo_normal.html");

        let results = parse_duckduckgo_results(html);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Tauri Documentation");
        assert_eq!(results[0].source, "tauri.app");
        assert_eq!(results[0].engine, SearchEngine::DuckDuckGo);
        assert!(results[0].resolved);
    }
}
