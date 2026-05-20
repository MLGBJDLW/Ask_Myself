use std::collections::HashSet;

use reqwest::Url;

use super::model::{
    SearchEngine, SearchLanguage, SearchRegion, SearchRequest, TimeRange, WebSearchArgs,
    WebSearchProviderProfile, WebSearchReranker, MAX_SEARCH_LIMIT,
};

pub fn build_search_request(args: WebSearchArgs) -> Result<SearchRequest, String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err("web_search requires a non-empty query".to_string());
    }

    let language = resolve_language(query, args.language);
    let region = resolve_region(language, args.region);
    let provider_profile = args.provider_profile.unwrap_or_default();
    let explicit_engines = !args.engines.is_empty();
    let engines = resolve_engines(language, region, provider_profile, &args.engines)?;
    let site = normalize_site(args.site.as_deref())?;
    let effective_query = match site.as_deref() {
        Some(site) => format!("site:{site} {query}"),
        None => query.to_string(),
    };
    let reranker = resolve_reranker(query, args.time_range, args.reranker.unwrap_or_default());

    Ok(SearchRequest {
        query: query.to_string(),
        effective_query,
        limit: args.limit.clamp(1, MAX_SEARCH_LIMIT),
        region,
        language,
        engines,
        explicit_engines,
        time_range: args.time_range,
        site,
        include_snippets: args.include_snippets,
        provider_profile,
        reranker,
    })
}

fn resolve_language(query: &str, requested: SearchLanguage) -> SearchLanguage {
    match requested {
        SearchLanguage::Auto if contains_cjk(query) => SearchLanguage::Zh,
        SearchLanguage::Auto => SearchLanguage::En,
        explicit => explicit,
    }
}

fn resolve_region(language: SearchLanguage, requested: SearchRegion) -> SearchRegion {
    match requested {
        SearchRegion::Auto if language == SearchLanguage::Zh => SearchRegion::MainlandCn,
        SearchRegion::Auto => SearchRegion::Global,
        explicit => explicit,
    }
}

fn resolve_engines(
    language: SearchLanguage,
    region: SearchRegion,
    provider_profile: WebSearchProviderProfile,
    requested: &[String],
) -> Result<Vec<SearchEngine>, String> {
    if requested.is_empty() {
        return Ok(default_engines_for_profile(
            language,
            region,
            provider_profile,
        ));
    }

    let mut engines = Vec::new();
    let mut unknown = Vec::new();
    let mut seen = HashSet::new();
    for engine in requested {
        match SearchEngine::parse(engine) {
            Some(parsed) if seen.insert(parsed) => engines.push(parsed),
            Some(_) => {}
            None => unknown.push(engine.clone()),
        }
    }

    if !unknown.is_empty() {
        return Err(format!(
            "Unsupported search engine(s): {}. Allowed: baidu, sogou, bing, duckduckgo.",
            unknown.join(", ")
        ));
    }
    if engines.is_empty() {
        return Err("No valid search engines were requested".to_string());
    }

    if language == SearchLanguage::Zh || region == SearchRegion::MainlandCn {
        engines.sort_by_key(|engine| match engine {
            SearchEngine::Baidu => 0,
            SearchEngine::Sogou => 1,
            SearchEngine::Bing => 2,
            SearchEngine::DuckDuckGo => 3,
        });
    }

    Ok(engines)
}

pub(crate) fn default_engines_for_profile(
    language: SearchLanguage,
    region: SearchRegion,
    provider_profile: WebSearchProviderProfile,
) -> Vec<SearchEngine> {
    let mainland = language == SearchLanguage::Zh || region == SearchRegion::MainlandCn;
    match provider_profile {
        WebSearchProviderProfile::Default | WebSearchProviderProfile::Free if mainland => {
            vec![SearchEngine::Baidu, SearchEngine::Sogou, SearchEngine::Bing]
        }
        WebSearchProviderProfile::Default | WebSearchProviderProfile::Free => {
            vec![SearchEngine::Bing, SearchEngine::DuckDuckGo]
        }
        WebSearchProviderProfile::FreeVerified if mainland => vec![
            SearchEngine::Baidu,
            SearchEngine::Sogou,
            SearchEngine::Bing,
            SearchEngine::DuckDuckGo,
        ],
        WebSearchProviderProfile::FreeVerified => vec![
            SearchEngine::Bing,
            SearchEngine::DuckDuckGo,
            SearchEngine::Sogou,
        ],
        WebSearchProviderProfile::MaxEvidence if mainland => vec![
            SearchEngine::Baidu,
            SearchEngine::Sogou,
            SearchEngine::Bing,
            SearchEngine::DuckDuckGo,
        ],
        WebSearchProviderProfile::MaxEvidence => vec![
            SearchEngine::Bing,
            SearchEngine::DuckDuckGo,
            SearchEngine::Sogou,
            SearchEngine::Baidu,
        ],
    }
}

fn resolve_reranker(
    query: &str,
    time_range: TimeRange,
    requested: WebSearchReranker,
) -> WebSearchReranker {
    if requested != WebSearchReranker::Auto {
        return requested;
    }

    let lower = query.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "docs",
            "documentation",
            "api",
            "sdk",
            "reference",
            "guide",
            "manual",
            "release notes",
        ],
    ) {
        return WebSearchReranker::DocsFirst;
    }
    if contains_any(
        &lower,
        &[
            "paper",
            "study",
            "research",
            "journal",
            "arxiv",
            "pubmed",
            "doi",
            "clinical trial",
        ],
    ) {
        return WebSearchReranker::Research;
    }
    if matches!(time_range, TimeRange::Day | TimeRange::Week)
        || contains_any(
            &lower,
            &["news", "latest", "today", "yesterday", "breaking", "report"],
        )
    {
        return WebSearchReranker::NewsBalanced;
    }

    WebSearchReranker::None
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn normalize_site(site: Option<&str>) -> Result<Option<String>, String> {
    let Some(site) = site.map(str::trim).filter(|site| !site.is_empty()) else {
        return Ok(None);
    };

    if site.split_whitespace().count() > 1 {
        return Err("site must be a single domain or URL".to_string());
    }

    let host = if site.starts_with("http://") || site.starts_with("https://") {
        Url::parse(site)
            .map_err(|e| format!("Invalid site URL: {e}"))?
            .host_str()
            .ok_or_else(|| "site URL has no host".to_string())?
            .to_string()
    } else {
        site.trim_matches('/').to_string()
    };

    let host = host
        .trim_start_matches("www.")
        .trim()
        .trim_matches('.')
        .to_ascii_lowercase();
    if host.is_empty()
        || host.contains('/')
        || host.contains('\\')
        || host.contains(':')
        || host == "localhost"
    {
        return Err("site must be a public domain such as example.com".to_string());
    }

    Ok(Some(host))
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xF900..=0xFAFF
                | 0x20000..=0x2A6DF
                | 0x2A700..=0x2B73F
                | 0x2B740..=0x2B81F
                | 0x2B820..=0x2CEAF
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_search::model::TimeRange;

    fn args(query: &str) -> WebSearchArgs {
        WebSearchArgs {
            query: query.to_string(),
            limit: 8,
            region: SearchRegion::Auto,
            language: SearchLanguage::Auto,
            engines: Vec::new(),
            time_range: TimeRange::Any,
            site: None,
            include_snippets: true,
            provider_profile: None,
            reranker: None,
        }
    }

    #[test]
    fn chinese_queries_route_to_baidu_first() {
        let request = build_search_request(args("人工智能 搜索 引擎")).unwrap();

        assert_eq!(request.language, SearchLanguage::Zh);
        assert_eq!(request.region, SearchRegion::MainlandCn);
        assert_eq!(
            request.engines,
            vec![SearchEngine::Baidu, SearchEngine::Sogou, SearchEngine::Bing]
        );
    }

    #[test]
    fn english_queries_use_global_pair() {
        let request = build_search_request(args("Tauri sidecar external binary")).unwrap();

        assert_eq!(request.language, SearchLanguage::En);
        assert_eq!(request.region, SearchRegion::Global);
        assert_eq!(
            request.engines,
            vec![SearchEngine::Bing, SearchEngine::DuckDuckGo]
        );
    }

    #[test]
    fn explicit_chinese_engine_list_still_puts_baidu_first() {
        let mut raw = args("中文 搜索");
        raw.engines = vec!["bing".to_string(), "baidu".to_string()];

        let request = build_search_request(raw).unwrap();

        assert_eq!(
            request.engines,
            vec![SearchEngine::Baidu, SearchEngine::Bing]
        );
    }

    #[test]
    fn site_filter_stays_single_focused_query() {
        let mut raw = args("searxng");
        raw.site = Some("https://github.com/search".to_string());

        let request = build_search_request(raw).unwrap();

        assert_eq!(request.site.as_deref(), Some("github.com"));
        assert_eq!(request.effective_query, "site:github.com searxng");
    }

    #[test]
    fn max_evidence_profile_uses_all_native_engines() {
        let mut raw = args("current search coverage");
        raw.provider_profile = Some(WebSearchProviderProfile::MaxEvidence);

        let request = build_search_request(raw).unwrap();

        assert_eq!(
            request.engines,
            vec![
                SearchEngine::Bing,
                SearchEngine::DuckDuckGo,
                SearchEngine::Sogou,
                SearchEngine::Baidu
            ]
        );
    }

    #[test]
    fn auto_reranker_detects_docs_queries() {
        let request = build_search_request(args("tauri api reference")).unwrap();

        assert_eq!(request.reranker, WebSearchReranker::DocsFirst);
    }
}
