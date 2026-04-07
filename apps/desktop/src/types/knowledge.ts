// ── Knowledge Compile Types ─────────────────────────────────────────────

export type EntityType = "concept" | "person" | "technology" | "event" | "organization" | "place" | "other";

export interface DocumentSummary {
  id: string;
  documentId: number;
  summary: string;
  keyPoints: string[];
  tags: string[];
  modelUsed: string;
  compiledAt: string;
}

export interface Entity {
  id: string;
  name: string;
  entityType: EntityType;
  description: string;
  firstSeenDoc: number | null;
  mentionCount: number;
  createdAt: string;
}

export interface CompileResult {
  documentId: number;
  summary: DocumentSummary;
  entitiesFound: number;
  linksCreated: number;
}

export interface CompileStats {
  totalDocs: number;
  compiledDocs: number;
  totalEntities: number;
  totalLinks: number;
}

// ── Knowledge Graph Types ───────────────────────────────────────────────

export interface EntityLink {
  id: string;
  sourceEntityId: string;
  targetEntityId: string;
  relationType: string;
  strength: number;
  evidenceDocId: number | null;
}

export interface EntityNode {
  entity: Entity;
  links: EntityLink[];
  depth: number;
}

export interface KnowledgeMap {
  entities: Entity[];
  links: EntityLink[];
  totalEntities: number;
  totalLinks: number;
}

// ── Lint / Health Check Types ───────────────────────────────────────────

export type CheckType = "stale" | "orphan" | "gap" | "duplicate" | "contradiction";
export type Severity = "info" | "warning" | "critical";

export interface HealthIssue {
  id: string;
  checkType: CheckType;
  severity: Severity;
  targetDocId: number | null;
  targetEntityId: string | null;
  description: string;
  suggestion: string;
}

export interface HealthReport {
  staleDocuments: HealthIssue[];
  orphanDocuments: HealthIssue[];
  lowCoverageEntities: HealthIssue[];
  duplicateCandidates: HealthIssue[];
  totalIssues: number;
  checkedAt: string;
}

// ── Wiki Types ──────────────────────────────────────────────────────────

export interface EntityEntry {
  entity: Entity;
  documentCount: number;
  linkCount: number;
}

export interface WikiIndex {
  byType: Record<string, EntityEntry[]>;
  totalEntities: number;
  totalDocuments: number;
  compiledDocuments: number;
}

export interface DocumentRef {
  documentId: number;
  title: string;
  summary: string | null;
  relevance: number;
}

export interface MapOfContent {
  topic: string;
  relatedEntities: Entity[];
  documents: DocumentRef[];
  subTopics: string[];
}

export interface HotConcept {
  entity: Entity;
  score: number;
  recentQueries: number;
}

// ── Knowledge Loop Types ────────────────────────────────────────────────

export interface KnowledgeGap {
  topic: string;
  queryCount: number;
  avgConfidence: number;
  suggestion: string;
}

export interface QueryTrend {
  topic: string;
  count: number;
  firstQueried: string;
  lastQueried: string;
}

export interface ArchiveResult {
  documentId: number;
  source: string;
  title: string;
}
