export interface SubagentToolDescriptor {
  name: string;
  label: string;
  description: string;
  enabledByDefault: boolean;
  source?: 'built_in' | 'delegation' | 'mcp';
  serverName?: string;
}

export const SUBAGENT_TOOL_CATALOG: SubagentToolDescriptor[] = [
  {
    name: 'search_knowledge_base',
    label: 'Knowledge Search',
    description: 'Search indexed local knowledge and find candidate evidence.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'manage_playbook',
    label: 'Manage Playbook',
    description: 'Create, update, list, and annotate reusable evidence collections.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'read_file',
    label: 'Read File',
    description: 'Open and inspect files that belong to registered sources.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'retrieve_evidence',
    label: 'Retrieve Evidence',
    description: 'Load exact evidence chunks and summarize or verify findings.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'list_sources',
    label: 'List Sources',
    description: 'Show available indexed sources.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'list_documents',
    label: 'List Documents',
    description: 'Browse indexed documents in the knowledge base.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'list_dir',
    label: 'List Directory',
    description: 'Inspect source directories and candidate file paths.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'get_chunk_context',
    label: 'Chunk Context',
    description: 'Expand truncated chunk snippets with nearby context.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'fetch_url',
    label: 'Fetch URL',
    description: 'Load a web page when the delegated task needs external context.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'write_note',
    label: 'Write Note',
    description: 'Write or append note files inside registered sources.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'search_playbooks',
    label: 'Search Playbooks',
    description: 'Inspect saved playbooks and reusable workflows.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'edit_file',
    label: 'Edit File',
    description: 'Apply text edits to files inside registered sources.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'submit_feedback',
    label: 'Submit Feedback',
    description: 'Record structured quality feedback on retrieved evidence.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'get_document_info',
    label: 'Document Info',
    description: 'Inspect document metadata and indexing details.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'reindex_document',
    label: 'Reindex Document',
    description: 'Trigger re-indexing for a file or source after content changes.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'compare_documents',
    label: 'Compare Documents',
    description: 'Compare passages, candidates, or drafts side by side.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'manage_source',
    label: 'Manage Source',
    description: 'Add or remove registered knowledge sources.',
    enabledByDefault: false,
    source: 'built_in',
  },
  {
    name: 'get_statistics',
    label: 'Statistics',
    description: 'Inspect corpus statistics and aggregate counts.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'search_by_date',
    label: 'Date Search',
    description: 'Find content constrained by date-related filters.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'summarize_document',
    label: 'Summarize Document',
    description: 'Generate document-level summaries for large files.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'update_plan',
    label: 'Update Plan',
    description: 'Record a structured plan artifact for the delegated task.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'record_verification',
    label: 'Record Verification',
    description: 'Record structured verification checks before returning.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'spawn_subagent',
    label: 'Spawn Subagent',
    description: 'Delegate a nested subtask to another short-lived worker.',
    enabledByDefault: false,
    source: 'delegation',
  },
  {
    name: 'spawn_subagent_batch',
    label: 'Spawn Batch',
    description: 'Launch several delegated workers in parallel for fan-out work.',
    enabledByDefault: false,
    source: 'delegation',
  },
  {
    name: 'judge_subagent_results',
    label: 'Judge Results',
    description: 'Adjudicate or rank delegated worker results with a rubric.',
    enabledByDefault: false,
    source: 'delegation',
  },
  {
    name: 'compile_document',
    label: 'Compile Document',
    description: 'Distill a document into a structured summary, entities, and relationships.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'query_knowledge_graph',
    label: 'Knowledge Graph',
    description: 'Query the entity-relationship graph for connected concepts and paths.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'run_health_check',
    label: 'Health Check',
    description: 'Detect stale documents, orphans, coverage gaps, and duplicates.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'archive_output',
    label: 'Archive Output',
    description: 'Archive an agent answer as a new document in the knowledge base.',
    enabledByDefault: true,
    source: 'built_in',
  },
  {
    name: 'get_related_concepts',
    label: 'Related Concepts',
    description: 'Find entities related to a topic with link strength and evidence.',
    enabledByDefault: true,
    source: 'built_in',
  },
];

export const DEFAULT_SUBAGENT_TOOL_NAMES = SUBAGENT_TOOL_CATALOG
  .filter(tool => tool.enabledByDefault)
  .map(tool => tool.name);

export function canonicalSubagentToolName(name: string): string {
  switch (name) {
    case 'compare':
      return 'compare_documents';
    case 'date_search':
      return 'search_by_date';
    default:
      return name;
  }
}

export function mergeSubagentToolCatalog(
  extraTools: SubagentToolDescriptor[] = [],
): SubagentToolDescriptor[] {
  const merged = new Map<string, SubagentToolDescriptor>();
  for (const tool of SUBAGENT_TOOL_CATALOG) {
    merged.set(tool.name, tool);
  }
  for (const tool of extraTools) {
    if (!merged.has(tool.name)) {
      merged.set(tool.name, tool);
    }
  }
  return Array.from(merged.values());
}

export function buildMcpSubagentToolDescriptors(
  tools: Array<{ name: string; description?: string | null; serverName?: string }>,
): SubagentToolDescriptor[] {
  return tools.map(tool => ({
    name: tool.name,
    label: tool.name,
    description: tool.description?.trim() || `MCP tool from ${tool.serverName ?? 'an enabled server'}.`,
    enabledByDefault: false,
    source: 'mcp',
    serverName: tool.serverName,
  }));
}

export function getSubagentToolDescriptor(name: string, catalog: SubagentToolDescriptor[] = SUBAGENT_TOOL_CATALOG) {
  return catalog.find(tool => tool.name === name) ?? null;
}

export function normalizeSubagentToolSelection(
  selection: string[] | null | undefined,
  catalog: SubagentToolDescriptor[] = SUBAGENT_TOOL_CATALOG,
): string[] {
  const selected = new Set((selection ?? DEFAULT_SUBAGENT_TOOL_NAMES).map(canonicalSubagentToolName));
  return catalog
    .filter(tool => selected.has(tool.name))
    .map(tool => tool.name);
}

export function usesDefaultSubagentToolSelection(selection: string[] | null | undefined): boolean {
  const normalized = (selection ?? DEFAULT_SUBAGENT_TOOL_NAMES).map(canonicalSubagentToolName);
  if (normalized.length !== DEFAULT_SUBAGENT_TOOL_NAMES.length) return false;
  return DEFAULT_SUBAGENT_TOOL_NAMES.every(name => normalized.includes(name));
}
