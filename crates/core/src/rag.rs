use crate::models::EvidenceCard;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_QUERY_VARIANTS: usize = 2;
const DEFAULT_CONTEXT_CHUNKS: usize = 2;
const LOW_CONFIDENCE_CONTEXT_CHUNKS: usize = 3;
const SUMMARY_CHUNK_KIND: &str = "summary";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalConfidenceLevel {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalConfidence {
    pub level: RetrievalConfidenceLevel,
    pub score: f64,
    pub reasons: Vec<String>,
    pub suggested_action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagStrategyPlan {
    pub original_query: String,
    pub query_variants: Vec<String>,
    pub use_hyde: bool,
    pub hyde_query: Option<String>,
    pub requires_context_window: bool,
    pub context_chunks: usize,
    pub target_candidates: usize,
    pub second_pass_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagEvalCase {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub expected_chunk_ids: Vec<String>,
    pub retrieved_chunk_ids: Vec<String>,
    #[serde(default)]
    pub expected_sources: Vec<String>,
    #[serde(default)]
    pub retrieved_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_supported: Option<bool>,
    pub top_k: usize,
    #[serde(default)]
    pub failure_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagEvalReport {
    pub case_count: usize,
    pub hit_rate_at_k: f64,
    pub mean_reciprocal_rank: f64,
    pub source_accuracy: f64,
    pub citation_support_rate: f64,
    pub failures: Vec<String>,
    pub failure_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagBenchmarkCase {
    pub name: String,
    pub query: String,
    pub expected_source_kind: String,
    pub expected_sources: Vec<String>,
    pub expected_chunk_ids: Vec<String>,
    pub top_k: usize,
    pub tags: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagContextPack {
    pub ordering_policy: String,
    pub recommended_context_chunks: usize,
    pub primary_chunk_ids: Vec<String>,
    pub context_window_chunk_ids: Vec<String>,
    pub supporting_chunk_ids: Vec<String>,
    pub groups: Vec<RagContextGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagContextGroup {
    pub role: String,
    pub source_id: String,
    pub source_name: String,
    pub document_id: String,
    pub document_title: String,
    pub document_path: String,
    pub chunk_ids: Vec<String>,
    pub chunks: Vec<RagContextChunk>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagContextChunk {
    pub chunk_id: String,
    pub chunk_index: i64,
    pub chunk_kind: String,
    pub role: String,
    pub score: f64,
}

pub fn build_contextual_search_text(card: &EvidenceCard) -> String {
    let mut parts = vec![
        card.document_title.trim().to_string(),
        card.document_path.trim().to_string(),
        card.source_name.trim().to_string(),
    ];

    if !card.heading_path.is_empty() {
        parts.push(card.heading_path.join(" > "));
    }

    if let Some(snippet) = card
        .snippet
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(snippet.to_string());
    }

    parts.push(card.content.trim().to_string());
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_supporting_summary_card(card: &EvidenceCard) -> bool {
    card.chunk_kind.eq_ignore_ascii_case(SUMMARY_CHUNK_KIND)
}

pub fn rerank_evidence_cards(cards: &mut [EvidenceCard], query: &str) {
    let terms = normalized_terms(query);
    if terms.is_empty() || cards.is_empty() {
        return;
    }

    let phrase = query.trim().to_lowercase();

    for card in cards.iter_mut() {
        let contextual_text = build_contextual_search_text(card).to_lowercase();
        let metadata_text = format!(
            "{} {} {} {}",
            card.document_title,
            card.document_path,
            card.source_name,
            card.heading_path.join(" ")
        )
        .to_lowercase();

        let coverage = term_coverage(&contextual_text, &terms);
        if coverage <= f64::EPSILON {
            card.score *= 0.62;
            if is_supporting_summary_card(card) {
                card.score *= 0.82;
            }
            continue;
        }

        let phrase_bonus = if !phrase.is_empty() && contextual_text.contains(&phrase) {
            0.05
        } else {
            0.0
        };
        let metadata_bonus = if terms.iter().any(|term| metadata_text.contains(term)) {
            0.04
        } else {
            0.0
        };

        card.score = (card.score * (0.86 + coverage * 0.14))
            + (coverage * 0.08)
            + phrase_bonus
            + metadata_bonus;

        if is_supporting_summary_card(card) {
            card.score *= 0.82;
        }
    }

    sort_direct_chunks_before_summaries(cards);
}

pub fn assess_retrieval_confidence(cards: &[EvidenceCard], query: &str) -> RetrievalConfidence {
    if cards.is_empty() {
        return RetrievalConfidence {
            level: RetrievalConfidenceLevel::Low,
            score: 0.0,
            reasons: vec!["no_results".to_string()],
            suggested_action: "Run a second pass with query variants or HyDE, then use get_chunk_context on the best candidate.".to_string(),
        };
    }

    let terms = normalized_terms(query);
    let top = &cards[0];
    let top_score = top.score.clamp(0.0, 1.0);
    let top_coverage = if terms.is_empty() {
        0.5
    } else {
        term_coverage(&build_contextual_search_text(top).to_lowercase(), &terms)
    };
    let average_coverage = if terms.is_empty() {
        0.5
    } else {
        let sample: Vec<_> = cards.iter().take(3).collect();
        sample
            .iter()
            .map(|card| term_coverage(&build_contextual_search_text(card).to_lowercase(), &terms))
            .sum::<f64>()
            / sample.len() as f64
    };

    let unique_docs = cards
        .iter()
        .take(5)
        .map(|card| card.document_id)
        .collect::<HashSet<_>>()
        .len();
    let unique_sources = cards
        .iter()
        .take(5)
        .map(|card| card.source_id)
        .collect::<HashSet<_>>()
        .len();
    let diversity = ((unique_docs + unique_sources) as f64 / 6.0).clamp(0.0, 1.0);
    let credibility = cards
        .iter()
        .take(3)
        .map(|card| card.credibility.unwrap_or(0.5).clamp(0.0, 1.0))
        .sum::<f64>()
        / cards.iter().take(3).count().max(1) as f64;

    let score = (top_score * 0.40)
        + (top_coverage * 0.30)
        + (average_coverage * 0.15)
        + (diversity * 0.05)
        + (credibility * 0.10);

    let level = if score >= 0.70 && top_coverage >= 0.50 {
        RetrievalConfidenceLevel::High
    } else if score >= 0.35 && top_coverage >= 0.20 {
        RetrievalConfidenceLevel::Medium
    } else {
        RetrievalConfidenceLevel::Low
    };

    let suggested_action = match level {
        RetrievalConfidenceLevel::High => {
            "Use retrieve_evidence for exact quotes/citations.".to_string()
        }
        RetrievalConfidenceLevel::Medium => {
            "Use get_chunk_context on the best chunk before making detailed claims.".to_string()
        }
        RetrievalConfidenceLevel::Low => {
            "Run a second pass with query variants or HyDE, then use get_chunk_context on the best candidate.".to_string()
        }
    };

    RetrievalConfidence {
        level,
        score: round_score(score),
        reasons: vec![
            format!("top_score={:.3}", top_score),
            format!("query_coverage={:.3}", top_coverage),
            format!("avg_coverage={:.3}", average_coverage),
            format!("unique_documents={unique_docs}"),
            format!("unique_sources={unique_sources}"),
        ],
        suggested_action,
    }
}

pub fn plan_rag_strategy(query: &str, confidence: Option<&RetrievalConfidence>) -> RagStrategyPlan {
    let original_query = query.trim().to_string();
    let low_confidence = confidence
        .map(|c| c.level == RetrievalConfidenceLevel::Low)
        .unwrap_or(false);
    let needs_context = confidence
        .map(|c| c.level != RetrievalConfidenceLevel::High)
        .unwrap_or(false);
    let vague = looks_vague(&original_query);
    let compound = is_compound_query(&original_query);

    let mut query_variants = Vec::new();
    push_unique(&mut query_variants, original_query.clone());
    if let Some(variant) = build_decomposed_query_variant(&original_query) {
        push_unique(&mut query_variants, variant);
    }

    let use_hyde = vague || low_confidence;
    let hyde_query = if use_hyde && !original_query.is_empty() {
        Some(build_hypothetical_document_query(&original_query))
    } else {
        None
    };

    if query_variants.len() < MAX_QUERY_VARIANTS {
        if let Some(hyde) = hyde_query.clone() {
            push_unique(&mut query_variants, hyde);
        }
    }
    query_variants.truncate(MAX_QUERY_VARIANTS);

    let context_chunks = if low_confidence {
        LOW_CONFIDENCE_CONTEXT_CHUNKS
    } else {
        DEFAULT_CONTEXT_CHUNKS
    };

    let second_pass_reason = if low_confidence {
        Some("low retrieval confidence".to_string())
    } else if vague {
        Some("vague recall query".to_string())
    } else if compound {
        Some("compound query decomposition".to_string())
    } else {
        None
    };

    RagStrategyPlan {
        original_query,
        query_variants,
        use_hyde,
        hyde_query,
        requires_context_window: needs_context || low_confidence || vague || compound,
        context_chunks,
        target_candidates: if low_confidence {
            16
        } else if needs_context || compound {
            12
        } else {
            8
        },
        second_pass_reason,
    }
}

pub fn build_hypothetical_document_query(query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        return String::new();
    }
    format!("{query} key details rationale decision evidence implementation source reference")
}

pub fn build_context_pack(cards: &[EvidenceCard], context_chunks: usize) -> RagContextPack {
    let mut direct_cards: Vec<&EvidenceCard> = cards
        .iter()
        .filter(|card| !is_supporting_summary_card(card))
        .collect();
    let mut supporting_cards: Vec<&EvidenceCard> = cards
        .iter()
        .filter(|card| is_supporting_summary_card(card))
        .collect();

    direct_cards.sort_by(score_desc);
    supporting_cards.sort_by(score_desc);

    let primary_chunk_id = direct_cards
        .first()
        .or_else(|| supporting_cards.first())
        .map(|card| card.chunk_id.to_string());
    let primary_document_id = direct_cards
        .first()
        .or_else(|| supporting_cards.first())
        .map(|card| card.document_id);

    let mut ordered_cards = Vec::with_capacity(cards.len());
    ordered_cards.extend(direct_cards);
    ordered_cards.extend(supporting_cards);

    let mut groups: Vec<RagContextGroup> = Vec::new();
    let mut context_window_chunk_ids = Vec::new();
    let mut supporting_chunk_ids = Vec::new();

    for card in ordered_cards {
        let card_chunk_id = card.chunk_id.to_string();
        if is_supporting_summary_card(card) {
            supporting_chunk_ids.push(card_chunk_id.clone());
        } else if !context_window_chunk_ids
            .iter()
            .any(|chunk_id| chunk_id == &card_chunk_id)
        {
            context_window_chunk_ids.push(card_chunk_id.clone());
        }

        let group_role = if Some(card.document_id) == primary_document_id {
            "primary_source"
        } else if is_supporting_summary_card(card) {
            "supporting_source"
        } else {
            "secondary_source"
        };
        let chunk_role = if Some(card_chunk_id.as_str()) == primary_chunk_id.as_deref() {
            "primary_direct"
        } else if is_supporting_summary_card(card) {
            "supporting_summary"
        } else if Some(card.document_id) == primary_document_id {
            "same_document_context"
        } else {
            "secondary_direct"
        };

        let document_id = card.document_id.to_string();
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.document_id == document_id)
        {
            group.chunk_ids.push(card_chunk_id.clone());
            group.chunks.push(context_pack_chunk(card, chunk_role));
        } else {
            groups.push(RagContextGroup {
                role: group_role.to_string(),
                source_id: card.source_id.to_string(),
                source_name: card.source_name.clone(),
                document_id,
                document_title: card.document_title.clone(),
                document_path: card.document_path.clone(),
                chunk_ids: vec![card_chunk_id],
                chunks: vec![context_pack_chunk(card, chunk_role)],
            });
        }
    }

    RagContextPack {
        ordering_policy:
            "strongest direct chunk first; same-document context next; secondary sources next; direct chunks before supporting summaries; preserve source and document boundaries"
                .to_string(),
        recommended_context_chunks: context_chunks,
        primary_chunk_ids: primary_chunk_id.into_iter().collect(),
        context_window_chunk_ids,
        supporting_chunk_ids,
        groups,
    }
}

pub fn saved_rag_benchmark_suite() -> Vec<RagBenchmarkCase> {
    vec![
        RagBenchmarkCase {
            name: "local-file-direct-retrieval".to_string(),
            query: "project ideas semantic retrieval".to_string(),
            expected_source_kind: "local_file".to_string(),
            expected_sources: vec!["testdata/sample_vault/notes/project-ideas.md".to_string()],
            expected_chunk_ids: vec!["local:project-ideas:retrieval".to_string()],
            top_k: 5,
            tags: vec!["local_file".to_string(), "direct_chunk".to_string()],
            notes: "Local-file cases protect direct chunk recall and citation traceability."
                .to_string(),
        },
        RagBenchmarkCase {
            name: "web-page-source-accuracy".to_string(),
            query: "api notes rate limit retry behavior".to_string(),
            expected_source_kind: "web_page".to_string(),
            expected_sources: vec!["https://example.com/docs/api-notes".to_string()],
            expected_chunk_ids: vec!["web:api-notes:retry".to_string()],
            top_k: 5,
            tags: vec!["web_page".to_string(), "source_accuracy".to_string()],
            notes: "Web-page cases protect URL-level source accuracy.".to_string(),
        },
        RagBenchmarkCase {
            name: "multi-hop-graph-support".to_string(),
            query: "GraphRAG RAPTOR context packing tradeoffs".to_string(),
            expected_source_kind: "mixed".to_string(),
            expected_sources: vec![
                "testdata/sample_vault/notes/project-ideas.md".to_string(),
                "https://example.com/docs/rag-architecture".to_string(),
            ],
            expected_chunk_ids: vec![
                "local:project-ideas:graph-rag".to_string(),
                "web:rag-architecture:raptor".to_string(),
            ],
            top_k: 8,
            tags: vec![
                "multi_hop".to_string(),
                "graph_support".to_string(),
                "context_packing".to_string(),
            ],
            notes: "Graph summaries may support this case, but direct chunks remain the citation target."
                .to_string(),
        },
    ]
}

pub fn build_rag_eval_report(cases: &[RagEvalCase]) -> RagEvalReport {
    if cases.is_empty() {
        return RagEvalReport {
            case_count: 0,
            hit_rate_at_k: 0.0,
            mean_reciprocal_rank: 0.0,
            source_accuracy: 0.0,
            citation_support_rate: 0.0,
            failures: Vec::new(),
            failure_notes: Vec::new(),
        };
    }

    let mut hit_count = 0usize;
    let mut reciprocal_rank_sum = 0.0f64;
    let mut source_case_count = 0usize;
    let mut source_hit_count = 0usize;
    let mut citation_case_count = 0usize;
    let mut citation_supported_count = 0usize;
    let mut failures = Vec::new();
    let mut failure_notes = Vec::new();

    for case in cases {
        let expected: HashSet<&str> = case.expected_chunk_ids.iter().map(String::as_str).collect();
        let top_k = if case.top_k == 0 {
            case.retrieved_chunk_ids.len()
        } else {
            case.top_k
        };
        let rank = case
            .retrieved_chunk_ids
            .iter()
            .take(top_k)
            .position(|chunk_id| expected.contains(chunk_id.as_str()));

        if let Some(rank) = rank {
            hit_count += 1;
            reciprocal_rank_sum += 1.0 / (rank + 1) as f64;
        } else {
            failures.push(case.name.clone());
            failure_notes.push(format!(
                "{}: expected chunk missing from top {top_k}",
                case.name
            ));
        }

        if !case.expected_sources.is_empty() {
            source_case_count += 1;
            let expected_sources: HashSet<&str> =
                case.expected_sources.iter().map(String::as_str).collect();
            let source_hit = case
                .retrieved_sources
                .iter()
                .take(top_k)
                .any(|source| expected_sources.contains(source.as_str()));
            if source_hit {
                source_hit_count += 1;
            } else {
                failure_notes.push(format!(
                    "{}: expected source not present in top {top_k}",
                    case.name
                ));
            }
        }

        if let Some(citation_supported) = case.citation_supported {
            citation_case_count += 1;
            if citation_supported {
                citation_supported_count += 1;
            } else {
                failure_notes.push(format!("{}: citation support failed", case.name));
            }
        }

        for note in &case.failure_notes {
            failure_notes.push(format!("{}: {note}", case.name));
        }
    }

    RagEvalReport {
        case_count: cases.len(),
        hit_rate_at_k: hit_count as f64 / cases.len() as f64,
        mean_reciprocal_rank: reciprocal_rank_sum / cases.len() as f64,
        source_accuracy: if source_case_count == 0 {
            0.0
        } else {
            source_hit_count as f64 / source_case_count as f64
        },
        citation_support_rate: if citation_case_count == 0 {
            0.0
        } else {
            citation_supported_count as f64 / citation_case_count as f64
        },
        failures,
        failure_notes,
    }
}

fn context_pack_chunk(card: &EvidenceCard, role: &str) -> RagContextChunk {
    RagContextChunk {
        chunk_id: card.chunk_id.to_string(),
        chunk_index: card.chunk_index,
        chunk_kind: card.chunk_kind.clone(),
        role: role.to_string(),
        score: round_score(card.score),
    }
}

fn score_desc(a: &&EvidenceCard, b: &&EvidenceCard) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn sort_direct_chunks_before_summaries(cards: &mut [EvidenceCard]) {
    cards.sort_by(
        |a, b| match (is_supporting_summary_card(a), is_supporting_summary_card(b)) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            _ => b
                .score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal),
        },
    );
}

fn normalized_terms(text: &str) -> Vec<String> {
    let stopwords = [
        "the", "and", "or", "for", "with", "that", "this", "from", "about", "what", "when",
        "where", "which", "into", "than", "then", "previous",
    ];
    let stopwords: HashSet<&str> = stopwords.into_iter().collect();
    let mut terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|term| term.chars().count() > 1)
        .map(|term| term.to_lowercase())
        .filter(|term| !stopwords.contains(term.as_str()))
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

fn term_coverage(haystack: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }

    let matched = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    matched as f64 / terms.len() as f64
}

fn round_score(score: f64) -> f64 {
    (score * 1000.0).round() / 1000.0
}

fn is_compound_query(query: &str) -> bool {
    let lower = format!(" {} ", query.to_lowercase());
    lower.contains(" vs ")
        || lower.contains(" versus ")
        || lower.contains(" compare ")
        || lower.contains(" comparison ")
        || lower.contains(" and ")
        || lower.contains(" & ")
        || lower.contains(" + ")
        || query.contains("对比")
        || query.contains("比较")
        || query.contains("以及")
        || query.contains("同时")
        || query.contains(" 和 ")
        || query.contains(" 与 ")
}

fn build_decomposed_query_variant(query: &str) -> Option<String> {
    if !is_compound_query(query) {
        return None;
    }

    let mut variant = format!(" {query} ");
    for marker in [
        " vs ",
        " VS ",
        " versus ",
        " Versus ",
        " compare ",
        " comparison ",
        " and ",
        " AND ",
        " & ",
        " + ",
    ] {
        variant = variant.replace(marker, " ");
    }
    for marker in ["对比", "比较", "以及", "同时", " 和 ", " 与 "] {
        variant = variant.replace(marker, " ");
    }

    let variant = collapse_spaces(&variant);
    if variant.is_empty() || variant.eq_ignore_ascii_case(query.trim()) {
        None
    } else {
        Some(variant)
    }
}

fn collapse_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_vague(query: &str) -> bool {
    let lower = query.to_lowercase();
    normalized_terms(query).is_empty()
        || lower.contains("that previous")
        || lower.contains("previous decision")
        || lower.contains("that thing")
        || lower.contains("the thing")
        || lower.contains("earlier notes")
        || query.contains("之前")
        || query.contains("那个")
        || query.contains("模糊")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    let value = collapse_spaces(value.trim());
    if value.is_empty() {
        return;
    }
    if !values
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&value))
    {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Highlight;
    use uuid::Uuid;

    fn card(path: &str, title: &str, heading: &[&str], content: &str, score: f64) -> EvidenceCard {
        card_with_kind(path, title, heading, content, score, 0, "text")
    }

    fn card_with_kind(
        path: &str,
        title: &str,
        heading: &[&str],
        content: &str,
        score: f64,
        chunk_index: i64,
        chunk_kind: &str,
    ) -> EvidenceCard {
        EvidenceCard {
            chunk_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            source_name: "source".to_string(),
            document_path: path.to_string(),
            document_title: title.to_string(),
            chunk_index,
            chunk_kind: chunk_kind.to_string(),
            content: content.to_string(),
            heading_path: heading.iter().map(|h| h.to_string()).collect(),
            score,
            highlights: Vec::<Highlight>::new(),
            snippet: None,
            document_date: None,
            credibility: Some(0.8),
            freshness_days: None,
        }
    }

    #[test]
    fn contextual_text_includes_source_title_heading_and_content() {
        let c = card(
            "/notes/rag.md",
            "RAG Notes",
            &["Retrieval", "Reranking"],
            "Cross encoder reranking improves precision.",
            0.7,
        );

        let text = build_contextual_search_text(&c);

        assert!(text.contains("RAG Notes"));
        assert!(text.contains("/notes/rag.md"));
        assert!(text.contains("Retrieval > Reranking"));
        assert!(text.contains("Cross encoder reranking"));
    }

    #[test]
    fn confidence_is_low_for_empty_results() {
        let confidence = assess_retrieval_confidence(&[], "reranking evaluation");

        assert_eq!(confidence.level, RetrievalConfidenceLevel::Low);
        assert_eq!(confidence.score, 0.0);
        assert!(confidence.suggested_action.contains("second pass"));
    }

    #[test]
    fn confidence_is_high_for_strong_contextual_match() {
        let cards = vec![card(
            "https://example.com/rag",
            "RAG evaluation and reranking",
            &["Evaluation"],
            "RAG evaluation uses hit rate and reranking quality metrics.",
            0.92,
        )];

        let confidence = assess_retrieval_confidence(&cards, "RAG evaluation reranking");

        assert_eq!(confidence.level, RetrievalConfidenceLevel::High);
        assert!(confidence.score >= 0.7);
        assert!(confidence.suggested_action.contains("retrieve_evidence"));
    }

    #[test]
    fn reranker_promotes_contextual_title_and_heading_matches() {
        let mut cards = vec![
            card(
                "/notes/meeting.md",
                "Meeting Notes",
                &["General"],
                "This chunk mentions implementation details.",
                0.58,
            ),
            card(
                "/notes/rag.md",
                "RAG reranking plan",
                &["Evaluation"],
                "The section covers scoring and confidence thresholds.",
                0.52,
            ),
        ];

        rerank_evidence_cards(&mut cards, "RAG reranking confidence");

        assert_eq!(cards[0].document_title, "RAG reranking plan");
        assert!(cards[0].score > cards[1].score);
    }

    #[test]
    fn strategy_decomposes_compound_queries_and_enables_context_window() {
        let plan = plan_rag_strategy("GraphRAG vs RAPTOR retrieval quality", None);

        assert_eq!(plan.query_variants.len(), 2);
        assert_eq!(
            plan.query_variants[0],
            "GraphRAG vs RAPTOR retrieval quality"
        );
        assert!(plan.query_variants[1].contains("GraphRAG"));
        assert!(plan.query_variants[1].contains("RAPTOR"));
        assert!(plan.requires_context_window);
        assert!(plan.context_chunks >= 2);
    }

    #[test]
    fn strategy_enables_hyde_for_vague_or_low_confidence_queries() {
        let low = RetrievalConfidence {
            level: RetrievalConfidenceLevel::Low,
            score: 0.12,
            reasons: vec!["weak term coverage".to_string()],
            suggested_action: "Run a second pass".to_string(),
        };

        let plan = plan_rag_strategy("that previous decision", Some(&low));

        assert!(plan.use_hyde);
        assert!(plan
            .hyde_query
            .as_deref()
            .unwrap_or("")
            .contains("previous decision"));
        assert!(plan.second_pass_reason.is_some());
        assert!(plan.context_chunks >= 3);
    }

    #[test]
    fn rag_eval_report_computes_hit_rate_and_mrr() {
        let cases = vec![
            RagEvalCase {
                name: "hit first".to_string(),
                query: Some("alpha".to_string()),
                expected_chunk_ids: vec!["a".to_string()],
                retrieved_chunk_ids: vec!["a".to_string(), "b".to_string()],
                expected_sources: vec!["/notes/a.md".to_string()],
                retrieved_sources: vec!["/notes/a.md".to_string(), "/notes/b.md".to_string()],
                citation_supported: Some(true),
                top_k: 2,
                failure_notes: Vec::new(),
            },
            RagEvalCase {
                name: "hit second".to_string(),
                query: Some("charlie".to_string()),
                expected_chunk_ids: vec!["c".to_string()],
                retrieved_chunk_ids: vec!["b".to_string(), "c".to_string()],
                expected_sources: vec!["https://example.com/c".to_string()],
                retrieved_sources: vec![
                    "https://example.com/other".to_string(),
                    "https://example.com/c".to_string(),
                ],
                citation_supported: Some(true),
                top_k: 2,
                failure_notes: Vec::new(),
            },
            RagEvalCase {
                name: "miss".to_string(),
                query: Some("zulu".to_string()),
                expected_chunk_ids: vec!["z".to_string()],
                retrieved_chunk_ids: vec!["x".to_string()],
                expected_sources: vec!["/notes/z.md".to_string()],
                retrieved_sources: vec!["/notes/x.md".to_string()],
                citation_supported: Some(false),
                top_k: 2,
                failure_notes: vec!["retrieved unrelated source".to_string()],
            },
        ];

        let report = build_rag_eval_report(&cases);

        assert_eq!(report.case_count, 3);
        assert!((report.hit_rate_at_k - 2.0 / 3.0).abs() < f64::EPSILON);
        assert!((report.mean_reciprocal_rank - 0.5).abs() < f64::EPSILON);
        assert!((report.source_accuracy - 2.0 / 3.0).abs() < f64::EPSILON);
        assert!((report.citation_support_rate - 2.0 / 3.0).abs() < f64::EPSILON);
        assert_eq!(report.failures, vec!["miss"]);
        assert!(report
            .failure_notes
            .iter()
            .any(|note| note.contains("retrieved unrelated source")));
    }

    #[test]
    fn context_pack_places_direct_chunks_before_supporting_summaries() {
        let shared_doc = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let mut direct = card_with_kind(
            "/notes/rag.md",
            "RAG Plan",
            &["Retrieval"],
            "Direct chunk about context packing.",
            0.82,
            4,
            "text",
        );
        direct.document_id = shared_doc;
        direct.source_id = source_id;

        let mut summary = card_with_kind(
            "/notes/rag.md",
            "RAG Plan",
            &["Summary"],
            "Compiled summary about context packing.",
            0.95,
            -1,
            "summary",
        );
        summary.document_id = shared_doc;
        summary.source_id = source_id;

        let mut secondary = card_with_kind(
            "https://example.com/rag",
            "External RAG Notes",
            &["Notes"],
            "Secondary source about GraphRAG.",
            0.74,
            2,
            "text",
        );
        secondary.source_name = "web".to_string();

        let pack = build_context_pack(&[summary.clone(), secondary, direct.clone()], 2);

        assert_eq!(pack.primary_chunk_ids, vec![direct.chunk_id.to_string()]);
        assert_eq!(
            pack.supporting_chunk_ids,
            vec![summary.chunk_id.to_string()]
        );
        assert_eq!(pack.groups[0].document_id, shared_doc.to_string());
        assert_eq!(pack.groups[0].role, "primary_source");
        assert_eq!(pack.groups[0].chunks[0].role, "primary_direct");
        assert_eq!(pack.groups[0].chunks[1].role, "supporting_summary");
        assert!(pack
            .ordering_policy
            .contains("direct chunks before supporting summaries"));
    }

    #[test]
    fn saved_benchmark_suite_covers_local_web_and_multihop_cases() {
        let suite = saved_rag_benchmark_suite();
        let source_kinds = suite
            .iter()
            .map(|case| case.expected_source_kind.as_str())
            .collect::<Vec<_>>();

        assert!(source_kinds.contains(&"local_file"));
        assert!(source_kinds.contains(&"web_page"));
        assert!(suite
            .iter()
            .any(|case| case.tags.contains(&"multi_hop".to_string())));
    }
}
