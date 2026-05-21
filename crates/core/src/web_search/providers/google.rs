use async_trait::async_trait;

use super::{
    blocked_by_challenge, fetch_search_html, parse_results, ParserConfig, SearchProvider,
    SearchProviderContext,
};
use crate::web_search::model::{
    SearchEngine, SearchProviderFailure, SearchRequest, SearchResultItem,
};

pub struct GoogleProvider;

const RESULT_SELECTORS: &[&str] = &["div.g", ".MjjYud", "div[data-sokoban-container]"];
const TITLE_SELECTORS: &[&str] = &["a[href] h3", "h3 a[href]", "a[href]"];
const SNIPPET_SELECTORS: &[&str] = &[".VwiC3b", ".IsZvec", ".aCOpRe", "span"];
const CAPTCHA_NEEDLES: &[&str] = &[
    "unusual traffic",
    "our systems have detected",
    "captcha",
    "sorry/index",
    "consent.google.com",
    "before you continue to google search",
];

pub(crate) fn parse_google_results(html: &str) -> Vec<SearchResultItem> {
    parse_results(
        html,
        ParserConfig {
            engine: SearchEngine::Google,
            base_url: "https://www.google.com/",
            result_selectors: RESULT_SELECTORS,
            title_selectors: TITLE_SELECTORS,
            snippet_selectors: SNIPPET_SELECTORS,
        },
    )
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    fn engine(&self) -> SearchEngine {
        SearchEngine::Google
    }

    async fn search(
        &self,
        request: &SearchRequest,
        ctx: &SearchProviderContext<'_>,
    ) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
        let mut url = reqwest::Url::parse("https://www.google.com/search").map_err(|e| {
            SearchProviderFailure::new(SearchEngine::Google, "invalid_provider_url", e.to_string())
        })?;
        url.query_pairs_mut()
            .append_pair("q", &request.effective_query)
            .append_pair("num", &request.limit.min(10).to_string())
            .append_pair("hl", "en");

        let html = fetch_search_html(
            ctx.client,
            SearchEngine::Google,
            url,
            Some("https://www.google.com/"),
            "en-US,en;q=0.9,zh-CN;q=0.6",
        )
        .await?;
        if blocked_by_challenge(&html, CAPTCHA_NEEDLES) {
            return Err(SearchProviderFailure::new(
                SearchEngine::Google,
                "captcha",
                "Google returned a verification or consent page",
            ));
        }
        Ok(parse_google_results(&html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_fixture() {
        let html = include_str!("fixtures/google_normal.html");

        let results = parse_google_results(html);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Tauri Documentation");
        assert_eq!(results[0].source, "tauri.app");
        assert_eq!(results[0].engine, SearchEngine::Google);
    }

    #[test]
    fn detects_google_challenge_fixture() {
        let html = include_str!("fixtures/google_captcha.html");

        assert!(blocked_by_challenge(html, CAPTCHA_NEEDLES));
    }
}
