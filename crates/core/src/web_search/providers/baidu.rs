use async_trait::async_trait;

use super::{
    blocked_by_challenge, fetch_search_html, parse_results, ParserConfig, SearchProvider,
    SearchProviderContext,
};
use crate::web_search::model::{
    SearchEngine, SearchProviderFailure, SearchRequest, SearchResultItem,
};

pub struct BaiduProvider;

const RESULT_SELECTORS: &[&str] = &[
    "div.result",
    "div.c-container",
    "div[class*='result']",
    "div[srcid]",
];
const TITLE_SELECTORS: &[&str] = &["h3 a[href]", ".t a[href]", "a[href]"];
const SNIPPET_SELECTORS: &[&str] = &[
    ".c-abstract",
    ".c-color-text",
    ".content-right",
    ".c-row",
    ".c-span-last",
    "p",
];
const CAPTCHA_NEEDLES: &[&str] = &[
    "百度安全验证",
    "请输入验证码",
    "verify.baidu.com",
    "安全验证",
];

pub(crate) fn parse_baidu_results(html: &str) -> Vec<SearchResultItem> {
    parse_results(
        html,
        ParserConfig {
            engine: SearchEngine::Baidu,
            base_url: "https://www.baidu.com/",
            result_selectors: RESULT_SELECTORS,
            title_selectors: TITLE_SELECTORS,
            snippet_selectors: SNIPPET_SELECTORS,
        },
    )
}

#[async_trait]
impl SearchProvider for BaiduProvider {
    fn engine(&self) -> SearchEngine {
        SearchEngine::Baidu
    }

    async fn search(
        &self,
        request: &SearchRequest,
        ctx: &SearchProviderContext<'_>,
    ) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
        let mut url = reqwest::Url::parse("https://www.baidu.com/s").map_err(|e| {
            SearchProviderFailure::new(SearchEngine::Baidu, "invalid_provider_url", e.to_string())
        })?;
        url.query_pairs_mut()
            .append_pair("wd", &request.effective_query)
            .append_pair("rn", &request.limit.min(10).to_string())
            .append_pair("ie", "utf-8");

        let html = fetch_search_html(
            ctx.client,
            SearchEngine::Baidu,
            url,
            Some("https://www.baidu.com/"),
            "zh-CN,zh;q=0.9,en;q=0.6",
        )
        .await?;
        if blocked_by_challenge(&html, CAPTCHA_NEEDLES) {
            return Err(SearchProviderFailure::new(
                SearchEngine::Baidu,
                "captcha",
                "Baidu returned a verification page",
            ));
        }
        Ok(parse_baidu_results(&html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_baidu_fixture() {
        let html = include_str!("fixtures/baidu_normal.html");

        let results = parse_baidu_results(html);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Tauri 官方文档");
        assert_eq!(results[0].source, "tauri.app");
        assert_eq!(results[0].engine, SearchEngine::Baidu);
    }

    #[test]
    fn detects_baidu_challenge_fixture() {
        let html = include_str!("fixtures/baidu_captcha.html");

        assert!(blocked_by_challenge(html, CAPTCHA_NEEDLES));
    }
}
