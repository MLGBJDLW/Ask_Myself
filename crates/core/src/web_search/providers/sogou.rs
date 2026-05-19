use async_trait::async_trait;

use super::{
    blocked_by_challenge, fetch_search_html, parse_results, ParserConfig, SearchProvider,
    SearchProviderContext,
};
use crate::web_search::model::{
    SearchEngine, SearchProviderFailure, SearchRequest, SearchResultItem,
};

pub struct SogouProvider;

const RESULT_SELECTORS: &[&str] = &[".vrwrap", ".rb", ".result", "div[class*='result']"];
const TITLE_SELECTORS: &[&str] = &["h3 a[href]", ".vr-title a[href]", "a[href]"];
const SNIPPET_SELECTORS: &[&str] = &[".str_info", ".ft", ".text-layout", ".fz-mid", "p"];
const CAPTCHA_NEEDLES: &[&str] = &["antispider", "请输入验证码", "用户您好", "安全验证"];

pub(crate) fn parse_sogou_results(html: &str) -> Vec<SearchResultItem> {
    parse_results(
        html,
        ParserConfig {
            engine: SearchEngine::Sogou,
            base_url: "https://www.sogou.com/",
            result_selectors: RESULT_SELECTORS,
            title_selectors: TITLE_SELECTORS,
            snippet_selectors: SNIPPET_SELECTORS,
        },
    )
}

#[async_trait]
impl SearchProvider for SogouProvider {
    fn engine(&self) -> SearchEngine {
        SearchEngine::Sogou
    }

    async fn search(
        &self,
        request: &SearchRequest,
        ctx: &SearchProviderContext<'_>,
    ) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
        let mut url = reqwest::Url::parse("https://www.sogou.com/web").map_err(|e| {
            SearchProviderFailure::new(SearchEngine::Sogou, "invalid_provider_url", e.to_string())
        })?;
        url.query_pairs_mut()
            .append_pair("query", &request.effective_query)
            .append_pair("page", "1");

        let html = fetch_search_html(
            ctx.client,
            SearchEngine::Sogou,
            url,
            Some("https://www.sogou.com/"),
            "zh-CN,zh;q=0.9,en;q=0.6",
        )
        .await?;
        if blocked_by_challenge(&html, CAPTCHA_NEEDLES) {
            return Err(SearchProviderFailure::new(
                SearchEngine::Sogou,
                "captcha",
                "Sogou returned a verification page",
            ));
        }
        Ok(parse_sogou_results(&html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sogou_fixture() {
        let html = include_str!("fixtures/sogou_normal.html");

        let results = parse_sogou_results(html);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "微信公众平台技术文档");
        assert_eq!(results[0].source, "developers.weixin.qq.com");
        assert_eq!(results[0].engine, SearchEngine::Sogou);
    }

    #[test]
    fn detects_sogou_challenge_fixture() {
        let html = include_str!("fixtures/sogou_captcha.html");

        assert!(blocked_by_challenge(html, CAPTCHA_NEEDLES));
    }
}
