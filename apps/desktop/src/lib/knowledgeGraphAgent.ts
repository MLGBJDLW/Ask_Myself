import type {
  KnowledgeGraphDocumentRef,
  KnowledgeGraphEdge,
  KnowledgeGraphNode,
} from '../types/knowledge';
import type { Conversation } from '../types/conversation';
import { buildRelationBundles } from './knowledgeGraphRelations';
import type { KnowledgeGraphRelationBundle, RelationCategory, RelationDirection } from './knowledgeGraphRelations';

export const GRAPH_AGENT_CONTEXT_STORAGE_KEY = 'nexa-graph-agent-context-v1';
export const GRAPH_AGENT_USAGE_STORAGE_KEY = 'nexa-agent-graph-usage-v1';
export const GRAPH_AGENT_CONTEXT_EVENT = 'nexa:graph-agent-context';
export const GRAPH_AGENT_USAGE_EVENT = 'nexa:agent-graph-usage';

export interface GraphAgentNodeRef {
  id: string;
  label: string;
  entityType?: string;
  description?: string;
  documentCount?: number;
  mentionCount?: number;
}

export interface GraphAgentEdgeRef {
  id: string;
  source: string;
  target: string;
  relationType: string;
  relationCategory?: RelationCategory;
  strength?: number;
  evidenceDocId?: string | null;
  evidenceTitle?: string | null;
  evidencePath?: string | null;
  otherLabel?: string | null;
}

export interface GraphAgentRelationBundleRef {
  id: string;
  source: string;
  target: string;
  sourceLabel?: string | null;
  targetLabel?: string | null;
  otherLabel?: string | null;
  relationTypes: string[];
  relationCount: number;
  direction: RelationDirection;
  category: RelationCategory;
  strongestStrength: number;
  averageStrength: number;
  evidenceTitles: string[];
  edgeIds: string[];
}

export interface GraphTokenEstimate {
  graphIndexChars: number;
  rawRetrievalCharsEstimate: number;
  savedCharsEstimate: number;
  savedPctEstimate: number;
  documentCount: number;
  basis: string;
}

export interface GraphAgentContext {
  id: string;
  createdAt: string;
  sourceId: string | null;
  sourceLabel: string | null;
  pathPrefix: string | null;
  scopeLabel: string | null;
  focusLabel?: string | null;
  focusKind?: 'node' | 'bundle';
  node: GraphAgentNodeRef;
  edges: GraphAgentEdgeRef[];
  relationBundles: GraphAgentRelationBundleRef[];
  documents: KnowledgeGraphDocumentRef[];
  tokenEstimate: GraphTokenEstimate;
}

export interface GraphAgentUsage {
  id: string;
  createdAt: string;
  sourceScope?: string[] | null;
  scopeLabel?: string | null;
  usedGraphNodes: GraphAgentNodeRef[];
  usedGraphEdges: GraphAgentEdgeRef[];
  usedGraphBundles?: GraphAgentRelationBundleRef[];
  usedDocuments: KnowledgeGraphDocumentRef[];
  tokenEstimate?: GraphTokenEstimate | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function numberOr(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function stringOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function parseNodeRef(value: unknown): GraphAgentNodeRef | null {
  if (!isRecord(value)) return null;
  const id = stringOrNull(value.id);
  const label = stringOrNull(value.label);
  if (!id || !label) return null;
  return {
    id,
    label,
    entityType: stringOrNull(value.entityType) ?? undefined,
    description: stringOrNull(value.description) ?? undefined,
    documentCount: optionalNumber(value.documentCount),
    mentionCount: optionalNumber(value.mentionCount),
  };
}

function parseEdgeRef(value: unknown): GraphAgentEdgeRef | null {
  if (!isRecord(value)) return null;
  const id = stringOrNull(value.id);
  const source = stringOrNull(value.source);
  const target = stringOrNull(value.target);
  const relationType = stringOrNull(value.relationType);
  const relationCategory = stringOrNull(value.relationCategory);
  if (!id || !source || !target || !relationType) return null;
  return {
    id,
    source,
    target,
    relationType,
    relationCategory: relationCategory ? (relationCategory as RelationCategory) : undefined,
    strength: typeof value.strength === 'number' ? value.strength : undefined,
    evidenceDocId: stringOrNull(value.evidenceDocId),
    evidenceTitle: stringOrNull(value.evidenceTitle),
    evidencePath: stringOrNull(value.evidencePath),
    otherLabel: stringOrNull(value.otherLabel),
  };
}

function parseRelationBundleRef(value: unknown): GraphAgentRelationBundleRef | null {
  if (!isRecord(value)) return null;
  const id = stringOrNull(value.id);
  const source = stringOrNull(value.source);
  const target = stringOrNull(value.target);
  if (!id || !source || !target) return null;
  const relationTypes = Array.isArray(value.relationTypes)
    ? value.relationTypes.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
    : [];
  if (relationTypes.length === 0) return null;
  const direction = stringOrNull(value.direction) as RelationDirection | null;
  const category = stringOrNull(value.category) as RelationCategory | null;
  const evidenceTitles = Array.isArray(value.evidenceTitles)
    ? value.evidenceTitles.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
    : [];
  const edgeIds = Array.isArray(value.edgeIds)
    ? value.edgeIds.filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
    : [];
  return {
    id,
    source,
    target,
    sourceLabel: stringOrNull(value.sourceLabel),
    targetLabel: stringOrNull(value.targetLabel),
    otherLabel: stringOrNull(value.otherLabel),
    relationTypes,
    relationCount: numberOr(value.relationCount, relationTypes.length),
    direction: direction ?? 'directed',
    category: category ?? 'general',
    strongestStrength: numberOr(value.strongestStrength),
    averageStrength: numberOr(value.averageStrength),
    evidenceTitles,
    edgeIds,
  };
}

function parseDocumentRef(value: unknown): KnowledgeGraphDocumentRef | null {
  if (!isRecord(value)) return null;
  const documentId = stringOrNull(value.documentId);
  const title = stringOrNull(value.title);
  const path = stringOrNull(value.path);
  const sourceId = stringOrNull(value.sourceId);
  if (!documentId || !title || !path || !sourceId) return null;
  return { documentId, title, path, sourceId };
}

function parseTokenEstimate(value: unknown): GraphTokenEstimate | null {
  if (!isRecord(value)) return null;
  return {
    graphIndexChars: numberOr(value.graphIndexChars),
    rawRetrievalCharsEstimate: numberOr(value.rawRetrievalCharsEstimate),
    savedCharsEstimate: numberOr(value.savedCharsEstimate),
    savedPctEstimate: numberOr(value.savedPctEstimate),
    documentCount: numberOr(value.documentCount),
    basis: stringOrNull(value.basis) ?? 'estimate',
  };
}

function readJson<T>(key: string): T | null {
  if (typeof window === 'undefined') return null;
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return null;
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

function writeJson(key: string, eventName: string, value: unknown | null): void {
  if (typeof window === 'undefined') return;
  try {
    if (value === null) {
      window.localStorage.removeItem(key);
    } else {
      window.localStorage.setItem(key, JSON.stringify(value));
    }
    window.dispatchEvent(new CustomEvent(eventName, { detail: value }));
  } catch {
    // Local storage is best-effort UI state.
  }
}

export function estimateGraphContextTokenSavings(input: {
  node: GraphAgentNodeRef;
  edges: GraphAgentEdgeRef[];
  relationBundles: GraphAgentRelationBundleRef[];
  documents: KnowledgeGraphDocumentRef[];
}): GraphTokenEstimate {
  const graphIndexChars = JSON.stringify(input).length;
  const documentCount = input.documents.length;
  const rawRetrievalCharsEstimate = Math.max(
    graphIndexChars,
    documentCount * 3200 + input.edges.length * 280 + input.relationBundles.length * 160,
  );
  const savedCharsEstimate = Math.max(0, rawRetrievalCharsEstimate - graphIndexChars);
  const savedPctEstimate =
    rawRetrievalCharsEstimate > 0
      ? Math.round((savedCharsEstimate / rawRetrievalCharsEstimate) * 100)
      : 0;

  return {
    graphIndexChars,
    rawRetrievalCharsEstimate,
    savedCharsEstimate,
    savedPctEstimate,
    documentCount,
    basis: 'graph_index_chars_vs_estimated_raw_document_context',
  };
}

function toAgentBundleRef(
  bundle: KnowledgeGraphRelationBundle,
  nodeLabelById: Map<string, string>,
  focusNodeId?: string,
): GraphAgentRelationBundleRef {
  const otherId = focusNodeId
    ? bundle.source === focusNodeId
      ? bundle.target
      : bundle.target === focusNodeId
        ? bundle.source
        : null
    : null;
  return {
    id: bundle.id,
    source: bundle.source,
    target: bundle.target,
    sourceLabel: nodeLabelById.get(bundle.source) ?? bundle.source,
    targetLabel: nodeLabelById.get(bundle.target) ?? bundle.target,
    otherLabel: otherId ? nodeLabelById.get(otherId) ?? otherId : null,
    relationTypes: bundle.relationTypes,
    relationCount: bundle.relationCount,
    direction: bundle.direction,
    category: bundle.category,
    strongestStrength: bundle.strongestStrength,
    averageStrength: bundle.averageStrength,
    evidenceTitles: bundle.evidenceTitles,
    edgeIds: bundle.edgeIds,
  };
}

export function buildGraphAgentContext(input: {
  sourceId: string | null;
  sourceLabel: string | null;
  pathPrefix: string | null;
  scopeLabel: string | null;
  node: KnowledgeGraphNode;
  edges: KnowledgeGraphEdge[];
  nodeLabelById: Map<string, string>;
  focusLabel?: string | null;
  focusKind?: 'node' | 'bundle';
}): GraphAgentContext {
  const documents = input.node.documents.slice(0, 12);
  const node: GraphAgentNodeRef = {
    id: input.node.id,
    label: input.node.label,
    entityType: input.node.entityType,
    description: input.node.description,
    documentCount: input.node.documentCount,
    mentionCount: input.node.mentionCount,
  };
  const edges: GraphAgentEdgeRef[] = input.edges.slice(0, 24).map((edge) => {
    const otherId = edge.source === input.node.id ? edge.target : edge.source;
    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      relationType: edge.relationType,
      relationCategory: buildRelationBundles([edge])[0]?.category,
      strength: edge.strength,
      evidenceDocId: edge.evidenceDocId,
      evidenceTitle: edge.evidenceTitle,
      evidencePath: edge.evidencePath,
      otherLabel: input.nodeLabelById.get(otherId) ?? otherId,
    };
  });
  const relationBundles = buildRelationBundles(input.edges)
    .slice(0, 12)
    .map((bundle) => toAgentBundleRef(bundle, input.nodeLabelById, input.node.id));
  return {
    id: `${input.focusKind ?? 'node'}:${input.node.id}:${Date.now()}`,
    createdAt: new Date().toISOString(),
    sourceId: input.sourceId,
    sourceLabel: input.sourceLabel,
    pathPrefix: input.pathPrefix,
    scopeLabel: input.scopeLabel,
    focusLabel: input.focusLabel ?? input.node.label,
    focusKind: input.focusKind ?? 'node',
    node,
    edges,
    relationBundles,
    documents,
    tokenEstimate: estimateGraphContextTokenSavings({ node, edges, relationBundles, documents }),
  };
}

export function buildGraphCollectionContext(
  context: GraphAgentContext,
): Conversation['collectionContext'] {
  const sourceIds = context.sourceId ? [context.sourceId] : [];
  const focusLabel = context.focusLabel ?? context.node.label;
  const relationBundles = context.relationBundles ?? [];
  const bundleLines = relationBundles.slice(0, 8).map((bundle) => {
    const other = bundle.otherLabel ?? bundle.targetLabel ?? bundle.target;
    const strength = ` strongest=${bundle.strongestStrength.toFixed(2)} avg=${bundle.averageStrength.toFixed(2)}`;
    const evidence = bundle.evidenceTitles.length ? ` evidence=${bundle.evidenceTitles.slice(0, 2).join('; ')}` : '';
    return `- ${context.node.label} <-> ${other} relations=${bundle.relationCount} direction=${bundle.direction} category=${bundle.category}${strength} types=${bundle.relationTypes.join(', ')}${evidence}`;
  });
  const relationLines = context.edges.slice(0, 12).map((edge) => {
    const other = edge.otherLabel ?? (edge.source === context.node.id ? edge.target : edge.source);
    const strength =
      typeof edge.strength === 'number' ? ` strength=${edge.strength.toFixed(2)}` : '';
    const evidence = edge.evidenceDocId ? ` evidenceDocId=${edge.evidenceDocId}` : '';
    return `- ${context.node.label} --${edge.relationType}${strength}--> ${other}${evidence}`;
  });
  const documentLines = context.documents.slice(0, 8).map(
    (doc) => `- ${doc.title} | documentId=${doc.documentId} | sourceId=${doc.sourceId} | path=${doc.path}`,
  );
  const scope = [
    `nodeId=${context.node.id}`,
    `entityName=${context.node.label}`,
    `entityType=${context.node.entityType ?? 'unknown'}`,
    `focusKind=${context.focusKind ?? 'node'}`,
    `focusLabel=${focusLabel}`,
    `sourceScope=${context.sourceId ?? 'all'}`,
    `pathPrefix=${context.pathPrefix ?? ''}`,
    `scopeLabel=${context.scopeLabel ?? ''}`,
  ].join('\n');

  return {
    title: `Graph: ${focusLabel}`,
    description:
      'User-selected relationship graph context. Treat it as a compact navigation index, not final evidence.',
    queryText: [
      scope,
      `tokenEstimate=graphIndexChars:${context.tokenEstimate.graphIndexChars},rawRetrievalCharsEstimate:${context.tokenEstimate.rawRetrievalCharsEstimate},savedPctEstimate:${context.tokenEstimate.savedPctEstimate}`,
      context.node.description ? `description=${context.node.description}` : '',
      bundleLines.length ? `Relationship bundles:\n${bundleLines.join('\n')}` : '',
      relationLines.length ? `Relations:\n${relationLines.join('\n')}` : '',
      documentLines.length ? `Evidence documents:\n${documentLines.join('\n')}` : '',
      'Agent instruction: inspect relationship bundles first to choose the smallest useful path, then use query_knowledge_graph related/path/search and retrieve or summarize only the necessary evidence documents before making detailed claims.',
    ].filter(Boolean).join('\n\n'),
    sourceIds,
  };
}

export function saveGraphAgentContext(context: GraphAgentContext): void {
  writeJson(GRAPH_AGENT_CONTEXT_STORAGE_KEY, GRAPH_AGENT_CONTEXT_EVENT, context);
}

export function readGraphAgentContext(): GraphAgentContext | null {
  return readJson<GraphAgentContext>(GRAPH_AGENT_CONTEXT_STORAGE_KEY);
}

export function clearGraphAgentContext(): void {
  writeJson(GRAPH_AGENT_CONTEXT_STORAGE_KEY, GRAPH_AGENT_CONTEXT_EVENT, null);
}

export function saveGraphAgentUsage(usage: GraphAgentUsage): void {
  writeJson(GRAPH_AGENT_USAGE_STORAGE_KEY, GRAPH_AGENT_USAGE_EVENT, usage);
}

export function readGraphAgentUsage(): GraphAgentUsage | null {
  return readJson<GraphAgentUsage>(GRAPH_AGENT_USAGE_STORAGE_KEY);
}

export function extractGraphAgentUsage(artifacts: unknown): GraphAgentUsage | null {
  if (!isRecord(artifacts)) return null;
  const payload = isRecord(artifacts.artifacts) ? artifacts.artifacts : artifacts;
  const graph = isRecord(payload.graph) ? payload.graph : null;
  if (payload.kind !== 'knowledgeGraphContext' && !graph) return null;

  const artifactNodes = Array.isArray(payload.usedGraphNodes)
    ? payload.usedGraphNodes
    : Array.isArray(graph?.nodes)
      ? graph.nodes
      : [];
  const artifactEdges = Array.isArray(payload.usedGraphEdges)
    ? payload.usedGraphEdges
    : Array.isArray(graph?.edges)
      ? graph.edges
      : [];
  const artifactBundles = Array.isArray(payload.usedGraphBundles)
    ? payload.usedGraphBundles
    : Array.isArray(graph?.relationBundles)
      ? graph.relationBundles
      : [];
  const artifactDocuments = Array.isArray(payload.usedDocuments)
    ? payload.usedDocuments
    : artifactNodes.flatMap((node) => isRecord(node) && Array.isArray(node.documents) ? node.documents : []);

  const usedGraphNodes = artifactNodes.flatMap((item) => {
    const parsed = parseNodeRef(item);
    return parsed ? [parsed] : [];
  });
  const usedGraphEdges = artifactEdges.flatMap((item) => {
    const parsed = parseEdgeRef(item);
    return parsed ? [parsed] : [];
  });
  const usedGraphBundles = artifactBundles.flatMap((item) => {
    const parsed = parseRelationBundleRef(item);
    return parsed ? [parsed] : [];
  });
  const usedDocuments = artifactDocuments.flatMap((item) => {
    const parsed = parseDocumentRef(item);
    return parsed ? [parsed] : [];
  });
  if (usedGraphNodes.length === 0 && usedGraphEdges.length === 0 && usedDocuments.length === 0) {
    return null;
  }

  const sourceScope = Array.isArray(payload.sourceScope)
    ? payload.sourceScope.filter((value): value is string => typeof value === 'string')
    : null;

  return {
    id: stringOrNull(payload.callId) ?? `usage:${Date.now()}`,
    createdAt: stringOrNull(payload.createdAt) ?? new Date().toISOString(),
    sourceScope,
    scopeLabel: stringOrNull(graph?.scopeLabel) ?? stringOrNull(payload.scopeLabel),
    usedGraphNodes,
    usedGraphEdges,
    usedGraphBundles,
    usedDocuments,
    tokenEstimate: parseTokenEstimate(payload.tokenEstimate),
  };
}
