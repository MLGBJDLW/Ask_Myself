import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { NexaSelect } from '../ui/overlay';
import { useNavigate } from 'react-router';
import {
  BookOpen,
  Building2,
  CalendarClock,
  CircleDot,
  ArrowLeftRight,
  ExternalLink,
  Filter,
  GitFork,
  Landmark,
  Loader2,
  MapPin,
  MessageSquare,
  Network,
  PlusCircle,
  RefreshCw,
  RotateCcw,
  Search,
  UserRound,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import type { Source } from '../../types';
import type {
  KnowledgeGraph,
  KnowledgeGraphEdge,
  KnowledgeGraphNode,
} from '../../types/knowledge';
import { useTranslation, type TranslationKey } from '../../i18n';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { Input } from '../ui/Input';
import { CardSkeleton } from '../ui/Skeleton';
import { formatUserError } from '../../lib/userError';
import {
  GRAPH_AGENT_USAGE_EVENT,
  buildGraphAgentContext,
  buildGraphCollectionContext,
  readGraphAgentUsage,
  saveGraphAgentContext,
  type GraphAgentContext,
} from '../../lib/knowledgeGraphAgent';
import {
  buildRelationBundles,
  type KnowledgeGraphRelationBundle,
  type RelationCategory,
  type RelationDirection,
} from '../../lib/knowledgeGraphRelations';

type EntityFilter = 'all' | 'person' | 'place' | 'organization' | 'event' | 'concept' | 'technology' | 'other';
type GraphMode = 'focus' | 'overview' | 'atlas';
type Translate = ReturnType<typeof useTranslation>['t'];

type PositionedNode = KnowledgeGraphNode & {
  x: number;
  y: number;
  radius: number;
  degree: number;
  importance: number;
  rank: number;
};

type SuggestedGraphArtifact = {
  id: string;
  kind: 'graph_relation_candidate' | 'entity_merge_candidate';
  title: string;
  sourceEntityId: string;
  targetEntityId: string;
  relationType: string;
  confidence: number;
};

const ENTITY_FILTERS: EntityFilter[] = ['all', 'person', 'place', 'organization', 'event', 'concept', 'technology', 'other'];
const ENTITY_FILTER_LABEL_KEYS: Record<EntityFilter, TranslationKey> = {
  all: 'knowledge.entityFilter.all',
  person: 'knowledge.entityFilter.person',
  place: 'knowledge.entityFilter.place',
  organization: 'knowledge.entityFilter.organization',
  event: 'knowledge.entityFilter.event',
  concept: 'knowledge.entityFilter.concept',
  technology: 'knowledge.entityFilter.technology',
  other: 'knowledge.entityFilter.other',
};
const ENTITY_TYPE_LABEL_KEYS: Record<string, TranslationKey> = {
  person: 'knowledge.entityTypeLabel.person',
  place: 'knowledge.entityTypeLabel.place',
  organization: 'knowledge.entityTypeLabel.organization',
  event: 'knowledge.entityTypeLabel.event',
  concept: 'knowledge.entityTypeLabel.concept',
  technology: 'knowledge.entityTypeLabel.technology',
  other: 'knowledge.entityTypeLabel.other',
};
const RELATION_CATEGORY_LABEL_KEYS: Record<RelationCategory, TranslationKey> = {
  conflict: 'knowledge.relationCategory.conflict',
  causal: 'knowledge.relationCategory.causal',
  hierarchy: 'knowledge.relationCategory.hierarchy',
  event: 'knowledge.relationCategory.event',
  social: 'knowledge.relationCategory.social',
  general: 'knowledge.relationCategory.general',
};
const RELATION_DIRECTION_LABEL_KEYS: Record<RelationDirection, TranslationKey> = {
  directed: 'knowledge.relationDirection.directed',
  bidirectional: 'knowledge.relationDirection.bidirectional',
  undirected: 'knowledge.relationDirection.undirected',
};
const GRAPH_MODE_LABEL_KEYS: Record<GraphMode, TranslationKey> = {
  focus: 'knowledge.graphMode.focus',
  overview: 'knowledge.graphMode.overview',
  atlas: 'knowledge.graphMode.atlas',
};
const RELATION_TYPE_LABEL_KEYS: Record<string, TranslationKey> = {
  co_occurs: 'knowledge.relationType.coOccurs',
};
const VIEWBOX_WIDTH = 1000;
const VIEWBOX_HEIGHT = 620;
const CENTER_X = VIEWBOX_WIDTH / 2;
const CENTER_Y = VIEWBOX_HEIGHT / 2;
const GRAPH_FETCH_LIMIT = 220;
const DEFAULT_VISIBLE_NODE_BUDGET = 40;
const NODE_BUDGET_OPTIONS = [40, 60, 100, 160];
const DEFAULT_GRAPH_VIEWBOX = { x: 0, y: 0, width: VIEWBOX_WIDTH, height: VIEWBOX_HEIGHT };
const DRAG_MOVE_THRESHOLD = 3;

const ENTITY_CLUSTER_ANCHORS: Record<string, { x: number; y: number }> = {
  person: { x: 255, y: 185 },
  place: { x: 740, y: 190 },
  organization: { x: 270, y: 450 },
  event: { x: 735, y: 455 },
  concept: { x: 505, y: 318 },
  technology: { x: 510, y: 130 },
  other: { x: 510, y: 515 },
};

const ENTITY_TONE: Record<string, { fill: string; stroke: string; text: string; solid: string; strokeColor: string }> = {
  person: { fill: 'fill-danger/15', stroke: 'stroke-danger', text: 'text-danger', solid: '#e11d48', strokeColor: '#be123c' },
  place: { fill: 'fill-info/15', stroke: 'stroke-info', text: 'text-info', solid: '#2563eb', strokeColor: '#1d4ed8' },
  organization: { fill: 'fill-warning/15', stroke: 'stroke-warning', text: 'text-warning', solid: '#f97316', strokeColor: '#c2410c' },
  event: { fill: 'fill-danger/15', stroke: 'stroke-danger', text: 'text-danger', solid: '#dc2626', strokeColor: '#991b1b' },
  concept: { fill: 'fill-info/15', stroke: 'stroke-info', text: 'text-info', solid: '#0891b2', strokeColor: '#0e7490' },
  technology: { fill: 'fill-info/15', stroke: 'stroke-info', text: 'text-info', solid: '#0284c7', strokeColor: '#075985' },
  other: { fill: 'fill-surface-3', stroke: 'stroke-text-tertiary', text: 'text-text-secondary', solid: '#94a3b8', strokeColor: '#64748b' },
};
const RELATION_CATEGORY_STYLE: Record<RelationCategory, {
  id: RelationCategory;
  text: string;
  color: string;
  badge: 'default' | 'success' | 'warning' | 'danger' | 'info';
  dash?: string;
}> = {
  conflict: { id: 'conflict', text: 'text-danger', color: '#e11d48', badge: 'danger', dash: '8 5' },
  causal: { id: 'causal', text: 'text-warning', color: '#f97316', badge: 'warning' },
  hierarchy: { id: 'hierarchy', text: 'text-info', color: '#2563eb', badge: 'info', dash: '4 3' },
  event: { id: 'event', text: 'text-danger', color: '#dc2626', badge: 'danger', dash: '2 4' },
  social: { id: 'social', text: 'text-info', color: '#0284c7', badge: 'info' },
  general: { id: 'general', text: 'text-text-secondary', color: '#64748b', badge: 'default' },
};

function entityIcon(entityType: string) {
  switch (entityType) {
    case 'person': return UserRound;
    case 'place': return MapPin;
    case 'organization': return Building2;
    case 'event': return CalendarClock;
    case 'concept': return CircleDot;
    case 'technology': return GitFork;
    default: return Landmark;
  }
}

function entityTone(entityType: string) {
  return ENTITY_TONE[entityType] ?? ENTITY_TONE.other;
}

function entityFilterLabel(filter: EntityFilter, t: Translate) {
  return t(ENTITY_FILTER_LABEL_KEYS[filter]);
}

function graphModeLabel(mode: GraphMode, t: Translate) {
  return t(GRAPH_MODE_LABEL_KEYS[mode]);
}

function entityTypeLabel(entityType: string, t: Translate) {
  return t(ENTITY_TYPE_LABEL_KEYS[entityType] ?? 'knowledge.entityTypeLabel.other');
}

function relationLabel(value: string, t: Translate) {
  const key = RELATION_TYPE_LABEL_KEYS[value];
  return key ? t(key) : value.replace(/_/g, ' ');
}

function relationCategoryLabel(category: RelationCategory, t: Translate) {
  return t(RELATION_CATEGORY_LABEL_KEYS[category]);
}

function relationDirectionLabel(direction: RelationDirection, t: Translate) {
  return t(RELATION_DIRECTION_LABEL_KEYS[direction]);
}

function shortPath(path: string) {
  const normalized = path.replace(/\\/g, '/');
  const parts = normalized.split('/').filter(Boolean);
  return parts.slice(-2).join('/') || path;
}

function formatCompactChars(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '0';
  if (value < 1000) return String(Math.round(value));
  const kilo = value / 1000;
  return `${kilo.toFixed(kilo < 10 ? 1 : 0)}k`;
}

function buildSuggestedGraphArtifacts(artifacts: api.DreamArtifact[]): SuggestedGraphArtifact[] {
  const suggestions: SuggestedGraphArtifact[] = [];
  for (const artifact of artifacts) {
    if (artifact.kind !== 'graph_relation_candidate' && artifact.kind !== 'entity_merge_candidate') {
      continue;
    }
    const payload = artifact.payloadJson;
    if (!payload || typeof payload !== 'object') {
      continue;
    }
    const record = payload as Record<string, unknown>;
    const sourceEntityId = stringValue(record.sourceEntityId) ?? stringValue(record.canonicalEntityId);
    const targetEntityId = stringValue(record.targetEntityId) ?? stringValue(record.duplicateEntityId);
    if (!sourceEntityId || !targetEntityId || sourceEntityId === targetEntityId) {
      continue;
    }
    suggestions.push({
      id: artifact.id,
      kind: artifact.kind,
      title: artifact.title,
      sourceEntityId,
      targetEntityId,
      relationType: stringValue(record.relationType) ?? (artifact.kind === 'entity_merge_candidate' ? 'same_as' : 'related_to'),
      confidence: artifact.confidence,
    });
  }
  return suggestions;
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function buildDegreeMap(edges: KnowledgeGraphEdge[]) {
  const degree = new Map<string, number>();
  for (const edge of edges) {
    degree.set(edge.source, (degree.get(edge.source) ?? 0) + 1);
    degree.set(edge.target, (degree.get(edge.target) ?? 0) + 1);
  }
  return degree;
}

function nodeImportance(node: KnowledgeGraphNode, degree: Map<string, number>) {
  return (degree.get(node.id) ?? 0) * 6 + node.linkCount * 3 + node.documentCount * 3 + node.mentionCount * 0.45;
}

function sortNodesByImportance(nodes: KnowledgeGraphNode[], degree: Map<string, number>) {
  return [...nodes].sort((a, b) => {
    const scoreDelta = nodeImportance(b, degree) - nodeImportance(a, degree);
    if (scoreDelta !== 0) return scoreDelta;
    return a.label.localeCompare(b.label);
  });
}

function pickDefaultNodeId(nodes: KnowledgeGraphNode[], edges: KnowledgeGraphEdge[]) {
  return sortNodesByImportance(nodes, buildDegreeMap(edges))[0]?.id ?? null;
}

function hashNumber(value: string) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function nodeMatchesSearch(node: KnowledgeGraphNode, query: string) {
  if (!query) return true;
  return (
    node.label.toLowerCase().includes(query) ||
    node.description.toLowerCase().includes(query) ||
    Boolean(node.aliases?.some((alias) => alias.toLowerCase().includes(query)))
  );
}

function collectNeighborIds(edges: KnowledgeGraphEdge[], ids: Set<string>) {
  const neighbors = new Set<string>();
  for (const edge of edges) {
    if (ids.has(edge.source)) neighbors.add(edge.target);
    if (ids.has(edge.target)) neighbors.add(edge.source);
  }
  return neighbors;
}

function limitOrderedIds(orderedIds: string[], requiredIds: Set<string>, budget: number) {
  const uniqueRequired = [...requiredIds].filter((id, index, ids) => ids.indexOf(id) === index);
  const result = [...uniqueRequired];
  const seen = new Set(result);
  for (const id of orderedIds) {
    if (seen.has(id)) continue;
    if (result.length >= budget) break;
    result.push(id);
    seen.add(id);
  }
  return result;
}

function buildVisibleGraph(
  graph: KnowledgeGraph | null,
  searchText: string,
  graphMode: GraphMode,
  anchorNodeId: string | null,
  selectedBundleId: string | null,
  maxVisibleNodes: number,
) {
  const nodes = graph?.nodes ?? [];
  const edges = graph?.edges ?? [];
  const query = searchText.trim().toLowerCase();
  const degree = buildDegreeMap(edges);
  const rankedNodes = sortNodesByImportance(nodes, degree);
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const requiredIds = new Set<string>();
  const selectedBundleNodeIds = selectedBundleId?.split('::').filter(Boolean) ?? [];
  for (const id of selectedBundleNodeIds) {
    if (nodeById.has(id)) requiredIds.add(id);
  }
  if (anchorNodeId && nodeById.has(anchorNodeId)) requiredIds.add(anchorNodeId);

  let orderedIds: string[] = [];
  if (query) {
    const matchedIds = new Set(nodes.filter((node) => nodeMatchesSearch(node, query)).map((node) => node.id));
    const neighborIds = collectNeighborIds(edges, matchedIds);
    orderedIds = [
      ...rankedNodes.filter((node) => matchedIds.has(node.id)).map((node) => node.id),
      ...rankedNodes.filter((node) => neighborIds.has(node.id)).map((node) => node.id),
    ];
  } else if (graphMode === 'focus' && requiredIds.size > 0) {
    const firstHop = collectNeighborIds(edges, requiredIds);
    const secondHop = collectNeighborIds(edges, firstHop);
    orderedIds = [
      ...rankedNodes.filter((node) => requiredIds.has(node.id)).map((node) => node.id),
      ...rankedNodes.filter((node) => firstHop.has(node.id)).map((node) => node.id),
      ...rankedNodes.filter((node) => secondHop.has(node.id) && !requiredIds.has(node.id)).map((node) => node.id),
    ];
    if (orderedIds.length < Math.min(8, maxVisibleNodes)) {
      orderedIds.push(...rankedNodes.map((node) => node.id));
    }
  } else if (graphMode === 'overview') {
    const buckets = new Map<string, KnowledgeGraphNode[]>();
    for (const node of rankedNodes) {
      const bucket = node.entityType in ENTITY_CLUSTER_ANCHORS ? node.entityType : 'other';
      buckets.set(bucket, [...(buckets.get(bucket) ?? []), node]);
    }
    const bucketOrder = Object.keys(ENTITY_CLUSTER_ANCHORS);
    let added = true;
    while (added) {
      added = false;
      for (const bucket of bucketOrder) {
        const next = buckets.get(bucket)?.shift();
        if (!next) continue;
        orderedIds.push(next.id);
        added = true;
      }
    }
  } else {
    orderedIds = rankedNodes.map((node) => node.id);
  }

  const candidateIds = [...new Set(orderedIds.filter((id) => nodeById.has(id)))];
  const limitedIds = limitOrderedIds(candidateIds, requiredIds, maxVisibleNodes);
  const visibleNodeIds = new Set(limitedIds);
  const visibleNodes = limitedIds.map((id) => nodeById.get(id)).filter((node): node is KnowledgeGraphNode => Boolean(node));
  const visibleEdges = edges.filter((edge) => visibleNodeIds.has(edge.source) && visibleNodeIds.has(edge.target));

  return {
    nodes: visibleNodes,
    edges: visibleEdges,
    hiddenNodeCount: Math.max(0, candidateIds.length - visibleNodes.length),
  };
}

function truncateNodeLabel(label: string, maxLength: number) {
  return label.length > maxLength ? `${label.slice(0, maxLength - 1)}...` : label;
}

function manualNodePositionKey(graphMode: GraphMode, nodeId: string) {
  return `${graphMode}:${nodeId}`;
}

function computeLayout(
  nodes: KnowledgeGraphNode[],
  edges: KnowledgeGraphEdge[],
  graphMode: GraphMode,
  anchorNodeId: string | null,
): PositionedNode[] {
  const degree = buildDegreeMap(edges);

  const sorted = sortNodesByImportance(nodes, degree);
  const rankById = new Map(sorted.map((node, index) => [node.id, index]));
  const bucketCounts = new Map<string, number>();
  const centerBias = graphMode === 'focus' ? 0.18 : 1;
  const focusAnchorId =
    graphMode === 'focus' && anchorNodeId && nodes.some((node) => node.id === anchorNodeId)
      ? anchorNodeId
      : graphMode === 'focus'
        ? sorted[0]?.id ?? null
        : null;
  const focusNeighborIds = new Set<string>();
  if (focusAnchorId) {
    for (const edge of edges) {
      if (edge.source === focusAnchorId) focusNeighborIds.add(edge.target);
      if (edge.target === focusAnchorId) focusNeighborIds.add(edge.source);
    }
  }
  const focusPrimaryNodes = sorted.filter((node) => node.id !== focusAnchorId && focusNeighborIds.has(node.id));
  const focusSecondaryNodes = sorted.filter((node) => node.id !== focusAnchorId && !focusNeighborIds.has(node.id));
  const focusPrimaryRank = new Map(focusPrimaryNodes.map((node, index) => [node.id, index]));
  const focusSecondaryRank = new Map(focusSecondaryNodes.map((node, index) => [node.id, index]));
  const layoutTargets = new Map<string, { x: number; y: number }>();

  const layout = sorted.map((node): PositionedNode => {
    const nodeDegree = degree.get(node.id) ?? 0;
    const nodeRadius = Math.min(15, 5.5 + Math.sqrt(Math.max(1, node.mentionCount + nodeDegree * 2)) * 1.1);
    const baseNode = {
      ...node,
      radius: nodeRadius,
      degree: nodeDegree,
      importance: nodeImportance(node, degree),
      rank: rankById.get(node.id) ?? 0,
    };

    if (graphMode === 'focus' && focusAnchorId) {
      if (node.id === focusAnchorId) {
        const x = CENTER_X - 28;
        const y = CENTER_Y - 12;
        layoutTargets.set(node.id, { x, y });
        return { ...baseNode, x, y };
      }

      const isPrimary = focusNeighborIds.has(node.id);
      const ringCount = Math.max(1, isPrimary ? focusPrimaryNodes.length : focusSecondaryNodes.length);
      const ringIndex = isPrimary ? focusPrimaryRank.get(node.id) ?? 0 : focusSecondaryRank.get(node.id) ?? 0;
      const angleOffset = isPrimary ? -Math.PI / 2 : -Math.PI / 2 + Math.PI / Math.max(3, ringCount);
      const angle = angleOffset + (ringIndex / ringCount) * Math.PI * 2;
      const radiusX = isPrimary ? Math.min(235, 188 + ringCount * 5) : 285;
      const radiusY = isPrimary ? Math.min(172, 132 + ringCount * 4) : 212;
      const x = CENTER_X + Math.cos(angle) * radiusX;
      const y = CENTER_Y + Math.sin(angle) * radiusY;
      layoutTargets.set(node.id, { x, y });
      return { ...baseNode, x, y };
    }

    const cluster = node.entityType in ENTITY_CLUSTER_ANCHORS ? node.entityType : 'other';
    const anchor = ENTITY_CLUSTER_ANCHORS[cluster];
    const bucketIndex = bucketCounts.get(cluster) ?? 0;
    bucketCounts.set(cluster, bucketIndex + 1);
    const seed = hashNumber(node.id);
    const angle = ((seed % 3600) / 3600) * Math.PI * 2 + bucketIndex * 0.82;
    const spiral = 24 + Math.sqrt(bucketIndex + 1) * 34;
    const anchorX = CENTER_X + (anchor.x - CENTER_X) * centerBias;
    const anchorY = CENTER_Y + (anchor.y - CENTER_Y) * centerBias;
    const x = anchorX + Math.cos(angle) * spiral;
    const y = anchorY + Math.sin(angle) * spiral * 0.72;
    layoutTargets.set(node.id, { x: anchorX, y: anchorY });

    return {
      ...baseNode,
      x,
      y,
    };
  });

  const indexById = new Map(layout.map((node, index) => [node.id, index]));
  const links = edges
    .map((edge) => {
      const sourceIndex = indexById.get(edge.source);
      const targetIndex = indexById.get(edge.target);
      if (sourceIndex === undefined || targetIndex === undefined) return null;
      return { sourceIndex, targetIndex, strength: Math.max(0.35, edge.strength) };
    })
    .filter((link): link is { sourceIndex: number; targetIndex: number; strength: number } => Boolean(link));
  const iterations = graphMode === 'focus' ? 82 : 96;

  for (let iteration = 0; iteration < iterations; iteration += 1) {
    for (let i = 0; i < layout.length; i += 1) {
      for (let j = i + 1; j < layout.length; j += 1) {
        const a = layout[i];
        const b = layout[j];
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const distance = Math.hypot(dx, dy) || 1;
        const minDistance = a.radius + b.radius + (graphMode === 'focus' ? 28 : 20);
        const nx = dx / distance;
        const ny = dy / distance;

        if (distance < minDistance) {
          const push = (minDistance - distance) * 0.026;
          a.x -= nx * push;
          a.y -= ny * push;
          b.x += nx * push;
          b.y += ny * push;
        } else {
          const push = Math.min(1.45, 92 / (distance * distance));
          a.x -= nx * push;
          a.y -= ny * push;
          b.x += nx * push;
          b.y += ny * push;
        }
      }
    }

    for (const link of links) {
      const source = layout[link.sourceIndex];
      const target = layout[link.targetIndex];
      const dx = target.x - source.x;
      const dy = target.y - source.y;
      const distance = Math.hypot(dx, dy) || 1;
      const desired = graphMode === 'focus' ? 148 : 172;
      const pull = (distance - desired) * 0.008 * link.strength;
      const nx = dx / distance;
      const ny = dy / distance;
      source.x += nx * pull;
      source.y += ny * pull;
      target.x -= nx * pull;
      target.y -= ny * pull;
    }

    for (const node of layout) {
      const cluster = node.entityType in ENTITY_CLUSTER_ANCHORS ? node.entityType : 'other';
      const anchor = ENTITY_CLUSTER_ANCHORS[cluster];
      const target = layoutTargets.get(node.id) ?? {
        x: CENTER_X + (anchor.x - CENTER_X) * centerBias,
        y: CENTER_Y + (anchor.y - CENTER_Y) * centerBias,
      };
      const pull = graphMode === 'focus' ? 0.024 : graphMode === 'atlas' ? 0.009 : 0.014;
      node.x += (target.x - node.x) * pull;
      node.y += (target.y - node.y) * pull;

      const margin = node.radius + 42;
      node.x = Math.min(VIEWBOX_WIDTH - margin, Math.max(margin, node.x));
      node.y = Math.min(VIEWBOX_HEIGHT - margin, Math.max(margin, node.y));
    }
  }

  return layout.sort((a, b) => a.importance - b.importance);
}

function relationOffset(index: number, total: number) {
  if (total <= 1) return 0;
  return (index - (total - 1) / 2) * 18;
}

function edgePath(source: PositionedNode, target: PositionedNode, offset = 0) {
  const dx = target.x - source.x;
  const dy = target.y - source.y;
  const length = Math.hypot(dx, dy) || 1;
  const nx = -dy / length;
  const ny = dx / length;
  const curve = Math.min(80, Math.max(-80, dx * 0.08));
  const sx = source.x + nx * offset * 0.35;
  const sy = source.y + ny * offset * 0.35;
  const tx = target.x + nx * offset * 0.35;
  const ty = target.y + ny * offset * 0.35;
  const cx = (source.x + target.x) / 2 - dy * 0.08 + nx * offset;
  const cy = (source.y + target.y) / 2 + curve + ny * offset;
  return `M ${sx} ${sy} Q ${cx} ${cy} ${tx} ${ty}`;
}

function bundleMidpoint(source: PositionedNode, target: PositionedNode) {
  return {
    x: (source.x + target.x) / 2,
    y: (source.y + target.y) / 2,
  };
}

function computeGraphViewBox(nodes: PositionedNode[], graphMode: GraphMode) {
  if (nodes.length === 0 || graphMode === 'atlas') return DEFAULT_GRAPH_VIEWBOX;

  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const node of nodes) {
    const labelPadding = graphMode === 'focus' ? 52 : 38;
    minX = Math.min(minX, node.x - node.radius - labelPadding);
    maxX = Math.max(maxX, node.x + node.radius + labelPadding);
    minY = Math.min(minY, node.y - node.radius - labelPadding);
    maxY = Math.max(maxY, node.y + node.radius + labelPadding);
  }

  const minWidth = graphMode === 'focus' ? 480 : 720;
  const minHeight = graphMode === 'focus' ? 360 : 460;
  const naturalWidth = Math.max(1, maxX - minX);
  const naturalHeight = Math.max(1, maxY - minY);
  const width = Math.min(VIEWBOX_WIDTH, Math.max(minWidth, naturalWidth));
  const height = Math.min(VIEWBOX_HEIGHT, Math.max(minHeight, naturalHeight));
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  const x = Math.max(0, Math.min(VIEWBOX_WIDTH - width, centerX - width / 2));
  const y = Math.max(0, Math.min(VIEWBOX_HEIGHT - height, centerY - height / 2));

  return {
    x: Math.round(x),
    y: Math.round(y),
    width: Math.round(width),
    height: Math.round(height),
  };
}

export function KnowledgeGraphView({ onOpenInsights }: { onOpenInsights?: () => void } = {}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const graphSvgRef = useRef<SVGSVGElement | null>(null);
  const dragStateRef = useRef<{
    nodeId: string;
    pointerId: number;
    startClientX: number;
    startClientY: number;
    moved: boolean;
  } | null>(null);
  const suppressClickNodeRef = useRef<string | null>(null);
  const [graph, setGraph] = useState<KnowledgeGraph | null>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedSourceId, setSelectedSourceId] = useState<string>('');
  const [pathPrefix, setPathPrefix] = useState('');
  const [entityFilter, setEntityFilter] = useState<EntityFilter>('all');
  const [relationFilter, setRelationFilter] = useState('');
  const [searchText, setSearchText] = useState('');
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedBundleId, setSelectedBundleId] = useState<string | null>(null);
  const [showExpandedRelations, setShowExpandedRelations] = useState(false);
  const [graphMode, setGraphMode] = useState<GraphMode>('overview');
  const [maxVisibleNodes, setMaxVisibleNodes] = useState(DEFAULT_VISIBLE_NODE_BUDGET);
  const [manualNodePositions, setManualNodePositions] = useState<Record<string, { x: number; y: number }>>({});
  const [draggingNodeId, setDraggingNodeId] = useState<string | null>(null);
  const [agentUsage, setAgentUsage] = useState(() => readGraphAgentUsage());
  const [suggestedArtifacts, setSuggestedArtifacts] = useState<api.DreamArtifact[]>([]);

  const loadSources = useCallback(async () => {
    try {
      const nextSources = await api.listSources();
      setSources(nextSources);
    } catch (e) {
      toast.error(formatUserError(t('knowledge.sources'), e));
    }
  }, [t]);

  const loadGraph = useCallback(async () => {
    setLoading(true);
    try {
      const nextGraph = await api.getKnowledgeGraph({
        limit: GRAPH_FETCH_LIMIT,
        sourceId: selectedSourceId || null,
        pathPrefix: pathPrefix.trim() || null,
        entityTypes: entityFilter === 'all' ? [] : [entityFilter],
        relationTypes: relationFilter ? [relationFilter] : [],
      });
      setGraph(nextGraph);
      setSelectedNodeId((current) => {
        if (current && nextGraph.nodes.some((node) => node.id === current)) return current;
        return pickDefaultNodeId(nextGraph.nodes, nextGraph.edges);
      });
      setSelectedBundleId((current) => {
        if (current && nextGraph.edges.some((edge) => `${edge.source}::${edge.target}` === current || `${edge.target}::${edge.source}` === current)) {
          return current;
        }
        return null;
      });
    } catch (e) {
      toast.error(formatUserError(t('knowledge.relationshipGraph'), e));
    } finally {
      setLoading(false);
    }
  }, [entityFilter, pathPrefix, relationFilter, selectedSourceId, t]);

  const loadGraphSuggestions = useCallback(async () => {
    try {
      const artifacts = await api.listDreamArtifacts({ status: 'pending', limit: 80 });
      setSuggestedArtifacts(artifacts.filter((artifact) =>
        artifact.kind === 'graph_relation_candidate' || artifact.kind === 'entity_merge_candidate',
      ));
    } catch {
      setSuggestedArtifacts([]);
    }
  }, []);

  useEffect(() => {
    void loadSources();
  }, [loadSources]);

  useEffect(() => {
    void loadGraph();
  }, [loadGraph]);

  useEffect(() => {
    void loadGraphSuggestions();
  }, [loadGraphSuggestions]);

  useEffect(() => {
    const syncUsage = () => setAgentUsage(readGraphAgentUsage());
    window.addEventListener(GRAPH_AGENT_USAGE_EVENT, syncUsage as EventListener);
    window.addEventListener('storage', syncUsage);
    return () => {
      window.removeEventListener(GRAPH_AGENT_USAGE_EVENT, syncUsage as EventListener);
      window.removeEventListener('storage', syncUsage);
    };
  }, []);

  const selectedSource = sources.find((source) => source.id === selectedSourceId) ?? null;
  const trimmedPathPrefix = pathPrefix.trim();
  const trimmedSearchText = searchText.trim();
  const bundleAnchorNodeId = selectedBundleId?.split('::')[0] ?? null;
  const anchorNodeId = selectedNodeId ?? bundleAnchorNodeId ?? pickDefaultNodeId(graph?.nodes ?? [], graph?.edges ?? []);
  const relationTypes = useMemo(() => {
    const values = new Set(graph?.edges.map((edge) => edge.relationType) ?? []);
    return [...values].sort((a, b) => a.localeCompare(b));
  }, [graph]);

  const visibleGraph = useMemo(
    () => buildVisibleGraph(graph, trimmedSearchText, graphMode, anchorNodeId, selectedBundleId, maxVisibleNodes),
    [anchorNodeId, graph, graphMode, maxVisibleNodes, selectedBundleId, trimmedSearchText],
  );
  const visibleNodes = visibleGraph.nodes;
  const visibleEdges = visibleGraph.edges;
  const visibleRelationBundles = useMemo(() => buildRelationBundles(visibleEdges), [visibleEdges]);

  const positionedNodes = useMemo(
    () => computeLayout(visibleNodes, visibleEdges, graphMode, anchorNodeId).map((node) => {
      const manualPosition = manualNodePositions[manualNodePositionKey(graphMode, node.id)];
      return manualPosition ? { ...node, ...manualPosition } : node;
    }),
    [anchorNodeId, graphMode, manualNodePositions, visibleEdges, visibleNodes],
  );
  const graphViewBox = useMemo(() => computeGraphViewBox(positionedNodes, graphMode), [graphMode, positionedNodes]);
  const nodeById = useMemo(() => new Map(positionedNodes.map((node) => [node.id, node])), [positionedNodes]);
  const suggestedGraphArtifacts = useMemo(
    () => buildSuggestedGraphArtifacts(suggestedArtifacts),
    [suggestedArtifacts],
  );
  const visibleSuggestedGraphArtifacts = useMemo(
    () => suggestedGraphArtifacts.filter((artifact) =>
      nodeById.has(artifact.sourceEntityId) && nodeById.has(artifact.targetEntityId),
    ),
    [nodeById, suggestedGraphArtifacts],
  );
  const bundleById = useMemo(
    () => new Map(visibleRelationBundles.map((bundle) => [bundle.id, bundle])),
    [visibleRelationBundles],
  );
  const nodeLabelById = useMemo(
    () => new Map(positionedNodes.map((node) => [node.id, node.label])),
    [positionedNodes],
  );
  const selectedNode = useMemo(() => {
    if (selectedBundleId) return null;
    if (!selectedNodeId) return positionedNodes[0] ?? null;
    return nodeById.get(selectedNodeId) ?? positionedNodes[0] ?? null;
  }, [nodeById, positionedNodes, selectedBundleId, selectedNodeId]);
  const selectedBundle = selectedBundleId ? bundleById.get(selectedBundleId) ?? null : null;
  const selectedNodeEdges = useMemo(() => {
    if (!selectedNode) return [];
    return visibleEdges.filter((edge) => edge.source === selectedNode.id || edge.target === selectedNode.id);
  }, [selectedNode, visibleEdges]);
  const selectedNodeBundles = useMemo(() => {
    if (!selectedNode) return [];
    return visibleRelationBundles.filter((bundle) => bundle.source === selectedNode.id || bundle.target === selectedNode.id);
  }, [selectedNode, visibleRelationBundles]);
  const selectedBundleNodeIds = useMemo(() => {
    if (!selectedBundle) return new Set<string>();
    return new Set([selectedBundle.source, selectedBundle.target]);
  }, [selectedBundle]);
  const selectedGraphContext = useMemo(() => {
    if (selectedBundle) {
      const anchor = nodeById.get(selectedBundle.source) ?? nodeById.get(selectedBundle.target) ?? null;
      if (!anchor) return null;
      const sourceLabel = nodeLabelById.get(selectedBundle.source) ?? selectedBundle.source;
      const targetLabel = nodeLabelById.get(selectedBundle.target) ?? selectedBundle.target;
      return buildGraphAgentContext({
        sourceId: selectedSourceId || null,
        sourceLabel: selectedSource ? shortPath(selectedSource.rootPath) : null,
        pathPrefix: trimmedPathPrefix || null,
        scopeLabel: graph?.scopeLabel ?? null,
        node: anchor,
        edges: selectedBundle.edges,
        nodeLabelById,
        focusKind: 'bundle',
        focusLabel: `${sourceLabel} <-> ${targetLabel}`,
      });
    }
    if (!selectedNode) return null;
    return buildGraphAgentContext({
      sourceId: selectedSourceId || null,
      sourceLabel: selectedSource ? shortPath(selectedSource.rootPath) : null,
      pathPrefix: trimmedPathPrefix || null,
      scopeLabel: graph?.scopeLabel ?? null,
      node: selectedNode,
      edges: selectedNodeEdges,
      nodeLabelById,
    });
  }, [graph?.scopeLabel, nodeById, nodeLabelById, selectedBundle, selectedNode, selectedNodeEdges, selectedSource, selectedSourceId, trimmedPathPrefix]);
  const agentUsedNodeIds = useMemo(
    () => new Set(agentUsage?.usedGraphNodes.map((node) => node.id) ?? []),
    [agentUsage],
  );
  const agentUsedEdgeIds = useMemo(
    () => new Set(agentUsage?.usedGraphEdges.map((edge) => edge.id) ?? []),
    [agentUsage],
  );
  const agentUsedBundleIds = useMemo(() => {
    const ids = new Set(agentUsage?.usedGraphBundles?.map((bundle) => bundle.id) ?? []);
    for (const bundle of visibleRelationBundles) {
      if (bundle.edges.some((edge) => agentUsedEdgeIds.has(edge.id))) {
        ids.add(bundle.id);
      }
    }
    return ids;
  }, [agentUsage, agentUsedEdgeIds, visibleRelationBundles]);
  const hasActiveGraphFilters = Boolean(
    selectedSourceId || trimmedPathPrefix || entityFilter !== 'all' || relationFilter || trimmedSearchText,
  );
  const totalGraphNodes = graph?.totalNodes ?? graph?.nodes.length ?? 0;
  const totalGraphEdges = graph?.totalEdges ?? graph?.edges.length ?? 0;
  const hiddenNodeCount = Math.max(visibleGraph.hiddenNodeCount, totalGraphNodes - visibleNodes.length);
  const graphScopeLabel = selectedSource
    ? `${shortPath(selectedSource.rootPath)}${trimmedPathPrefix ? ` / ${trimmedPathPrefix}` : ''}`
    : trimmedPathPrefix
      ? `${t('knowledge.allSources')} / ${trimmedPathPrefix}`
      : t('knowledge.allSources');
  const emptyGraphTitle = hasActiveGraphFilters ? t('knowledge.noGraphMatches') : t('knowledge.noGraph');
  const emptyGraphDescription = hasActiveGraphFilters
    ? t('knowledge.noGraphFilteredDescription')
    : t('knowledge.noGraphDescription');

  const persistSelectedGraphContext = useCallback(() => {
    if (!selectedGraphContext) return null;
    saveGraphAgentContext(selectedGraphContext);
    toast.success(t('knowledge.graphContextReady'));
    return selectedGraphContext;
  }, [selectedGraphContext, t]);

  const handleUseAsContext = useCallback(() => {
    persistSelectedGraphContext();
  }, [persistSelectedGraphContext]);

  const handleAskAgent = useCallback(() => {
    const context = persistSelectedGraphContext();
    if (!context) return;
    navigate('/chat', {
      state: {
        sourceIds: context.sourceId ? [context.sourceId] : [],
        collectionContext: buildGraphCollectionContext(context),
      },
    });
  }, [navigate, persistSelectedGraphContext]);

  const handleSelectNode = useCallback((nodeId: string) => {
    setSelectedNodeId(nodeId);
    setSelectedBundleId(null);
    if (graphMode !== 'focus') {
      setGraphMode('focus');
    }
  }, [graphMode]);

  const handleSelectBundle = useCallback((bundleId: string) => {
    setSelectedBundleId(bundleId);
    setSelectedNodeId(null);
    if (graphMode !== 'focus') {
      setGraphMode('focus');
    }
  }, [graphMode]);

  const getSvgPoint = useCallback((event: ReactPointerEvent) => {
    const svg = graphSvgRef.current;
    if (!svg) return null;
    const matrix = svg.getScreenCTM();
    if (!matrix) return null;
    const point = svg.createSVGPoint();
    point.x = event.clientX;
    point.y = event.clientY;
    return point.matrixTransform(matrix.inverse());
  }, []);

  const handleNodePointerDown = useCallback((event: ReactPointerEvent<SVGGElement>, nodeId: string) => {
    if (event.button !== 0) return;
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragStateRef.current = {
      nodeId,
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      moved: false,
    };
    setDraggingNodeId(nodeId);
  }, []);

  const handleGraphPointerMove = useCallback((event: ReactPointerEvent<SVGSVGElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    const movedDistance = Math.hypot(event.clientX - dragState.startClientX, event.clientY - dragState.startClientY);
    if (movedDistance < DRAG_MOVE_THRESHOLD && !dragState.moved) return;

    dragState.moved = true;
    event.preventDefault();
    const point = getSvgPoint(event);
    const node = nodeById.get(dragState.nodeId);
    if (!point || !node) return;

    const margin = node.radius + 38;
    const x = Math.min(VIEWBOX_WIDTH - margin, Math.max(margin, point.x));
    const y = Math.min(VIEWBOX_HEIGHT - margin, Math.max(margin, point.y));
    const key = manualNodePositionKey(graphMode, dragState.nodeId);
    setManualNodePositions((current) => ({
      ...current,
      [key]: { x, y },
    }));
  }, [getSvgPoint, graphMode, nodeById]);

  const finishGraphDrag = useCallback((event: ReactPointerEvent<SVGSVGElement>) => {
    const dragState = dragStateRef.current;
    if (!dragState || dragState.pointerId !== event.pointerId) return;

    if (dragState.moved) {
      suppressClickNodeRef.current = dragState.nodeId;
      window.setTimeout(() => {
        if (suppressClickNodeRef.current === dragState.nodeId) {
          suppressClickNodeRef.current = null;
        }
      }, 0);
    }

    dragStateRef.current = null;
    setDraggingNodeId(null);
  }, []);

  const resetFilters = () => {
    setSelectedSourceId('');
    setPathPrefix('');
    setEntityFilter('all');
    setRelationFilter('');
    setSearchText('');
    setSelectedBundleId(null);
    setGraphMode('overview');
    setMaxVisibleNodes(DEFAULT_VISIBLE_NODE_BUDGET);
    setManualNodePositions({});
  };

  if (loading && !graph) {
    return (
      <div className="space-y-3">
        <CardSkeleton />
        <CardSkeleton />
        <CardSkeleton />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-col gap-4">
      <section className="rounded-lg border border-border bg-surface-1 p-3">
        <div className="grid gap-3 lg:grid-cols-[minmax(180px,0.9fr)_minmax(180px,0.8fr)_minmax(220px,1fr)_auto]">
          <label className="space-y-1.5">
            <span className="text-xs font-medium text-text-tertiary">{t('knowledge.sourceScope')}</span>
            <NexaSelect
              value={selectedSourceId}
              onChange={(event) => setSelectedSourceId(event.target.value)}
              className="h-10 w-full rounded-md border border-border bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            >
              <option value="">{t('knowledge.allSources')}</option>
              {sources.map((source) => (
                <option key={source.id} value={source.id}>{shortPath(source.rootPath)}</option>
              ))}
            </NexaSelect>
          </label>

          <label className="space-y-1.5">
            <span className="text-xs font-medium text-text-tertiary">{t('knowledge.folderPrefix')}</span>
            <Input
              value={pathPrefix}
              onChange={(event) => setPathPrefix(event.target.value)}
              placeholder={t('knowledge.folderPrefixPlaceholder')}
            />
          </label>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-text-tertiary">{t('knowledge.focusType')}</span>
            <div className="flex min-h-10 flex-wrap items-center gap-1 rounded-md border border-border bg-surface-0 px-1.5 py-1.5">
              {ENTITY_FILTERS.map((filter) => (
                <button
                  key={filter}
                  type="button"
                  onClick={() => setEntityFilter(filter)}
                  className={`h-7 shrink-0 rounded-md px-2 text-xs transition-colors ${
                    entityFilter === filter
                      ? 'bg-accent-subtle text-accent-hover'
                      : 'text-text-tertiary hover:bg-surface-2 hover:text-text-primary'
                  }`}
                >
                  {entityFilterLabel(filter, t)}
                </button>
              ))}
            </div>
          </div>

          <div className="flex items-end gap-2">
            <Button
              variant="secondary"
              size="md"
              icon={<RotateCcw size={15} />}
              onClick={resetFilters}
            >
              {t('knowledge.reset')}
            </Button>
            <Button
              variant="primary"
              size="md"
              loading={loading}
              icon={<RefreshCw size={15} />}
              onClick={() => void loadGraph()}
            >
              {t('knowledge.refreshGraph')}
            </Button>
          </div>
        </div>

        <div className="mt-3 grid gap-3 xl:grid-cols-[minmax(240px,1fr)_minmax(180px,0.55fr)_minmax(260px,0.75fr)_minmax(130px,0.35fr)]">
          <Input
            icon={<Search size={15} />}
            placeholder={t('knowledge.searchGraph')}
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
          />
          <label className="relative">
            <Filter size={15} className="pointer-events-none absolute left-3 top-1/2 block shrink-0 -translate-y-1/2 text-text-tertiary" />
            <NexaSelect
              value={relationFilter}
              onChange={(event) => setRelationFilter(event.target.value)}
              className="h-10 w-full rounded-md border border-border bg-surface-0 py-0 pl-10 pr-3 text-sm text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            >
              <option value="">{t('knowledge.allRelations')}</option>
              {relationTypes.map((relation) => (
                <option key={relation} value={relation}>{relationLabel(relation, t)}</option>
              ))}
            </NexaSelect>
          </label>

          <div className="space-y-1.5">
            <span className="text-xs font-medium text-text-tertiary">{t('knowledge.graphViewMode')}</span>
            <div className="flex h-10 items-center gap-1 rounded-md border border-border bg-surface-0 px-1.5">
              {(['focus', 'overview', 'atlas'] as GraphMode[]).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  onClick={() => setGraphMode(mode)}
                  className={`h-7 min-w-0 flex-1 rounded-md px-2 text-xs transition-colors ${
                    graphMode === mode
                      ? 'bg-accent-subtle text-accent-hover'
                      : 'text-text-tertiary hover:bg-surface-2 hover:text-text-primary'
                  }`}
                >
                  {graphModeLabel(mode, t)}
                </button>
              ))}
            </div>
          </div>

          <label className="space-y-1.5">
            <span className="text-xs font-medium text-text-tertiary">{t('knowledge.visibleBudget')}</span>
            <NexaSelect
              value={maxVisibleNodes}
              onChange={(event) => setMaxVisibleNodes(Number(event.target.value))}
              className="h-10 w-full rounded-md border border-border bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            >
              {NODE_BUDGET_OPTIONS.map((value) => (
                <option key={value} value={value}>{value}</option>
              ))}
            </NexaSelect>
          </label>
        </div>
      </section>

      <div className="grid min-h-[620px] gap-4 xl:grid-cols-[minmax(0,1fr)_340px]">
        <section className="relative h-[620px] min-h-[620px] overflow-hidden rounded-lg border border-border bg-surface-1">
          <div className="flex items-center justify-between border-b border-border px-4 py-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                <Network size={16} className="block shrink-0 text-accent" />
                {t('knowledge.relationshipGraph')}
              </div>
              <p className="mt-0.5 truncate text-xs text-text-tertiary">
                {graphScopeLabel}
              </p>
            </div>
            <div className="flex shrink-0 flex-wrap justify-end gap-1.5">
              <Button
                variant="secondary"
                size="sm"
                icon={<GitFork size={14} />}
                onClick={() => setShowExpandedRelations((value) => !value)}
              >
                {showExpandedRelations ? t('knowledge.bundleRelations') : t('knowledge.expandRelations')}
              </Button>
              {agentUsage && agentUsedNodeIds.size > 0 && (
                <Badge variant="warning">
                  {t('knowledge.agentUsedPath')}: {agentUsedNodeIds.size}
                </Badge>
              )}
              {hiddenNodeCount > 0 && (
                <Badge variant="warning">{hiddenNodeCount} {t('knowledge.hiddenNodes')}</Badge>
              )}
              {visibleSuggestedGraphArtifacts.length > 0 && (
                <button
                  type="button"
                  onClick={onOpenInsights}
                  className="inline-flex items-center rounded-md border border-accent/25 bg-accent/10 px-2 py-1 text-xs font-medium text-accent transition-colors hover:bg-accent/15"
                >
                  {visibleSuggestedGraphArtifacts.length} {t('knowledge.graphSuggestions')}
                </button>
              )}
              <Badge variant="info">
                {visibleNodes.length}/{totalGraphNodes || visibleNodes.length} {t('knowledge.nodes')}
              </Badge>
              <Badge variant="success">{visibleRelationBundles.length} {t('knowledge.relationBundles')}</Badge>
              <Badge variant="default">{visibleEdges.length}/{totalGraphEdges || visibleEdges.length} {t('knowledge.edges')}</Badge>
            </div>
          </div>

          {loading && (
            <div className="absolute right-4 top-16 z-10 inline-flex items-center gap-2 rounded-md border border-border bg-surface-0 px-3 py-2 text-xs text-text-secondary shadow-md">
              <Loader2 size={14} className="block shrink-0 animate-spin text-accent" />
              {t('common.loading')}
            </div>
          )}

          {positionedNodes.length === 0 ? (
            <EmptyState
              icon={<Network size={32} />}
              title={emptyGraphTitle}
              description={emptyGraphDescription}
              action={
                hasActiveGraphFilters
                  ? { label: t('knowledge.clearGraphFilters'), onClick: resetFilters }
                  : undefined
              }
            />
          ) : (
            <div className="h-[562px]">
              <svg
                ref={graphSvgRef}
                viewBox={`${graphViewBox.x} ${graphViewBox.y} ${graphViewBox.width} ${graphViewBox.height}`}
                role="img"
                aria-label={t('knowledge.relationshipGraph')}
                className="h-full w-full"
                onPointerMove={handleGraphPointerMove}
                onPointerUp={finishGraphDrag}
                onPointerCancel={finishGraphDrag}
              >
                <defs>
                  <style>
                    {`
                      .kg-edge-line {
                        stroke-linecap: round;
                        stroke-linejoin: round;
                        transition: opacity 160ms ease, stroke-width 160ms ease;
                      }
                      .kg-edge-transfer {
                        pointer-events: none;
                      }
                      .kg-edge-comet {
                        fill: currentColor;
                        filter: url(#knowledge-transfer-glow);
                      }
                      .kg-edge-comet-core {
                        fill: var(--graph-transfer-core);
                        filter: url(#knowledge-transfer-glow);
                      }
                      .kg-node-hit {
                        pointer-events: all;
                      }
                      .kg-node-core {
                        filter: url(#knowledge-node-frost);
                        transition: filter 160ms ease;
                      }
                      .kg-node-shell {
                        filter: url(#knowledge-node-glow);
                      }
                      .kg-label-chip {
                        fill: var(--graph-label-background);
                        stroke: var(--graph-label-border);
                        filter: url(#knowledge-label-shadow);
                      }
                    `}
                  </style>
                  <radialGradient id="knowledge-canvas-glow" cx="50%" cy="48%" r="62%">
                    <stop offset="0%" stopColor="var(--graph-canvas-glow-center)" />
                    <stop offset="54%" stopColor="var(--graph-canvas-glow-mid)" />
                    <stop offset="100%" stopColor="var(--graph-canvas-glow-edge)" />
                  </radialGradient>
                  <filter id="knowledge-node-frost" x="-90%" y="-90%" width="280%" height="280%" colorInterpolationFilters="sRGB">
                    <feTurbulence type="fractalNoise" baseFrequency="1.1" numOctaves="2" seed="9" result="noise" />
                    <feColorMatrix
                      in="noise"
                      type="matrix"
                      values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0.034 0.1144 0.0115 0 0"
                      result="grain-mask"
                    />
                    <feFlood floodColor="var(--graph-node-frost)" result="grain-color" />
                    <feComposite in="grain-color" in2="grain-mask" operator="in" result="grain" />
                    <feComposite in="grain" in2="SourceAlpha" operator="in" result="masked-grain" />
                    <feBlend in="SourceGraphic" in2="masked-grain" mode="screen" result="frosted" />
                    <feDropShadow
                      in="frosted"
                      dx="0"
                      dy="4"
                      stdDeviation="5"
                      floodColor="var(--graph-shadow-color)"
                      floodOpacity="var(--graph-shadow-opacity)"
                    />
                  </filter>
                  <filter id="knowledge-node-glow" x="-120%" y="-120%" width="340%" height="340%">
                    <feGaussianBlur stdDeviation="4" result="blur" />
                    <feMerge>
                      <feMergeNode in="blur" />
                      <feMergeNode in="SourceGraphic" />
                    </feMerge>
                  </filter>
                  <filter id="knowledge-transfer-soften" x="-160%" y="-220%" width="420%" height="540%">
                    <feGaussianBlur stdDeviation="1.6" result="blur" />
                    <feMerge>
                      <feMergeNode in="blur" />
                      <feMergeNode in="SourceGraphic" />
                    </feMerge>
                  </filter>
                  <filter id="knowledge-transfer-glow" x="-180%" y="-180%" width="460%" height="460%">
                    <feGaussianBlur stdDeviation="2.4" result="blur" />
                    <feMerge>
                      <feMergeNode in="blur" />
                      <feMergeNode in="SourceGraphic" />
                    </feMerge>
                  </filter>
                  <filter id="knowledge-label-shadow" x="-30%" y="-80%" width="160%" height="260%">
                    <feDropShadow
                      dx="0"
                      dy="4"
                      stdDeviation="4"
                      floodColor="var(--graph-shadow-color)"
                      floodOpacity="var(--graph-shadow-opacity)"
                    />
                  </filter>
                  {Object.values(RELATION_CATEGORY_STYLE).map((style) => (
                    <marker
                      key={style.id}
                      id={`knowledge-edge-arrow-${style.id}`}
                      markerWidth="9"
                      markerHeight="9"
                      refX="6"
                      refY="2.5"
                      orient="auto-start-reverse"
                      markerUnits="strokeWidth"
                    >
                      <path d="M0,0 L0,5 L6,2.5 z" fill={style.color} />
                    </marker>
                  ))}
                </defs>
                <rect
                  x={graphViewBox.x}
                  y={graphViewBox.y}
                  width={graphViewBox.width}
                  height={graphViewBox.height}
                  className="fill-surface-1"
                />
                <rect
                  x={graphViewBox.x}
                  y={graphViewBox.y}
                  width={graphViewBox.width}
                  height={graphViewBox.height}
                  fill="url(#knowledge-canvas-glow)"
                  className="pointer-events-none"
                />
                <g className="opacity-24">
                  {Array.from({ length: 9 }).map((_, index) => (
                    <line
                      key={`v-${index}`}
                      x1={120 + index * 96}
                      y1={64}
                      x2={120 + index * 96}
                      y2={VIEWBOX_HEIGHT - 64}
                      className="stroke-border"
                      strokeWidth="1"
                    />
                  ))}
                  {Array.from({ length: 5 }).map((_, index) => (
                    <line
                      key={`h-${index}`}
                      x1={80}
                      y1={110 + index * 96}
                      x2={VIEWBOX_WIDTH - 80}
                      y2={110 + index * 96}
                      className="stroke-border"
                      strokeWidth="1"
                    />
                  ))}
                </g>

                {visibleSuggestedGraphArtifacts.length > 0 && (
                  <g>
                    {visibleSuggestedGraphArtifacts.map((artifact) => {
                      const source = nodeById.get(artifact.sourceEntityId);
                      const target = nodeById.get(artifact.targetEntityId);
                      if (!source || !target) return null;
                      const path = edgePath(source, target, artifact.kind === 'entity_merge_candidate' ? 18 : -18);
                      const midpoint = bundleMidpoint(source, target);
                      const color = artifact.kind === 'entity_merge_candidate' ? '#7c3aed' : '#0891b2';
                      const label = artifact.kind === 'entity_merge_candidate'
                        ? t('knowledge.graphSuggestionMerge')
                        : relationLabel(artifact.relationType, t);
                      return (
                        <g
                          key={artifact.id}
                          role="button"
                          tabIndex={0}
                          onClick={onOpenInsights}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter' || event.key === ' ') onOpenInsights?.();
                          }}
                          className="cursor-pointer"
                        >
                          <title>{artifact.title}</title>
                          <path
                            d={path}
                            fill="none"
                            stroke={color}
                            strokeWidth="1.7"
                            strokeDasharray="8 7"
                            opacity="0.88"
                            className="kg-edge-line"
                          />
                          <g transform={`translate(${midpoint.x - 50} ${midpoint.y - 36})`}>
                            <rect
                              width="100"
                              height="24"
                              rx="7"
                              fill="var(--graph-suggestion-background)"
                              stroke={color}
                              strokeOpacity="0.45"
                            />
                            <text x="50" y="16" textAnchor="middle" fill={color} className="text-[10px] font-semibold">
                              {label.slice(0, 14)}
                            </text>
                          </g>
                        </g>
                      );
                    })}
                  </g>
                )}

                <g>
                  {visibleRelationBundles.map((bundle, bundleIndex) => {
                    const source = nodeById.get(bundle.source);
                    const target = nodeById.get(bundle.target);
                    if (!source || !target) return null;
                    const style = RELATION_CATEGORY_STYLE[bundle.category];
                    const directlySelected = selectedBundle?.id === bundle.id;
                    const connectedToSelectedNode = Boolean(selectedNode && (bundle.source === selectedNode.id || bundle.target === selectedNode.id));
                    const selected = directlySelected || Boolean(graphMode === 'focus' && connectedToSelectedNode);
                    const agentUsed = agentUsedBundleIds.has(bundle.id);
                    const midpoint = bundleMidpoint(source, target);
                    const bundleLabel = `${source.label} ${target.label}, ${bundle.relationCount} relations`;
                    const path = edgePath(source, target);
                    const showRelationBadge = bundle.relationCount > 1 || directlySelected || agentUsed;
                    const pulsing = directlySelected || agentUsed || Boolean(graphMode === 'focus' && connectedToSelectedNode);
                    const bundleWaveDuration = directlySelected || agentUsed ? '2.6s' : '3.4s';
                    const bundleWaveBegin = `${(bundleIndex % 7) * 0.42}s`;
                    const bundleDash = bundle.edges.every((edge) => edge.evidenceSource === 'cooccurrence') ? '3 5' : style.dash;
                    const selectBundle = () => {
                      handleSelectBundle(bundle.id);
                    };
                    return (
                      <g
                        key={bundle.id}
                        role="button"
                        tabIndex={0}
                        aria-label={bundleLabel}
                        onClick={selectBundle}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') selectBundle();
                        }}
                        className={`cursor-pointer outline-none transition-opacity ${selected || agentUsed ? 'opacity-100' : 'opacity-45'}`}
                      >
                        {showExpandedRelations ? (
                          bundle.edges.map((edge, index) => {
                            const edgeSource = nodeById.get(edge.source);
                            const edgeTarget = nodeById.get(edge.target);
                            if (!edgeSource || !edgeTarget) return null;
                            const edgeStyle = RELATION_CATEGORY_STYLE[buildRelationBundles([edge])[0]?.category ?? 'general'];
                            const expandedPath = edgePath(edgeSource, edgeTarget, relationOffset(index, bundle.edges.length));
                            const edgePulsing = selected || agentUsedEdgeIds.has(edge.id);
                            const edgeWaveDuration = directlySelected || agentUsedEdgeIds.has(edge.id) ? '2.6s' : '3.4s';
                            const edgeWaveBegin = `${(index % 6) * 0.42}s`;
                            const dash = edge.evidenceSource === 'cooccurrence' ? '3 5' : edgeStyle.dash;
                            return (
                              <g key={edge.id}>
                                <path
                                  d={expandedPath}
                                  fill="none"
                                  className="kg-edge-line"
                                  stroke={agentUsedEdgeIds.has(edge.id) ? '#f59e0b' : edgeStyle.color}
                                  strokeWidth={selected ? 1.25 : agentUsedEdgeIds.has(edge.id) ? 1.15 : 0.9}
                                  opacity={selected || agentUsedEdgeIds.has(edge.id) ? 0.82 : 0.58}
                                  strokeDasharray={dash}
                                  markerEnd={`url(#knowledge-edge-arrow-${edgeStyle.id})`}
                                />
                                {edgePulsing && (
                                  <g className="kg-edge-transfer" style={{ color: edgeStyle.color }}>
                                    <animateMotion dur={edgeWaveDuration} begin={edgeWaveBegin} repeatCount="indefinite" path={expandedPath} rotate="auto" />
                                    <animate
                                      attributeName="opacity"
                                      values="0;1;1;0"
                                      keyTimes="0;0.08;0.88;1"
                                      dur={edgeWaveDuration}
                                      begin={edgeWaveBegin}
                                      repeatCount="indefinite"
                                    />
                                    <path
                                      d="M 2.6 0 C 1.2 -1.6 -3 -2.8 -10 -2 C -16 -1.2 -22 -0.4 -22 0 C -22 0.4 -16 1.2 -10 2 C -3 2.8 1.2 1.6 2.6 0 Z"
                                      className="kg-edge-comet"
                                    />
                                    <circle r="1.05" className="kg-edge-comet-core" />
                                  </g>
                                )}
                              </g>
                            );
                          })
                        ) : (
                          <>
                            <path
                              d={path}
                              fill="none"
                              className="kg-edge-line"
                              stroke={agentUsed ? '#f59e0b' : style.color}
                              strokeWidth={selected ? 1.25 : agentUsed ? 1.15 : bundle.relationCount > 1 ? 1 : 0.85}
                              opacity={selected || agentUsed ? 0.84 : 0.56}
                              strokeDasharray={bundleDash}
                              markerStart={bundle.direction === 'bidirectional' ? `url(#knowledge-edge-arrow-${style.id})` : undefined}
                              markerEnd={bundle.direction !== 'undirected' ? `url(#knowledge-edge-arrow-${style.id})` : undefined}
                            />
                            {pulsing && (
                              <g className="kg-edge-transfer" style={{ color: agentUsed ? '#f59e0b' : style.color }}>
                                <animateMotion
                                  dur={bundleWaveDuration}
                                  begin={bundleWaveBegin}
                                  repeatCount="indefinite"
                                  path={path}
                                  rotate="auto"
                                />
                                <animate
                                  attributeName="opacity"
                                  values="0;1;1;0"
                                  keyTimes="0;0.08;0.88;1"
                                  dur={bundleWaveDuration}
                                  begin={bundleWaveBegin}
                                  repeatCount="indefinite"
                                />
                                <path
                                  d="M 2.8 0 C 1.4 -1.7 -3.2 -3 -10.5 -2.2 C -17 -1.3 -23.5 -0.4 -23.5 0 C -23.5 0.4 -17 1.3 -10.5 2.2 C -3.2 3 1.4 1.7 2.8 0 Z"
                                  className="kg-edge-comet"
                                />
                                <circle r="1.15" className="kg-edge-comet-core" />
                              </g>
                            )}
                          </>
                        )}
                        {showRelationBadge && (
                          <g transform={`translate(${midpoint.x - 48} ${midpoint.y - 13})`}>
                            <rect width="96" height="26" rx="7" className="fill-surface-0 stroke-border" />
                            <text x="24" y="17" textAnchor="middle" fill={style.color} className="text-[11px] font-semibold">
                              {bundle.relationCount}
                            </text>
                            <text x="62" y="17" textAnchor="middle" className="fill-text-secondary text-[10px]">
                              {selected ? relationLabel(bundle.relationTypes[0], t).slice(0, 11) : relationCategoryLabel(bundle.category, t).slice(0, 10)}
                            </text>
                          </g>
                        )}
                      </g>
                    );
                  })}
                </g>

                <g>
                  {positionedNodes.map((node) => {
                    const tone = entityTone(node.entityType);
                    const selected = selectedNode?.id === node.id;
                    const agentUsed = agentUsedNodeIds.has(node.id);
                    const highlighted = selectedBundle
                      ? selectedBundleNodeIds.has(node.id)
                      : Boolean(graphMode === 'focus' && selectedNodeEdges.some((edge) => edge.source === node.id || edge.target === node.id));
                    const muted = selectedBundle
                      ? !selectedBundleNodeIds.has(node.id) && !agentUsed
                      : graphMode === 'focus' && selectedNode && !selected && !agentUsed && !highlighted;
                    const showLabel = selected || agentUsed || highlighted || positionedNodes.length <= 36 || node.rank < (graphMode === 'focus' ? 16 : 10);
                    const label = truncateNodeLabel(node.label, selected ? 28 : 20);
                    const labelWidth = Math.min(168, Math.max(54, label.length * 7.5 + 20));
                    return (
                      <g
                        key={node.id}
                        role="button"
                        tabIndex={0}
                        onPointerDown={(event) => handleNodePointerDown(event, node.id)}
                        onClick={(event) => {
                          if (suppressClickNodeRef.current === node.id) {
                            event.preventDefault();
                            event.stopPropagation();
                            suppressClickNodeRef.current = null;
                            return;
                          }
                          handleSelectNode(node.id);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') {
                            handleSelectNode(node.id);
                          }
                        }}
                        aria-label={`${node.label}, ${entityTypeLabel(node.entityType, t)}`}
                        className={`kg-node outline-none transition-opacity ${draggingNodeId === node.id ? 'cursor-grabbing' : 'cursor-grab'} ${muted ? 'opacity-38' : 'opacity-100'}`}
                        style={{ touchAction: 'none' }}
                      >
                        <title>{node.label}</title>
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={Math.max(22, node.radius + 12)}
                          className="kg-node-hit fill-transparent"
                        />
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={node.radius + (selected ? 5 : agentUsed ? 4 : 0)}
                          fill={selected ? 'rgba(37, 99, 235, 0.10)' : agentUsed ? 'rgba(245, 158, 11, 0.10)' : 'transparent'}
                          stroke={selected ? '#2563eb' : agentUsed ? '#f59e0b' : 'transparent'}
                          className={selected || agentUsed ? 'kg-node-shell' : undefined}
                          strokeWidth="1.4"
                        >
                          {(selected || agentUsed) && (
                            <>
                              <animate attributeName="r" values={`${node.radius + 3};${node.radius + 9};${node.radius + 3}`} dur="2.8s" repeatCount="indefinite" />
                              <animate attributeName="opacity" values="0.72;0.16;0.72" dur="2.8s" repeatCount="indefinite" />
                            </>
                          )}
                        </circle>
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={node.radius}
                          fill={tone.solid}
                          stroke={tone.strokeColor}
                          className="kg-node-core"
                          opacity={muted ? 0.72 : 0.94}
                          strokeWidth={selected ? 2.1 : 1.15}
                        />
                        {showLabel && (
                          <g className="pointer-events-none" transform={`translate(${node.x - labelWidth / 2} ${node.y - node.radius - 26})`}>
                            <rect
                              width={labelWidth}
                              height={graphMode === 'focus' ? 25 : 21}
                              rx={graphMode === 'focus' ? 8 : 6}
                              className="kg-label-chip"
                              opacity={selected || agentUsed || graphMode === 'focus' ? 0.94 : 0.76}
                            />
                            <text
                              x={labelWidth / 2}
                              y={graphMode === 'focus' ? 17 : 14}
                              textAnchor="middle"
                              className={`${graphMode === 'focus' ? 'text-[12px]' : 'text-[10px]'} fill-text-primary font-semibold`}
                            >
                              {label}
                            </text>
                          </g>
                        )}
                        {(selected || agentUsed) && (
                          <text
                            x={node.x}
                            y={node.y + node.radius + 17}
                            textAnchor="middle"
                            className="pointer-events-none fill-text-tertiary text-[10px]"
                          >
                            {node.documentCount} / {node.degree}
                          </text>
                        )}
                      </g>
                    );
                  })}
                </g>
              </svg>
            </div>
          )}
        </section>

        <aside className="min-h-[620px] rounded-lg border border-border bg-surface-1">
          {selectedBundle ? (
            <RelationBundleDetail
              bundle={selectedBundle}
              nodeById={nodeById}
              graphContext={selectedGraphContext}
              onUseAsContext={handleUseAsContext}
              onAskAgent={handleAskAgent}
            />
          ) : selectedNode ? (
            <NodeDetail
              node={selectedNode}
              edges={selectedNodeEdges}
              bundles={selectedNodeBundles}
              nodeById={nodeById}
              onSelectBundle={handleSelectBundle}
              graphContext={selectedGraphContext}
              onUseAsContext={handleUseAsContext}
              onAskAgent={handleAskAgent}
            />
          ) : (
            <EmptyState
              icon={<CircleDot size={32} />}
              title={t('knowledge.noNodeSelected')}
              description={t('knowledge.noNodeSelectedDescription')}
            />
          )}
        </aside>
      </div>
    </div>
  );
}

function NodeDetail({
  node,
  edges,
  bundles,
  nodeById,
  onSelectBundle,
  graphContext,
  onUseAsContext,
  onAskAgent,
}: {
  node: PositionedNode;
  edges: KnowledgeGraphEdge[];
  bundles: KnowledgeGraphRelationBundle[];
  nodeById: Map<string, PositionedNode>;
  onSelectBundle: (bundleId: string) => void;
  graphContext: GraphAgentContext | null;
  onUseAsContext: () => void;
  onAskAgent: () => void;
}) {
  const { t } = useTranslation();
  const Icon = entityIcon(node.entityType);
  const tone = entityTone(node.entityType);
  const tokenEstimate = graphContext?.tokenEstimate ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-border p-4">
        <div className="flex items-start gap-3">
          <div className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-surface-0 leading-none ${tone.text}`}>
            <Icon size={20} className="block shrink-0" />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="line-clamp-2 text-base font-semibold text-text-primary">{node.label}</h3>
            <div className="mt-2 flex flex-wrap gap-1.5">
              <Badge variant="info">{entityTypeLabel(node.entityType, t)}</Badge>
              <Badge variant="default">{node.documentCount} {t('knowledge.documents')}</Badge>
              <Badge variant="success">{bundles.length} {t('knowledge.relationBundles')}</Badge>
              <Badge variant="default">{edges.length} {t('knowledge.edges')}</Badge>
            </div>
          </div>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-2">
          <Button
            variant="primary"
            size="sm"
            icon={<MessageSquare size={14} />}
            onClick={onAskAgent}
          >
            {t('knowledge.askAgent')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<PlusCircle size={14} />}
            onClick={onUseAsContext}
          >
            {t('knowledge.useAsContext')}
          </Button>
        </div>
        {node.description && (
          <p className="mt-3 text-sm leading-6 text-text-secondary">{node.description}</p>
        )}
        {node.aliases && node.aliases.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {node.aliases.slice(0, 8).map((alias) => (
              <span key={alias} className="rounded bg-surface-0 px-2 py-1 text-[11px] text-text-tertiary">
                {alias}
              </span>
            ))}
          </div>
        )}
        {tokenEstimate && (
          <div className="mt-3 rounded-md border border-border bg-surface-0 p-2">
            <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-text-tertiary">
              {t('knowledge.tokenEstimate')}
            </div>
            <div className="grid grid-cols-3 gap-2 text-center">
              <div className="min-w-0 rounded-md bg-surface-1 px-2 py-1.5">
                <div className="text-xs font-semibold text-text-primary">
                  {formatCompactChars(tokenEstimate.graphIndexChars)}
                </div>
                <div className="truncate text-[10px] text-text-tertiary">{t('knowledge.graphIndex')}</div>
              </div>
              <div className="min-w-0 rounded-md bg-surface-1 px-2 py-1.5">
                <div className="text-xs font-semibold text-text-primary">
                  {formatCompactChars(tokenEstimate.rawRetrievalCharsEstimate)}
                </div>
                <div className="truncate text-[10px] text-text-tertiary">{t('knowledge.rawEvidenceEstimate')}</div>
              </div>
              <div className="min-w-0 rounded-md bg-success/10 px-2 py-1.5">
                <div className="text-xs font-semibold text-success">
                  {tokenEstimate.savedPctEstimate}%
                </div>
                <div className="truncate text-[10px] text-success">{t('knowledge.tokenSavings')}</div>
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <section>
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-text-tertiary">
            <BookOpen size={13} className="block shrink-0" />
            {t('knowledge.evidenceDocuments')}
          </div>
          <div className="space-y-2">
            {node.documents.length === 0 ? (
              <p className="rounded-md border border-dashed border-border px-3 py-4 text-sm text-text-tertiary">
                {t('knowledge.noEvidenceDocuments')}
              </p>
            ) : (
              node.documents.map((doc) => (
                <div key={doc.documentId} className="rounded-md border border-border bg-surface-0 px-3 py-2">
                  <div className="line-clamp-1 text-sm font-medium text-text-primary">{doc.title}</div>
                  <div className="mt-1 flex items-center gap-1 text-[11px] text-text-tertiary">
                    <ExternalLink size={11} className="block shrink-0" />
                    <span className="truncate">{shortPath(doc.path)}</span>
                  </div>
                </div>
              ))
            )}
          </div>
        </section>

        <section className="mt-5">
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-text-tertiary">
            <GitFork size={13} className="block shrink-0" />
            {t('knowledge.connectedRelations')}
          </div>
          <div className="space-y-2">
            {bundles.length === 0 ? (
              <p className="rounded-md border border-dashed border-border px-3 py-4 text-sm text-text-tertiary">
                {t('knowledge.noRelations')}
              </p>
            ) : (
              bundles.map((bundle) => {
                const otherId = bundle.source === node.id ? bundle.target : bundle.source;
                const other = nodeById.get(otherId);
                const style = RELATION_CATEGORY_STYLE[bundle.category];
                return (
                  <button
                    key={bundle.id}
                    type="button"
                    onClick={() => onSelectBundle(bundle.id)}
                    className="w-full rounded-md border border-border bg-surface-0 px-3 py-2 text-left transition-colors hover:border-border-hover hover:bg-surface-2"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="min-w-0">
                        <div className="truncate text-sm font-medium text-text-primary">
                          {other?.label ?? otherId}
                        </div>
                        <div className="mt-1 flex flex-wrap gap-1">
                          {bundle.relationTypes.slice(0, 3).map((type) => (
                            <span key={type} className="rounded bg-surface-3 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                              {relationLabel(type, t)}
                            </span>
                          ))}
                        </div>
                      </div>
                      <div className="flex shrink-0 flex-col items-end gap-1">
                        <Badge variant={style.badge}>{bundle.relationCount} {t('knowledge.edges')}</Badge>
                        <span className="text-[10px] text-text-tertiary">{bundle.strongestStrength.toFixed(1)}</span>
                      </div>
                    </div>
                    {bundle.evidenceTitles.length > 0 ? (
                      <div className="mt-2 truncate text-[11px] text-text-tertiary">
                        {bundle.evidenceTitles[0]}
                      </div>
                    ) : null}
                  </button>
                );
              })
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

function RelationBundleDetail({
  bundle,
  nodeById,
  graphContext,
  onUseAsContext,
  onAskAgent,
}: {
  bundle: KnowledgeGraphRelationBundle;
  nodeById: Map<string, PositionedNode>;
  graphContext: GraphAgentContext | null;
  onUseAsContext: () => void;
  onAskAgent: () => void;
}) {
  const { t } = useTranslation();
  const source = nodeById.get(bundle.source);
  const target = nodeById.get(bundle.target);
  const style = RELATION_CATEGORY_STYLE[bundle.category];
  const tokenEstimate = graphContext?.tokenEstimate ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-border p-4">
        <div className="flex items-start gap-3">
          <div className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-surface-0 leading-none ${style.text}`}>
            <ArrowLeftRight size={20} className="block shrink-0" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-xs font-semibold uppercase tracking-[0.12em] text-text-tertiary">
              {t('knowledge.relationshipBundle')}
            </div>
            <h3 className="mt-1 line-clamp-2 text-base font-semibold text-text-primary">
              {source?.label ?? bundle.source} <span className="text-text-tertiary">&lt;-&gt;</span> {target?.label ?? bundle.target}
            </h3>
            <div className="mt-2 flex flex-wrap gap-1.5">
              <Badge variant={style.badge}>{relationCategoryLabel(bundle.category, t)}</Badge>
              <Badge variant="default">{relationDirectionLabel(bundle.direction, t)}</Badge>
              <Badge variant="info">{bundle.relationCount} {t('knowledge.edges')}</Badge>
            </div>
          </div>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-2">
          <Button
            variant="primary"
            size="sm"
            icon={<MessageSquare size={14} />}
            onClick={onAskAgent}
          >
            {t('knowledge.askAgent')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<PlusCircle size={14} />}
            onClick={onUseAsContext}
          >
            {t('knowledge.useAsContext')}
          </Button>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-2">
          <div className="rounded-md border border-border bg-surface-0 px-3 py-2">
            <div className="text-[10px] uppercase tracking-[0.12em] text-text-tertiary">{t('knowledge.strongestStrength')}</div>
            <div className="mt-1 text-sm font-semibold text-text-primary">{bundle.strongestStrength.toFixed(2)}</div>
          </div>
          <div className="rounded-md border border-border bg-surface-0 px-3 py-2">
            <div className="text-[10px] uppercase tracking-[0.12em] text-text-tertiary">{t('knowledge.averageStrength')}</div>
            <div className="mt-1 text-sm font-semibold text-text-primary">{bundle.averageStrength.toFixed(2)}</div>
          </div>
        </div>
        {tokenEstimate && (
          <div className="mt-3 rounded-md border border-border bg-surface-0 p-2">
            <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-text-tertiary">
              {t('knowledge.tokenEstimate')}
            </div>
            <div className="grid grid-cols-3 gap-2 text-center">
              <div className="min-w-0 rounded-md bg-surface-1 px-2 py-1.5">
                <div className="text-xs font-semibold text-text-primary">
                  {formatCompactChars(tokenEstimate.graphIndexChars)}
                </div>
                <div className="truncate text-[10px] text-text-tertiary">{t('knowledge.graphIndex')}</div>
              </div>
              <div className="min-w-0 rounded-md bg-surface-1 px-2 py-1.5">
                <div className="text-xs font-semibold text-text-primary">
                  {formatCompactChars(tokenEstimate.rawRetrievalCharsEstimate)}
                </div>
                <div className="truncate text-[10px] text-text-tertiary">{t('knowledge.rawEvidenceEstimate')}</div>
              </div>
              <div className="min-w-0 rounded-md bg-success/10 px-2 py-1.5">
                <div className="text-xs font-semibold text-success">
                  {tokenEstimate.savedPctEstimate}%
                </div>
                <div className="truncate text-[10px] text-success">{t('knowledge.tokenSavings')}</div>
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <section>
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-text-tertiary">
            <GitFork size={13} className="block shrink-0" />
            {t('knowledge.relationsInBundle')}
          </div>
          <div className="space-y-2">
            {bundle.edges.map((edge) => {
              const edgeSource = nodeById.get(edge.source);
              const edgeTarget = nodeById.get(edge.target);
              const evidenceTitle = edge.evidenceTitle || edge.evidenceTitles?.[0] || (edge.evidencePath ? shortPath(edge.evidencePath) : null);
              const evidenceSource = edge.evidenceSource === 'cooccurrence' ? 'co-occurrence' : 'explicit';
              return (
                <div key={edge.id} className="rounded-md border border-border bg-surface-0 px-3 py-2">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium text-text-primary">
                        {edgeSource?.label ?? edge.source} {'->'} {edgeTarget?.label ?? edge.target}
                      </div>
                      <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs text-text-tertiary">
                        <span>{relationLabel(edge.relationType, t)}</span>
                        <span className="rounded bg-surface-2 px-1.5 py-0.5 text-[10px]">{evidenceSource}</span>
                        {edge.evidenceCount ? (
                          <span className="rounded bg-surface-2 px-1.5 py-0.5 text-[10px]">{edge.evidenceCount} evidence</span>
                        ) : null}
                        {typeof edge.confidence === 'number' ? (
                          <span className="rounded bg-surface-2 px-1.5 py-0.5 text-[10px]">{Math.round(edge.confidence * 100)}%</span>
                        ) : null}
                      </div>
                    </div>
                    <Badge variant="default">{edge.strength.toFixed(1)}</Badge>
                  </div>
                  {evidenceTitle ? (
                    <div className="mt-2 truncate text-[11px] text-text-tertiary">
                      {evidenceTitle}
                    </div>
                  ) : null}
                  {edge.evidenceSnippet ? (
                    <div className="mt-2 line-clamp-2 rounded bg-surface-2 px-2 py-1.5 text-[11px] leading-5 text-text-secondary">
                      {edge.evidenceSnippet}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </section>
      </div>
    </div>
  );
}
