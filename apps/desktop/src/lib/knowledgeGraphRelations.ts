import type { KnowledgeGraphEdge } from '../types/knowledge';

export type RelationCategory = 'conflict' | 'causal' | 'hierarchy' | 'event' | 'social' | 'general';
export type RelationDirection = 'directed' | 'bidirectional' | 'undirected';

export interface KnowledgeGraphRelationBundle {
  id: string;
  source: string;
  target: string;
  edges: KnowledgeGraphEdge[];
  edgeIds: string[];
  relationTypes: string[];
  relationCount: number;
  direction: RelationDirection;
  category: RelationCategory;
  strongestStrength: number;
  averageStrength: number;
  evidenceTitles: string[];
}

function relationMatches(value: string, needles: string[]) {
  const normalized = value.toLowerCase();
  return needles.some((needle) => normalized.includes(needle));
}

export function relationCategory(relationTypes: string[]): RelationCategory {
  if (relationTypes.some((value) => relationMatches(value, ['conflict', 'enemy', 'rival', 'threat', 'oppose', 'contradict']))) {
    return 'conflict';
  }
  if (relationTypes.some((value) => relationMatches(value, ['cause', 'lead', 'affect', 'influence', 'enable', 'prevent', 'trigger']))) {
    return 'causal';
  }
  if (relationTypes.some((value) => relationMatches(value, ['parent', 'child', 'belongs', 'member', 'part', 'located', 'contains', 'owns']))) {
    return 'hierarchy';
  }
  if (relationTypes.some((value) => relationMatches(value, ['event', 'appears', 'participates', 'occurs', 'incident', 'meeting']))) {
    return 'event';
  }
  if (relationTypes.some((value) => relationMatches(value, ['friend', 'ally', 'mentor', 'family', 'knows', 'protect', 'trust', 'love']))) {
    return 'social';
  }
  return 'general';
}

function isUndirectedRelationType(relationType: string) {
  const normalized = relationType.toLowerCase();
  return (
    normalized.includes('related') ||
    normalized.includes('similar') ||
    normalized.includes('co_occurs') ||
    normalized.includes('cooccurs') ||
    normalized.includes('associated')
  );
}

export function buildRelationBundles(edges: KnowledgeGraphEdge[]): KnowledgeGraphRelationBundle[] {
  const bundles: KnowledgeGraphRelationBundle[] = [];

  for (const edge of edges) {
    let bundle = bundles.find(
      (candidate) =>
        (candidate.source === edge.source && candidate.target === edge.target) ||
        (candidate.source === edge.target && candidate.target === edge.source),
    );
    if (!bundle) {
      bundle = {
        id: `${edge.source}::${edge.target}`,
        source: edge.source,
        target: edge.target,
        edges: [],
        edgeIds: [],
        relationTypes: [],
        relationCount: 0,
        direction: 'directed',
        category: 'general',
        strongestStrength: 0,
        averageStrength: 0,
        evidenceTitles: [],
      };
      bundles.push(bundle);
    }

    bundle.edges.push(edge);
    bundle.edgeIds.push(edge.id);
    if (!bundle.relationTypes.includes(edge.relationType)) {
      bundle.relationTypes.push(edge.relationType);
    }
    const edgeEvidenceTitles = edge.evidenceTitles?.length ? edge.evidenceTitles : edge.evidenceTitle ? [edge.evidenceTitle] : [];
    for (const title of edgeEvidenceTitles) {
      if (!bundle.evidenceTitles.includes(title)) {
        bundle.evidenceTitles.push(title);
      }
    }
  }

  for (const bundle of bundles) {
    const hasForward = bundle.edges.some((edge) => edge.source === bundle.source && edge.target === bundle.target);
    const hasReverse = bundle.edges.some((edge) => edge.source === bundle.target && edge.target === bundle.source);
    const hasDirected = bundle.edges.some((edge) => !isUndirectedRelationType(edge.relationType));
    const totalStrength = bundle.edges.reduce((sum, edge) => sum + edge.strength, 0);
    bundle.relationTypes.sort((a, b) => a.localeCompare(b));
    bundle.relationCount = bundle.edges.length;
    bundle.direction = !hasDirected ? 'undirected' : hasForward && hasReverse ? 'bidirectional' : 'directed';
    bundle.category = relationCategory(bundle.relationTypes);
    bundle.strongestStrength = Math.max(...bundle.edges.map((edge) => edge.strength), 0);
    bundle.averageStrength = bundle.edges.length > 0 ? totalStrength / bundle.edges.length : 0;
  }

  return bundles.sort((a, b) => {
    const countDelta = b.relationCount - a.relationCount;
    if (countDelta !== 0) return countDelta;
    const strengthDelta = b.strongestStrength - a.strongestStrength;
    if (strengthDelta !== 0) return strengthDelta;
    return a.id.localeCompare(b.id);
  });
}
