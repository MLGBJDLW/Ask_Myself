import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
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

type EntityFilter = 'all' | 'person' | 'place' | 'organization' | 'event' | 'concept';
type Translate = ReturnType<typeof useTranslation>['t'];

type PositionedNode = KnowledgeGraphNode & {
  x: number;
  y: number;
  radius: number;
  degree: number;
};

const ENTITY_FILTERS: EntityFilter[] = ['all', 'person', 'place', 'organization', 'event', 'concept'];
const ENTITY_FILTER_LABEL_KEYS: Record<EntityFilter, TranslationKey> = {
  all: 'knowledge.entityFilter.all',
  person: 'knowledge.entityFilter.person',
  place: 'knowledge.entityFilter.place',
  organization: 'knowledge.entityFilter.organization',
  event: 'knowledge.entityFilter.event',
  concept: 'knowledge.entityFilter.concept',
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
const RELATION_TYPE_LABEL_KEYS: Record<string, TranslationKey> = {
  co_occurs: 'knowledge.relationType.coOccurs',
};
const VIEWBOX_WIDTH = 1000;
const VIEWBOX_HEIGHT = 620;
const CENTER_X = VIEWBOX_WIDTH / 2;
const CENTER_Y = VIEWBOX_HEIGHT / 2;

const ENTITY_TONE: Record<string, { fill: string; stroke: string; text: string }> = {
  person: { fill: 'fill-accent/15', stroke: 'stroke-accent', text: 'text-accent-hover' },
  place: { fill: 'fill-info/15', stroke: 'stroke-info', text: 'text-info' },
  organization: { fill: 'fill-warning/15', stroke: 'stroke-warning', text: 'text-warning' },
  event: { fill: 'fill-danger/15', stroke: 'stroke-danger', text: 'text-danger' },
  concept: { fill: 'fill-success/15', stroke: 'stroke-success', text: 'text-success' },
  technology: { fill: 'fill-info/15', stroke: 'stroke-info', text: 'text-info' },
  other: { fill: 'fill-surface-3', stroke: 'stroke-text-tertiary', text: 'text-text-secondary' },
};
const RELATION_CATEGORY_STYLE: Record<RelationCategory, {
  id: RelationCategory;
  stroke: string;
  fill: string;
  text: string;
  badge: 'default' | 'success' | 'warning' | 'danger' | 'info';
  dash?: string;
}> = {
  conflict: { id: 'conflict', stroke: 'stroke-danger', fill: 'fill-danger', text: 'text-danger', badge: 'danger', dash: '8 5' },
  causal: { id: 'causal', stroke: 'stroke-warning', fill: 'fill-warning', text: 'text-warning', badge: 'warning' },
  hierarchy: { id: 'hierarchy', stroke: 'stroke-info', fill: 'fill-info', text: 'text-info', badge: 'info', dash: '4 3' },
  event: { id: 'event', stroke: 'stroke-success', fill: 'fill-success', text: 'text-success', badge: 'success', dash: '2 4' },
  social: { id: 'social', stroke: 'stroke-accent', fill: 'fill-accent', text: 'text-accent-hover', badge: 'info' },
  general: { id: 'general', stroke: 'stroke-text-tertiary', fill: 'fill-text-tertiary', text: 'text-text-secondary', badge: 'default' },
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

function computeLayout(nodes: KnowledgeGraphNode[], edges: KnowledgeGraphEdge[]): PositionedNode[] {
  const degree = new Map<string, number>();
  for (const edge of edges) {
    degree.set(edge.source, (degree.get(edge.source) ?? 0) + 1);
    degree.set(edge.target, (degree.get(edge.target) ?? 0) + 1);
  }

  const sorted = [...nodes].sort((a, b) => {
    const degreeDelta = (degree.get(b.id) ?? 0) - (degree.get(a.id) ?? 0);
    if (degreeDelta !== 0) return degreeDelta;
    return b.documentCount - a.documentCount || b.mentionCount - a.mentionCount;
  });

  return sorted.map((node, index) => {
    if (index === 0) {
      return {
        ...node,
        x: CENTER_X,
        y: CENTER_Y,
        radius: 42,
        degree: degree.get(node.id) ?? 0,
      };
    }

    const ringIndex = Math.floor((index - 1) / 10);
    const ringStart = 1 + ringIndex * 10;
    const ringSize = Math.min(10 + ringIndex * 4, sorted.length - ringStart);
    const slot = index - ringStart;
    const angleOffset = ringIndex % 2 === 0 ? -Math.PI / 2 : -Math.PI / 2 + Math.PI / Math.max(6, ringSize);
    const angle = angleOffset + (slot / Math.max(1, ringSize)) * Math.PI * 2;
    const radius = Math.min(280, 165 + ringIndex * 92);
    const x = CENTER_X + Math.cos(angle) * radius;
    const y = CENTER_Y + Math.sin(angle) * radius * 0.78;
    const nodeDegree = degree.get(node.id) ?? 0;
    const nodeRadius = Math.min(38, 24 + Math.sqrt(Math.max(1, node.mentionCount + nodeDegree * 2)) * 2.4);

    return {
      ...node,
      x,
      y,
      radius: nodeRadius,
      degree: nodeDegree,
    };
  });
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

export function KnowledgeGraphView() {
  const { t } = useTranslation();
  const navigate = useNavigate();
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
  const [agentUsage, setAgentUsage] = useState(() => readGraphAgentUsage());

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
        limit: 90,
        sourceId: selectedSourceId || null,
        pathPrefix: pathPrefix.trim() || null,
        entityTypes: entityFilter === 'all' ? [] : [entityFilter],
        relationTypes: relationFilter ? [relationFilter] : [],
      });
      setGraph(nextGraph);
      setSelectedNodeId((current) => {
        if (current && nextGraph.nodes.some((node) => node.id === current)) return current;
        return nextGraph.nodes[0]?.id ?? null;
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

  useEffect(() => {
    void loadSources();
  }, [loadSources]);

  useEffect(() => {
    void loadGraph();
  }, [loadGraph]);

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
  const relationTypes = useMemo(() => {
    const values = new Set(graph?.edges.map((edge) => edge.relationType) ?? []);
    return [...values].sort((a, b) => a.localeCompare(b));
  }, [graph]);

  const visibleNodeIds = useMemo(() => {
    const query = trimmedSearchText.toLowerCase();
    const ids = new Set<string>();
    for (const node of graph?.nodes ?? []) {
      if (!query || node.label.toLowerCase().includes(query) || node.description.toLowerCase().includes(query)) {
        ids.add(node.id);
      }
    }
    return ids;
  }, [graph, trimmedSearchText]);

  const visibleNodes = useMemo(
    () => (graph?.nodes ?? []).filter((node) => visibleNodeIds.has(node.id)),
    [graph, visibleNodeIds],
  );

  const visibleEdges = useMemo(
    () => (graph?.edges ?? []).filter((edge) => visibleNodeIds.has(edge.source) && visibleNodeIds.has(edge.target)),
    [graph, visibleNodeIds],
  );
  const visibleRelationBundles = useMemo(() => buildRelationBundles(visibleEdges), [visibleEdges]);

  const positionedNodes = useMemo(() => computeLayout(visibleNodes, visibleEdges), [visibleNodes, visibleEdges]);
  const nodeById = useMemo(() => new Map(positionedNodes.map((node) => [node.id, node])), [positionedNodes]);
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

  const resetFilters = () => {
    setSelectedSourceId('');
    setPathPrefix('');
    setEntityFilter('all');
    setRelationFilter('');
    setSearchText('');
    setSelectedBundleId(null);
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
            <select
              value={selectedSourceId}
              onChange={(event) => setSelectedSourceId(event.target.value)}
              className="h-10 w-full rounded-md border border-border bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            >
              <option value="">{t('knowledge.allSources')}</option>
              {sources.map((source) => (
                <option key={source.id} value={source.id}>{shortPath(source.rootPath)}</option>
              ))}
            </select>
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

        <div className="mt-3 grid gap-3 lg:grid-cols-[minmax(240px,1fr)_minmax(180px,0.55fr)]">
          <Input
            icon={<Search size={15} />}
            placeholder={t('knowledge.searchGraph')}
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
          />
          <label className="relative">
            <Filter size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
            <select
              value={relationFilter}
              onChange={(event) => setRelationFilter(event.target.value)}
              className="h-10 w-full rounded-md border border-border bg-surface-0 py-0 pl-10 pr-3 text-sm text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            >
              <option value="">{t('knowledge.allRelations')}</option>
              {relationTypes.map((relation) => (
                <option key={relation} value={relation}>{relationLabel(relation, t)}</option>
              ))}
            </select>
          </label>
        </div>
      </section>

      <div className="grid min-h-[620px] gap-4 xl:grid-cols-[minmax(0,1fr)_340px]">
        <section className="relative min-h-[620px] overflow-hidden rounded-lg border border-border bg-surface-1">
          <div className="flex items-center justify-between border-b border-border px-4 py-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                <Network size={16} className="text-accent" />
                {t('knowledge.relationshipGraph')}
              </div>
              <p className="mt-0.5 truncate text-xs text-text-tertiary">
                {graphScopeLabel}
              </p>
            </div>
            <div className="flex shrink-0 gap-1.5">
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
              <Badge variant="info">{visibleNodes.length} {t('knowledge.nodes')}</Badge>
              <Badge variant="success">{visibleRelationBundles.length} {t('knowledge.relationBundles')}</Badge>
              <Badge variant="default">{visibleEdges.length} {t('knowledge.edges')}</Badge>
            </div>
          </div>

          {loading && (
            <div className="absolute right-4 top-16 z-10 inline-flex items-center gap-2 rounded-md border border-border bg-surface-0 px-3 py-2 text-xs text-text-secondary shadow-md">
              <Loader2 size={14} className="animate-spin text-accent" />
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
            <div className="h-[calc(100%-58px)] min-h-[562px]">
              <svg
                viewBox={`0 0 ${VIEWBOX_WIDTH} ${VIEWBOX_HEIGHT}`}
                role="img"
                aria-label={t('knowledge.relationshipGraph')}
                className="h-full w-full"
              >
                <defs>
                  {Object.values(RELATION_CATEGORY_STYLE).map((style) => (
                    <marker
                      key={style.id}
                      id={`knowledge-edge-arrow-${style.id}`}
                      markerWidth="9"
                      markerHeight="9"
                      refX="7"
                      refY="3"
                      orient="auto-start-reverse"
                      markerUnits="strokeWidth"
                    >
                      <path d="M0,0 L0,6 L7,3 z" className={style.fill} />
                    </marker>
                  ))}
                </defs>
                <rect width={VIEWBOX_WIDTH} height={VIEWBOX_HEIGHT} className="fill-surface-1" />
                <g className="opacity-40">
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

                <g>
                  {visibleRelationBundles.map((bundle) => {
                    const source = nodeById.get(bundle.source);
                    const target = nodeById.get(bundle.target);
                    if (!source || !target) return null;
                    const style = RELATION_CATEGORY_STYLE[bundle.category];
                    const selected =
                      selectedBundle?.id === bundle.id ||
                      Boolean(selectedNode && (bundle.source === selectedNode.id || bundle.target === selectedNode.id));
                    const agentUsed = agentUsedBundleIds.has(bundle.id);
                    const midpoint = bundleMidpoint(source, target);
                    const bundleLabel = `${source.label} ${target.label}, ${bundle.relationCount} relations`;
                    const selectBundle = () => {
                      setSelectedBundleId(bundle.id);
                      setSelectedNodeId(null);
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
                            return (
                              <path
                                key={edge.id}
                                d={edgePath(edgeSource, edgeTarget, relationOffset(index, bundle.edges.length))}
                                fill="none"
                                className={selected ? edgeStyle.stroke : agentUsedEdgeIds.has(edge.id) ? 'stroke-warning' : edgeStyle.stroke}
                                strokeWidth={selected ? 2.2 : agentUsedEdgeIds.has(edge.id) ? 2 : 1.35}
                                strokeDasharray={edgeStyle.dash}
                                markerEnd={`url(#knowledge-edge-arrow-${edgeStyle.id})`}
                              />
                            );
                          })
                        ) : (
                          <path
                            d={edgePath(source, target)}
                            fill="none"
                            className={selected ? style.stroke : agentUsed ? 'stroke-warning' : style.stroke}
                            strokeWidth={selected ? 3 : agentUsed ? 2.6 : bundle.relationCount > 1 ? 2.2 : 1.5}
                            strokeDasharray={style.dash}
                            markerStart={bundle.direction === 'bidirectional' ? `url(#knowledge-edge-arrow-${style.id})` : undefined}
                            markerEnd={bundle.direction !== 'undirected' ? `url(#knowledge-edge-arrow-${style.id})` : undefined}
                          />
                        )}
                        {(bundle.relationCount > 1 || selected || agentUsed) && (
                          <g transform={`translate(${midpoint.x - 48} ${midpoint.y - 13})`}>
                            <rect width="96" height="26" rx="7" className="fill-surface-0 stroke-border" />
                            <text x="24" y="17" textAnchor="middle" className={`${style.fill} text-[11px] font-semibold`}>
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
                    const muted = selectedBundle
                      ? !selectedBundleNodeIds.has(node.id) && !agentUsed
                      : selectedNode && !selected && !agentUsed && !selectedNodeEdges.some((edge) => edge.source === node.id || edge.target === node.id);
                    return (
                      <g
                        key={node.id}
                        role="button"
                        tabIndex={0}
                        onClick={() => {
                          setSelectedNodeId(node.id);
                          setSelectedBundleId(null);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') {
                            setSelectedNodeId(node.id);
                            setSelectedBundleId(null);
                          }
                        }}
                        aria-label={`${node.label}, ${entityTypeLabel(node.entityType, t)}`}
                        className={`cursor-pointer outline-none transition-opacity ${muted ? 'opacity-45' : 'opacity-100'}`}
                      >
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={node.radius + (selected ? 8 : agentUsed ? 6 : 0)}
                          className={selected ? 'fill-accent-subtle stroke-accent' : agentUsed ? 'fill-warning/10 stroke-warning' : 'fill-transparent stroke-transparent'}
                          strokeWidth="2"
                        />
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={node.radius}
                          className={`${tone.fill} ${tone.stroke}`}
                          strokeWidth={selected ? 3 : 2}
                        />
                        <text
                          x={node.x}
                          y={node.y + 4}
                          textAnchor="middle"
                          className="pointer-events-none fill-text-primary text-[11px] font-semibold"
                        >
                          {node.label.length > 14 ? `${node.label.slice(0, 13)}...` : node.label}
                        </text>
                        <text
                          x={node.x}
                          y={node.y + node.radius + 17}
                          textAnchor="middle"
                          className="pointer-events-none fill-text-tertiary text-[10px]"
                        >
                          {node.documentCount} / {node.degree}
                        </text>
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
              onSelectBundle={(bundleId) => {
                setSelectedBundleId(bundleId);
                setSelectedNodeId(null);
              }}
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
          <div className={`rounded-lg border border-border bg-surface-0 p-2 ${tone.text}`}>
            <Icon size={20} />
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
            <BookOpen size={13} />
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
                    <ExternalLink size={11} />
                    <span className="truncate">{shortPath(doc.path)}</span>
                  </div>
                </div>
              ))
            )}
          </div>
        </section>

        <section className="mt-5">
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-text-tertiary">
            <GitFork size={13} />
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
          <div className={`rounded-lg border border-border bg-surface-0 p-2 ${style.text}`}>
            <ArrowLeftRight size={20} />
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
            <GitFork size={13} />
            {t('knowledge.relationsInBundle')}
          </div>
          <div className="space-y-2">
            {bundle.edges.map((edge) => {
              const edgeSource = nodeById.get(edge.source);
              const edgeTarget = nodeById.get(edge.target);
              return (
                <div key={edge.id} className="rounded-md border border-border bg-surface-0 px-3 py-2">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium text-text-primary">
                        {edgeSource?.label ?? edge.source} {'->'} {edgeTarget?.label ?? edge.target}
                      </div>
                      <div className="mt-1 text-xs text-text-tertiary">{relationLabel(edge.relationType, t)}</div>
                    </div>
                    <Badge variant="default">{edge.strength.toFixed(1)}</Badge>
                  </div>
                  {edge.evidenceTitle || edge.evidencePath ? (
                    <div className="mt-2 truncate text-[11px] text-text-tertiary">
                      {edge.evidenceTitle || shortPath(edge.evidencePath ?? '')}
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
