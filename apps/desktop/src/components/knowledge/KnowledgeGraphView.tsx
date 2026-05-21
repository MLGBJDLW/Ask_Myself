import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  BookOpen,
  Building2,
  CalendarClock,
  CircleDot,
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

function relationLabel(value: string) {
  return value.replace(/_/g, ' ');
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

function edgePath(source: PositionedNode, target: PositionedNode) {
  const dx = target.x - source.x;
  const dy = target.y - source.y;
  const curve = Math.min(80, Math.max(-80, dx * 0.08));
  const cx = (source.x + target.x) / 2 - dy * 0.08;
  const cy = (source.y + target.y) / 2 + curve;
  return `M ${source.x} ${source.y} Q ${cx} ${cy} ${target.x} ${target.y}`;
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
  const relationTypes = useMemo(() => {
    const values = new Set(graph?.edges.map((edge) => edge.relationType) ?? []);
    return [...values].sort((a, b) => a.localeCompare(b));
  }, [graph]);

  const visibleNodeIds = useMemo(() => {
    const query = searchText.trim().toLowerCase();
    const ids = new Set<string>();
    for (const node of graph?.nodes ?? []) {
      if (!query || node.label.toLowerCase().includes(query) || node.description.toLowerCase().includes(query)) {
        ids.add(node.id);
      }
    }
    return ids;
  }, [graph, searchText]);

  const visibleNodes = useMemo(
    () => (graph?.nodes ?? []).filter((node) => visibleNodeIds.has(node.id)),
    [graph, visibleNodeIds],
  );

  const visibleEdges = useMemo(
    () => (graph?.edges ?? []).filter((edge) => visibleNodeIds.has(edge.source) && visibleNodeIds.has(edge.target)),
    [graph, visibleNodeIds],
  );

  const positionedNodes = useMemo(() => computeLayout(visibleNodes, visibleEdges), [visibleNodes, visibleEdges]);
  const nodeById = useMemo(() => new Map(positionedNodes.map((node) => [node.id, node])), [positionedNodes]);
  const nodeLabelById = useMemo(
    () => new Map(positionedNodes.map((node) => [node.id, node.label])),
    [positionedNodes],
  );
  const selectedNode = useMemo(() => {
    if (!selectedNodeId) return positionedNodes[0] ?? null;
    return nodeById.get(selectedNodeId) ?? positionedNodes[0] ?? null;
  }, [nodeById, positionedNodes, selectedNodeId]);
  const selectedNodeEdges = useMemo(() => {
    if (!selectedNode) return [];
    return visibleEdges.filter((edge) => edge.source === selectedNode.id || edge.target === selectedNode.id);
  }, [selectedNode, visibleEdges]);
  const selectedGraphContext = useMemo(() => {
    if (!selectedNode) return null;
    return buildGraphAgentContext({
      sourceId: selectedSourceId || null,
      sourceLabel: selectedSource ? shortPath(selectedSource.rootPath) : null,
      pathPrefix: pathPrefix.trim() || null,
      scopeLabel: graph?.scopeLabel ?? null,
      node: selectedNode,
      edges: selectedNodeEdges,
      nodeLabelById,
    });
  }, [graph?.scopeLabel, nodeLabelById, pathPrefix, selectedNode, selectedNodeEdges, selectedSource, selectedSourceId]);
  const agentUsedNodeIds = useMemo(
    () => new Set(agentUsage?.usedGraphNodes.map((node) => node.id) ?? []),
    [agentUsage],
  );
  const agentUsedEdgeIds = useMemo(
    () => new Set(agentUsage?.usedGraphEdges.map((edge) => edge.id) ?? []),
    [agentUsage],
  );

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
              disabled={!selectedSourceId}
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
                <option key={relation} value={relation}>{relationLabel(relation)}</option>
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
                {selectedSource
                  ? `${shortPath(selectedSource.rootPath)}${pathPrefix.trim() ? ` / ${pathPrefix.trim()}` : ''}`
                  : t('knowledge.allSources')}
              </p>
            </div>
            <div className="flex shrink-0 gap-1.5">
              {agentUsage && agentUsedNodeIds.size > 0 && (
                <Badge variant="warning">
                  {t('knowledge.agentUsedPath')}: {agentUsedNodeIds.size}
                </Badge>
              )}
              <Badge variant="info">{visibleNodes.length} {t('knowledge.nodes')}</Badge>
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
              title={t('knowledge.noGraph')}
              description={t('knowledge.noGraphDescription')}
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
                  <marker id="knowledge-edge-arrow" markerWidth="9" markerHeight="9" refX="7" refY="3" orient="auto" markerUnits="strokeWidth">
                    <path d="M0,0 L0,6 L7,3 z" className="fill-text-tertiary" />
                  </marker>
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
                  {visibleEdges.map((edge) => {
                    const source = nodeById.get(edge.source);
                    const target = nodeById.get(edge.target);
                    if (!source || !target) return null;
                    const path = edgePath(source, target);
                    const selected = selectedNode && (edge.source === selectedNode.id || edge.target === selectedNode.id);
                    const agentUsed = agentUsedEdgeIds.has(edge.id);
                    const midX = (source.x + target.x) / 2;
                    const midY = (source.y + target.y) / 2;
                    return (
                      <g key={edge.id} className={selected || agentUsed ? 'opacity-100' : 'opacity-45'}>
                        <path
                          d={path}
                          fill="none"
                          className={selected ? 'stroke-accent' : agentUsed ? 'stroke-warning' : 'stroke-text-tertiary'}
                          strokeWidth={selected ? 2.6 : agentUsed ? 2.4 : 1.5}
                          markerEnd="url(#knowledge-edge-arrow)"
                        />
                        {selected && (
                          <g transform={`translate(${midX - 42} ${midY - 12})`}>
                            <rect width="84" height="24" rx="6" className="fill-surface-0 stroke-border" />
                            <text x="42" y="16" textAnchor="middle" className="fill-text-secondary text-[10px]">
                              {relationLabel(edge.relationType).slice(0, 16)}
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
                    const muted = selectedNode && !selected && !agentUsed && !selectedNodeEdges.some((edge) => edge.source === node.id || edge.target === node.id);
                    return (
                      <g
                        key={node.id}
                        role="button"
                        tabIndex={0}
                        onClick={() => setSelectedNodeId(node.id)}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') setSelectedNodeId(node.id);
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
          {selectedNode ? (
            <NodeDetail
              node={selectedNode}
              edges={selectedNodeEdges}
              nodeById={nodeById}
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
  nodeById,
  graphContext,
  onUseAsContext,
  onAskAgent,
}: {
  node: PositionedNode;
  edges: KnowledgeGraphEdge[];
  nodeById: Map<string, PositionedNode>;
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
            {edges.length === 0 ? (
              <p className="rounded-md border border-dashed border-border px-3 py-4 text-sm text-text-tertiary">
                {t('knowledge.noRelations')}
              </p>
            ) : (
              edges.map((edge) => {
                const otherId = edge.source === node.id ? edge.target : edge.source;
                const other = nodeById.get(otherId);
                return (
                  <div key={edge.id} className="rounded-md border border-border bg-surface-0 px-3 py-2">
                    <div className="flex items-center justify-between gap-2">
                      <div className="min-w-0">
                        <div className="truncate text-sm font-medium text-text-primary">
                          {other?.label ?? otherId}
                        </div>
                        <div className="mt-1 text-xs text-text-tertiary">{relationLabel(edge.relationType)}</div>
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
              })
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
