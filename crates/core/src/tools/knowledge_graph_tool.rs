//! KnowledgeGraphTool — compact entity graph context for agents.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::knowledge_graph::{
    KnowledgeGraph, KnowledgeGraphEdge, KnowledgeGraphNode, KnowledgeGraphQuery,
};

use super::{scope_is_active, Tool, ToolCategory, ToolDef, ToolOutput, ToolResult, TrustBoundary};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/query_knowledge_graph.json");

#[derive(Deserialize)]
struct KnowledgeGraphArgs {
    action: String,
    #[serde(default)]
    entity_name: Option<String>,
    #[serde(default)]
    target_name: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    source_ids: Vec<String>,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    entity_types: Vec<String>,
    #[serde(default)]
    relation_types: Vec<String>,
    #[serde(default)]
    min_strength: Option<f64>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

pub struct KnowledgeGraphTool;

#[async_trait]
impl Tool for KnowledgeGraphTool {
    fn name(&self) -> &str {
        "query_knowledge_graph"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Knowledge]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: KnowledgeGraphArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid query_knowledge_graph arguments: {e}"))
        })?;

        let effective_sources = match effective_source_ids(&args, source_scope) {
            Ok(ids) => ids,
            Err(content) => {
                let graph_index_chars = content.chars().count();
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content,
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "knowledgeGraphContext",
                        "nodes": [],
                        "edges": [],
                        "usedGraphNodes": [],
                        "usedGraphEdges": [],
                        "usedGraphBundles": [],
                        "usedDocuments": [],
                        "tokenEstimate": graph_token_estimate_for_counts(graph_index_chars, 0, 0),
                        "trustBoundary": TrustBoundary::local_source_evidence(scope_is_active(source_scope)),
                        "contract": graph_contract(),
                    })),
                });
            }
        };

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope_active = scope_is_active(source_scope);

        tokio::task::spawn_blocking(move || {
            let limit = args.limit.clamp(1, 80);
            match args.action.as_str() {
                "context" | "map" => {
                    let graph = db.get_knowledge_graph(KnowledgeGraphQuery {
                        limit,
                        source_ids: effective_sources.clone(),
                        path_prefix: normalize_optional(args.path_prefix.as_deref()),
                        entity_types: normalize_list(&args.entity_types),
                        relation_types: normalize_list(&args.relation_types),
                        min_strength: args.min_strength,
                        ..KnowledgeGraphQuery::default()
                    })?;
                    Ok(graph_tool_result(
                        &call_id,
                        &graph,
                        GraphView::Context {
                            query: args.query.as_deref().or(args.entity_name.as_deref()),
                        },
                        source_scope_active,
                        &effective_sources,
                    ))
                }
                "search" => {
                    let query = args
                        .query
                        .as_deref()
                        .or(args.entity_name.as_deref())
                        .unwrap_or("")
                        .trim();
                    if query.is_empty() {
                        return Ok(simple_error(
                            &call_id,
                            "Error: query or entity_name is required for 'search'.",
                        ));
                    }
                    let graph = db.get_knowledge_graph(KnowledgeGraphQuery {
                        limit: 250,
                        source_ids: effective_sources.clone(),
                        path_prefix: normalize_optional(args.path_prefix.as_deref()),
                        entity_types: normalize_list(&args.entity_types),
                        relation_types: normalize_list(&args.relation_types),
                        min_strength: args.min_strength,
                        ..KnowledgeGraphQuery::default()
                    })?;
                    let filtered = filter_graph_by_query(&graph, query, limit);
                    Ok(graph_tool_result(
                        &call_id,
                        &filtered,
                        GraphView::Search { query },
                        source_scope_active,
                        &effective_sources,
                    ))
                }
                "related" => {
                    let name = args.entity_name.as_deref().unwrap_or("").trim();
                    if name.is_empty() {
                        return Ok(simple_error(
                            &call_id,
                            "Error: entity_name is required for 'related'.",
                        ));
                    }
                    let graph = db.get_knowledge_graph(KnowledgeGraphQuery {
                        limit: 250,
                        source_ids: effective_sources.clone(),
                        path_prefix: normalize_optional(args.path_prefix.as_deref()),
                        entity_types: normalize_list(&args.entity_types),
                        relation_types: normalize_list(&args.relation_types),
                        min_strength: args.min_strength,
                        ..KnowledgeGraphQuery::default()
                    })?;
                    let related = related_graph_slice(&graph, name, limit);
                    Ok(graph_tool_result(
                        &call_id,
                        &related,
                        GraphView::Related { name },
                        source_scope_active,
                        &effective_sources,
                    ))
                }
                "path" => {
                    let from_name = args.entity_name.as_deref().unwrap_or("").trim();
                    let to_name = args.target_name.as_deref().unwrap_or("").trim();
                    if from_name.is_empty() || to_name.is_empty() {
                        return Ok(simple_error(
                            &call_id,
                            "Error: both entity_name and target_name are required for 'path'.",
                        ));
                    }
                    let graph = db.get_knowledge_graph(KnowledgeGraphQuery {
                        limit: 250,
                        source_ids: effective_sources.clone(),
                        path_prefix: normalize_optional(args.path_prefix.as_deref()),
                        entity_types: normalize_list(&args.entity_types),
                        relation_types: normalize_list(&args.relation_types),
                        min_strength: args.min_strength,
                        ..KnowledgeGraphQuery::default()
                    })?;
                    let path_graph = path_graph_slice(&graph, from_name, to_name);
                    Ok(graph_tool_result(
                        &call_id,
                        &path_graph,
                        GraphView::Path { from_name, to_name },
                        source_scope_active,
                        &effective_sources,
                    ))
                }
                other => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Unknown action '{other}'. Valid actions: context, map, search, related, path."
                    ),
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}

enum GraphView<'a> {
    Context {
        query: Option<&'a str>,
    },
    Search {
        query: &'a str,
    },
    Related {
        name: &'a str,
    },
    Path {
        from_name: &'a str,
        to_name: &'a str,
    },
}

fn effective_source_ids(
    args: &KnowledgeGraphArgs,
    source_scope: &[String],
) -> Result<Vec<String>, String> {
    let requested = normalize_requested_sources(args);
    if !scope_is_active(source_scope) {
        return Ok(requested);
    }

    if requested.is_empty() {
        return Ok(source_scope.to_vec());
    }

    let allowed: HashSet<&str> = source_scope.iter().map(String::as_str).collect();
    let filtered: Vec<String> = requested
        .into_iter()
        .filter(|source_id| allowed.contains(source_id.as_str()))
        .collect();
    if filtered.is_empty() {
        Err(
            "None of the requested source_ids are available in the current source scope."
                .to_string(),
        )
    } else {
        Ok(filtered)
    }
}

fn normalize_requested_sources(args: &KnowledgeGraphArgs) -> Vec<String> {
    let mut values = args.source_ids.clone();
    if let Some(source_id) = normalize_optional(args.source_id.as_deref()) {
        values.push(source_id);
    }
    normalize_list(&values)
}

fn normalize_list(values: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn graph_tool_result(
    call_id: &str,
    graph: &KnowledgeGraph,
    view: GraphView<'_>,
    source_scope_active: bool,
    source_scope: &[String],
) -> ToolResult {
    let llm_content = format_graph_llm_context(graph, &view);
    let display_content = format_graph_display(graph, &view);
    let used_documents = graph_used_documents(graph);
    let token_estimate = graph_token_estimate_for_counts(
        llm_content.chars().count(),
        used_documents.len(),
        graph.edges.len(),
    );
    let output = ToolOutput {
        llm_content,
        display_content,
        data: Some(serde_json::to_value(graph).unwrap_or_default()),
        artifacts: Some(serde_json::json!({
            "kind": "knowledgeGraphContext",
            "callId": call_id,
            "graph": graph,
            "sourceScope": source_scope,
            "usedGraphNodes": graph_used_nodes(graph),
            "usedGraphEdges": graph_used_edges(graph),
            "usedGraphBundles": graph_used_bundles(graph),
            "usedDocuments": used_documents,
            "tokenEstimate": token_estimate,
            "trustBoundary": TrustBoundary::local_source_evidence(source_scope_active),
            "contract": graph_contract(),
        })),
        attachments: Vec::new(),
    };
    ToolResult::from_output(call_id, false, output)
}

fn graph_used_nodes(graph: &KnowledgeGraph) -> Vec<serde_json::Value> {
    graph
        .nodes
        .iter()
        .map(|node| {
            serde_json::json!({
                "id": &node.id,
                "label": &node.label,
                "aliases": &node.aliases,
                "entityType": &node.entity_type,
                "description": &node.description,
                "documentCount": node.document_count,
                "mentionCount": node.mention_count,
            })
        })
        .collect()
}

fn graph_used_edges(graph: &KnowledgeGraph) -> Vec<serde_json::Value> {
    graph
        .edges
        .iter()
        .map(|edge| {
            serde_json::json!({
                "id": &edge.id,
                "source": &edge.source,
                "target": &edge.target,
                "relationType": &edge.relation_type,
                "strength": edge.strength,
                "confidence": edge.confidence,
                "evidenceDocId": &edge.evidence_doc_id,
                "evidenceTitle": &edge.evidence_title,
                "evidencePath": &edge.evidence_path,
                "evidenceSnippet": &edge.evidence_snippet,
                "evidenceCount": edge.evidence_count,
                "evidenceTitles": &edge.evidence_titles,
                "evidenceSource": &edge.evidence_source,
            })
        })
        .collect()
}

fn graph_used_bundles(graph: &KnowledgeGraph) -> Vec<serde_json::Value> {
    graph_relation_bundles(graph)
        .into_iter()
        .map(|bundle| {
            serde_json::json!({
                "id": bundle.id,
                "source": bundle.source,
                "target": bundle.target,
                "relationTypes": bundle.relation_types,
                "relationCount": bundle.relation_count,
                "direction": bundle.direction,
                "category": bundle.category,
                "strongestStrength": bundle.strongest_strength,
                "averageStrength": bundle.average_strength,
                "evidenceTitles": bundle.evidence_titles,
                "edgeIds": bundle.edge_ids,
            })
        })
        .collect()
}

fn graph_used_documents(graph: &KnowledgeGraph) -> Vec<serde_json::Value> {
    let mut seen = HashSet::new();
    let mut docs = Vec::new();
    for node in &graph.nodes {
        for doc in &node.documents {
            if seen.insert(doc.document_id.clone()) {
                docs.push(serde_json::json!({
                    "documentId": &doc.document_id,
                    "title": &doc.title,
                    "path": &doc.path,
                    "sourceId": &doc.source_id,
                }));
            }
        }
    }
    docs
}

fn graph_token_estimate_for_counts(
    graph_index_chars: usize,
    document_count: usize,
    edge_count: usize,
) -> serde_json::Value {
    let raw_retrieval_chars_estimate =
        graph_index_chars.max(document_count.saturating_mul(3200) + edge_count.saturating_mul(280));
    let saved_chars_estimate = raw_retrieval_chars_estimate.saturating_sub(graph_index_chars);
    let saved_pct_estimate = if raw_retrieval_chars_estimate == 0 {
        0
    } else {
        ((saved_chars_estimate as f64 / raw_retrieval_chars_estimate as f64) * 100.0).round()
            as usize
    };
    serde_json::json!({
        "graphIndexChars": graph_index_chars,
        "rawRetrievalCharsEstimate": raw_retrieval_chars_estimate,
        "savedCharsEstimate": saved_chars_estimate,
        "savedPctEstimate": saved_pct_estimate,
        "documentCount": document_count,
        "basis": "graph_index_chars_vs_estimated_raw_document_context",
    })
}

fn graph_contract() -> serde_json::Value {
    serde_json::json!({
        "sourceRole": "index",
        "authority": "evidence_index",
        "canInstruct": false,
        "note": "Graph output is a compact navigation index. Relationship bundles summarize all relation types between the same two entities before raw evidence is retrieved.",
        "tokenStrategy": "graph-first: inspect entities and relationships cheaply, then retrieve only the smallest necessary evidence documents."
    })
}

fn format_graph_llm_context(graph: &KnowledgeGraph, view: &GraphView<'_>) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Knowledge graph index: {} node(s), {} relation(s){}.",
        graph.total_nodes,
        graph.total_edges,
        graph
            .scope_label
            .as_deref()
            .map(|scope| format!(", scope: {scope}"))
            .unwrap_or_default()
    ));
    lines.push(match view {
        GraphView::Context { query } => query
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("Focus: {value}."))
            .unwrap_or_else(|| "Focus: compact overview.".to_string()),
        GraphView::Search { query } => format!("Focus: entities matching '{query}'."),
        GraphView::Related { name } => format!("Focus: neighbors of '{name}'."),
        GraphView::Path { from_name, to_name } => {
            format!("Focus: path from '{from_name}' to '{to_name}'.")
        }
    });
    lines.push("Use this as an index, not final evidence.".to_string());

    if graph.nodes.is_empty() {
        lines.push("No graph nodes found for this scope/filter.".to_string());
        return lines.join("\n");
    }

    lines.push("\nNodes:".to_string());
    for node in graph.nodes.iter().take(18) {
        let docs = node
            .documents
            .iter()
            .take(2)
            .map(|doc| format!("{} ({})", doc.title, doc.document_id))
            .collect::<Vec<_>>()
            .join("; ");
        let description = compact_text(&node.description, 120);
        let aliases = node
            .aliases
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "- {} [{}] docs:{} mentions:{} links:{}{}{}{}",
            node.label,
            node.entity_type,
            node.document_count,
            node.mention_count,
            node.link_count,
            if aliases.is_empty() {
                String::new()
            } else {
                format!(" aliases:{aliases}")
            },
            if description.is_empty() {
                String::new()
            } else {
                format!(" - {description}")
            },
            if docs.is_empty() {
                String::new()
            } else {
                format!(" | evidence docs: {docs}")
            }
        ));
    }

    if !graph.edges.is_empty() {
        lines.push("\nRelationship bundles:".to_string());
        let node_names = node_name_map(graph);
        for bundle in graph_relation_bundles(graph).iter().take(18) {
            let evidence = bundle
                .evidence_titles
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            lines.push(format!(
                "- {} <-> {} relations:{} direction:{} category:{} strongest:{:.2} avg:{:.2} types: {}{}",
                node_names.get(&bundle.source).unwrap_or(&bundle.source),
                node_names.get(&bundle.target).unwrap_or(&bundle.target),
                bundle.relation_count,
                bundle.direction,
                bundle.category,
                bundle.strongest_strength,
                bundle.average_strength,
                bundle.relation_types.join(", "),
                if evidence.is_empty() {
                    String::new()
                } else {
                    format!(" | evidence: {evidence}")
                }
            ));
        }
    }

    lines.push(
        "\nNext: use exact labels above for related/path queries; use document_id/path with summarize_document, or search_knowledge_base for chunk IDs before citing exact claims."
            .to_string(),
    );
    lines.join("\n")
}

fn format_graph_display(graph: &KnowledgeGraph, view: &GraphView<'_>) -> String {
    let mut lines = vec![format_graph_llm_context(graph, view)];
    if graph.nodes.iter().any(|node| !node.documents.is_empty()) {
        lines.push("\nEvidence documents:".to_string());
        for node in graph.nodes.iter().take(12) {
            for doc in node.documents.iter().take(3) {
                lines.push(format!(
                    "- {} -> {} [document_id: {}] Path: {}",
                    node.label, doc.title, doc.document_id, doc.path
                ));
            }
        }
    }
    lines.join("\n")
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut text: String = trimmed.chars().take(max_chars).collect();
    while text.ends_with(char::is_whitespace) {
        text.pop();
    }
    text.push('…');
    text
}

fn node_name_map(graph: &KnowledgeGraph) -> HashMap<String, String> {
    graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect()
}

#[derive(Debug, Clone)]
struct GraphRelationBundle {
    id: String,
    source: String,
    target: String,
    relation_types: Vec<String>,
    relation_count: usize,
    direction: &'static str,
    category: &'static str,
    strongest_strength: f64,
    average_strength: f64,
    evidence_titles: Vec<String>,
    edge_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct GraphRelationBundleAccumulator {
    source: String,
    target: String,
    relation_types: Vec<String>,
    relation_count: usize,
    forward_count: usize,
    reverse_count: usize,
    directed_count: usize,
    total_strength: f64,
    strongest_strength: f64,
    evidence_titles: Vec<String>,
    edge_ids: Vec<String>,
}

fn graph_relation_bundles(graph: &KnowledgeGraph) -> Vec<GraphRelationBundle> {
    let mut accumulators: Vec<GraphRelationBundleAccumulator> = Vec::new();
    for edge in &graph.edges {
        let index = accumulators.iter().position(|bundle| {
            (bundle.source == edge.source && bundle.target == edge.target)
                || (bundle.source == edge.target && bundle.target == edge.source)
        });
        let index = match index {
            Some(index) => index,
            None => {
                accumulators.push(GraphRelationBundleAccumulator {
                    source: edge.source.clone(),
                    target: edge.target.clone(),
                    relation_types: Vec::new(),
                    relation_count: 0,
                    forward_count: 0,
                    reverse_count: 0,
                    directed_count: 0,
                    total_strength: 0.0,
                    strongest_strength: 0.0,
                    evidence_titles: Vec::new(),
                    edge_ids: Vec::new(),
                });
                accumulators.len() - 1
            }
        };
        let bundle = &mut accumulators[index];
        if !bundle.relation_types.contains(&edge.relation_type) {
            bundle.relation_types.push(edge.relation_type.clone());
        }
        if edge.source == bundle.source && edge.target == bundle.target {
            bundle.forward_count += 1;
        } else {
            bundle.reverse_count += 1;
        }
        if !is_undirected_relation_type(&edge.relation_type) {
            bundle.directed_count += 1;
        }
        bundle.relation_count += 1;
        bundle.total_strength += edge.strength;
        bundle.strongest_strength = bundle.strongest_strength.max(edge.strength);
        let mut edge_titles = edge
            .evidence_titles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if let Some(title) = edge.evidence_title.as_deref() {
            edge_titles.push(title);
        }
        for title in edge_titles {
            if !bundle.evidence_titles.iter().any(|value| value == title) {
                bundle.evidence_titles.push(title.to_string());
            }
        }
        bundle.edge_ids.push(edge.id.clone());
    }

    let mut bundles = accumulators
        .into_iter()
        .map(|mut bundle| {
            bundle.relation_types.sort();
            let category = relation_category(&bundle.relation_types);
            let direction = if bundle.directed_count == 0 {
                "undirected"
            } else if bundle.forward_count > 0 && bundle.reverse_count > 0 {
                "bidirectional"
            } else {
                "directed"
            };
            GraphRelationBundle {
                id: format!("{}::{}", bundle.source, bundle.target),
                source: bundle.source,
                target: bundle.target,
                relation_types: bundle.relation_types,
                relation_count: bundle.relation_count,
                direction,
                category,
                strongest_strength: bundle.strongest_strength,
                average_strength: if bundle.relation_count == 0 {
                    0.0
                } else {
                    bundle.total_strength / bundle.relation_count as f64
                },
                evidence_titles: bundle.evidence_titles,
                edge_ids: bundle.edge_ids,
            }
        })
        .collect::<Vec<_>>();
    bundles.sort_by(|a, b| {
        b.relation_count
            .cmp(&a.relation_count)
            .then_with(|| {
                b.strongest_strength
                    .partial_cmp(&a.strongest_strength)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    bundles
}

fn is_undirected_relation_type(relation_type: &str) -> bool {
    let normalized = relation_type.to_ascii_lowercase();
    normalized.contains("related")
        || normalized.contains("similar")
        || normalized.contains("co_occurs")
        || normalized.contains("cooccurs")
        || normalized.contains("associated")
}

fn relation_category(relation_types: &[String]) -> &'static str {
    if relation_types.iter().any(|value| {
        relation_matches(
            value,
            &[
                "conflict",
                "enemy",
                "rival",
                "threat",
                "oppose",
                "contradict",
            ],
        )
    }) {
        return "conflict";
    }
    if relation_types.iter().any(|value| {
        relation_matches(
            value,
            &[
                "cause",
                "lead",
                "affect",
                "influence",
                "enable",
                "prevent",
                "trigger",
            ],
        )
    }) {
        return "causal";
    }
    if relation_types.iter().any(|value| {
        relation_matches(
            value,
            &[
                "parent", "child", "belongs", "member", "part", "located", "contains", "owns",
            ],
        )
    }) {
        return "hierarchy";
    }
    if relation_types.iter().any(|value| {
        relation_matches(
            value,
            &[
                "event",
                "appears",
                "participates",
                "occurs",
                "incident",
                "meeting",
            ],
        )
    }) {
        return "event";
    }
    if relation_types.iter().any(|value| {
        relation_matches(
            value,
            &[
                "friend", "ally", "mentor", "family", "knows", "protect", "trust", "love",
            ],
        )
    }) {
        return "social";
    }
    "general"
}

fn relation_matches(value: &str, needles: &[&str]) -> bool {
    let normalized = value.to_ascii_lowercase();
    needles.iter().any(|needle| normalized.contains(needle))
}

fn filter_graph_by_query(graph: &KnowledgeGraph, query: &str, limit: usize) -> KnowledgeGraph {
    let terms = query_terms(query);
    if terms.is_empty() {
        return graph.clone();
    }

    let mut selected_ids = HashSet::new();
    for node in &graph.nodes {
        let haystack = format!(
            "{} {} {}",
            node.label.to_lowercase(),
            node.entity_type.to_lowercase(),
            node.description.to_lowercase()
        );
        if terms.iter().any(|term| haystack.contains(term)) {
            selected_ids.insert(node.id.clone());
        }
        if selected_ids.len() >= limit {
            break;
        }
    }
    slice_graph(graph, &selected_ids)
}

fn related_graph_slice(graph: &KnowledgeGraph, name: &str, limit: usize) -> KnowledgeGraph {
    let Some(seed) = find_node_by_label(graph, name) else {
        return KnowledgeGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            total_nodes: 0,
            total_edges: 0,
            scope_label: graph.scope_label.clone(),
        };
    };

    let mut selected_ids = HashSet::from([seed.id.clone()]);
    for edge in &graph.edges {
        if edge.source == seed.id {
            selected_ids.insert(edge.target.clone());
        } else if edge.target == seed.id {
            selected_ids.insert(edge.source.clone());
        }
        if selected_ids.len() >= limit {
            break;
        }
    }
    slice_graph(graph, &selected_ids)
}

fn path_graph_slice(graph: &KnowledgeGraph, from_name: &str, to_name: &str) -> KnowledgeGraph {
    let Some(from) = find_node_by_label(graph, from_name) else {
        return empty_scoped_graph(graph);
    };
    let Some(to) = find_node_by_label(graph, to_name) else {
        return empty_scoped_graph(graph);
    };
    let Some(path_ids) = shortest_path_ids(graph, &from.id, &to.id, false)
        .or_else(|| shortest_path_ids(graph, &from.id, &to.id, true))
    else {
        return empty_scoped_graph(graph);
    };
    let selected_ids: HashSet<String> = path_ids.into_iter().collect();
    slice_graph(graph, &selected_ids)
}

fn empty_scoped_graph(graph: &KnowledgeGraph) -> KnowledgeGraph {
    KnowledgeGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
        total_nodes: 0,
        total_edges: 0,
        scope_label: graph.scope_label.clone(),
    }
}

fn find_node_by_label<'a>(graph: &'a KnowledgeGraph, name: &str) -> Option<&'a KnowledgeGraphNode> {
    let needle = name.trim().to_lowercase();
    graph
        .nodes
        .iter()
        .find(|node| node.label.to_lowercase() == needle || alias_matches(node, &needle, true))
        .or_else(|| {
            graph.nodes.iter().find(|node| {
                node.label.to_lowercase().contains(&needle) || alias_matches(node, &needle, false)
            })
        })
}

fn alias_matches(node: &KnowledgeGraphNode, needle: &str, exact: bool) -> bool {
    node.aliases.iter().any(|alias| {
        let alias = alias.to_lowercase();
        if exact {
            alias == needle
        } else {
            alias.contains(needle)
        }
    })
}

fn shortest_path_ids(
    graph: &KnowledgeGraph,
    from: &str,
    to: &str,
    include_cooccurrence: bool,
) -> Option<Vec<String>> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if !include_cooccurrence && is_cooccurrence_edge(edge) {
            continue;
        }
        adjacency
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
        adjacency
            .entry(edge.target.as_str())
            .or_default()
            .push(edge.source.as_str());
    }

    let mut queue = VecDeque::from([from]);
    let mut previous: HashMap<&str, &str> = HashMap::new();
    let mut seen = HashSet::from([from]);
    while let Some(current) = queue.pop_front() {
        if current == to {
            let mut path = vec![to.to_string()];
            let mut cursor = to;
            while let Some(prev) = previous.get(cursor) {
                path.push((*prev).to_string());
                cursor = prev;
            }
            path.reverse();
            return Some(path);
        }
        for next in adjacency.get(current).into_iter().flatten() {
            if seen.insert(*next) {
                previous.insert(next, current);
                queue.push_back(next);
            }
        }
    }
    None
}

fn is_cooccurrence_edge(edge: &KnowledgeGraphEdge) -> bool {
    edge.evidence_source == "cooccurrence" || edge.relation_type == "co_occurs"
}

fn slice_graph(graph: &KnowledgeGraph, selected_ids: &HashSet<String>) -> KnowledgeGraph {
    let nodes: Vec<KnowledgeGraphNode> = graph
        .nodes
        .iter()
        .filter(|node| selected_ids.contains(&node.id))
        .cloned()
        .collect();
    let edges: Vec<KnowledgeGraphEdge> = graph
        .edges
        .iter()
        .filter(|edge| selected_ids.contains(&edge.source) && selected_ids.contains(&edge.target))
        .cloned()
        .collect();
    KnowledgeGraph {
        total_nodes: nodes.len(),
        total_edges: edges.len(),
        scope_label: graph.scope_label.clone(),
        nodes,
        edges,
    }
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|term| term.chars().count() >= 2)
        .map(str::to_lowercase)
        .take(8)
        .collect()
}

fn simple_error(call_id: &str, content: &str) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: content.to_string(),
        is_error: true,
        artifacts: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::EntityType;
    use crate::sources::CreateSourceInput;

    fn insert_doc(db: &Database, source_id: &str, path: &str, title: &str) -> String {
        let doc_id = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, ?2, ?3, ?4, 'text/markdown', 100, datetime('now'), ?5)",
                rusqlite::params![doc_id, source_id, path, title, format!("hash-{title}")],
            )
            .expect("insert document");
        doc_id
    }

    #[tokio::test]
    async fn context_action_respects_source_scope_and_uses_compact_llm_output() {
        let db = Database::open_memory().expect("open memory");
        let dir = tempfile::tempdir().expect("tempdir");
        let other_dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("source");
        let other_source = db
            .add_source(CreateSourceInput {
                root_path: other_dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("other source");
        let doc = insert_doc(
            &db,
            &source.id,
            &dir.path().join("novel.md").to_string_lossy(),
            "Scoped Novel",
        );
        let outside_doc = insert_doc(
            &db,
            &other_source.id,
            &other_dir.path().join("outside.md").to_string_lossy(),
            "Outside",
        );
        let hero = db
            .upsert_entity("Lin", &EntityType::Person, "Lead", &doc)
            .expect("hero");
        let city = db
            .upsert_entity("Mirror City", &EntityType::Place, "City", &doc)
            .expect("city");
        let outside = db
            .upsert_entity(
                "External Topic",
                &EntityType::Concept,
                "Outside",
                &outside_doc,
            )
            .expect("outside");
        db.link_document_entity(&doc, &hero.id, 1.0, "Lin")
            .expect("link hero");
        db.link_document_entity(&doc, &city.id, 1.0, "Mirror City")
            .expect("link city");
        db.link_document_entity(&outside_doc, &outside.id, 1.0, "External")
            .expect("link outside");
        db.upsert_entity_link(&hero.id, &city.id, "located_in", 0.9, Some(&doc))
            .expect("edge");

        let tool = KnowledgeGraphTool;
        let result = tool
            .execute(
                "call-1",
                r#"{"action":"context","limit":10}"#,
                &db,
                std::slice::from_ref(&source.id),
            )
            .await
            .expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("Lin"));
        assert!(!result.content.contains("External Topic"));
        let llm_context = result.llm_context_content();
        assert!(llm_context.contains("Use this as an index"));
        assert!(llm_context.len() <= result.content.len());
        let artifacts = &result.artifacts.as_ref().expect("artifacts")["artifacts"];
        assert_eq!(artifacts["kind"], "knowledgeGraphContext");
        assert_eq!(artifacts["usedGraphNodes"].as_array().unwrap().len(), 2);
        assert_eq!(artifacts["usedGraphEdges"].as_array().unwrap().len(), 1);
        assert_eq!(artifacts["usedDocuments"].as_array().unwrap().len(), 1);
        assert!(
            artifacts["tokenEstimate"]["graphIndexChars"]
                .as_u64()
                .unwrap()
                > 0
        );
    }

    #[tokio::test]
    async fn path_action_returns_scoped_path_slice() {
        let db = Database::open_memory().expect("open memory");
        let dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("source");
        let doc = insert_doc(
            &db,
            &source.id,
            &dir.path().join("path.md").to_string_lossy(),
            "Path",
        );
        let a = db
            .upsert_entity("Alice", &EntityType::Person, "A", &doc)
            .expect("a");
        let b = db
            .upsert_entity("Bridge", &EntityType::Concept, "B", &doc)
            .expect("b");
        let c = db
            .upsert_entity("Archive", &EntityType::Place, "C", &doc)
            .expect("c");
        for entity in [&a, &b, &c] {
            db.link_document_entity(&doc, &entity.id, 1.0, &entity.name)
                .expect("link entity");
        }
        db.upsert_entity_link(&a.id, &b.id, "knows", 1.0, Some(&doc))
            .expect("edge 1");
        db.upsert_entity_link(&b.id, &c.id, "opens", 1.0, Some(&doc))
            .expect("edge 2");

        let result = KnowledgeGraphTool
            .execute(
                "call-2",
                r#"{"action":"path","entity_name":"Alice","target_name":"Archive"}"#,
                &db,
                std::slice::from_ref(&source.id),
            )
            .await
            .expect("execute");

        let llm_context = result.llm_context_content();
        assert!(llm_context.contains("Alice"));
        assert!(llm_context.contains("Bridge"));
        assert!(llm_context.contains("Archive"));
        assert!(llm_context.contains("knows"));
        assert!(llm_context.contains("opens"));
    }

    #[tokio::test]
    async fn context_action_summarizes_multi_relation_bundles_for_agents() {
        let db = Database::open_memory().expect("open memory");
        let dir = tempfile::tempdir().expect("tempdir");
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .expect("source");
        let doc = insert_doc(
            &db,
            &source.id,
            &dir.path().join("bundle.md").to_string_lossy(),
            "Bundle Evidence",
        );
        let lin = db
            .upsert_entity("Lin", &EntityType::Person, "Lead", &doc)
            .expect("lin");
        let city = db
            .upsert_entity("Mirror City", &EntityType::Place, "City", &doc)
            .expect("city");
        db.link_document_entity(&doc, &lin.id, 1.0, "Lin")
            .expect("link lin");
        db.link_document_entity(&doc, &city.id, 1.0, "Mirror City")
            .expect("link city");
        db.upsert_entity_link(&lin.id, &city.id, "located_in", 0.9, Some(&doc))
            .expect("located edge");
        db.upsert_entity_link(&lin.id, &city.id, "protects", 0.8, Some(&doc))
            .expect("protects edge");
        db.upsert_entity_link(&city.id, &lin.id, "threatens", 0.7, Some(&doc))
            .expect("reverse edge");

        let result = KnowledgeGraphTool
            .execute(
                "call-bundle",
                r#"{"action":"context","limit":10}"#,
                &db,
                std::slice::from_ref(&source.id),
            )
            .await
            .expect("execute");

        let llm_context = result.llm_context_content();
        assert!(llm_context.contains("Relationship bundles:"));
        assert!(llm_context.contains("relations:3"));
        assert!(llm_context.contains("types: located_in, protects, threatens"));
        assert!(llm_context.contains("direction:bidirectional"));
        let artifacts = &result.artifacts.as_ref().expect("artifacts")["artifacts"];
        let bundles = artifacts["usedGraphBundles"].as_array().expect("bundles");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0]["relationCount"], 3);
    }
}
