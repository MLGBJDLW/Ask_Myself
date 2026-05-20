import { invoke } from "@tauri-apps/api/core";
import type {
  Source,
  ScanError,
  EvidenceCard,
  Playbook,
  PlaybookCitation,
  SearchResult,
  SearchFilters,
  IngestResult,
  IndexStats,
  QueryLog,
  Feedback,
  EmbedResult,
} from "../types";
import type { PrivacyConfig } from "../types/privacy";
import type { Project, CreateProjectInput, UpdateProjectInput } from "../types/project";
import type { EmbedderConfig } from "../types/embedder";
import type { OcrConfig } from "../types/ocr";
import type { VideoConfig, TranscriptChunk, VideoMetadata } from "../types/video";
import type {
  AgentConfig,
  AppConfig,
  SaveAgentConfigInput,
  Conversation,
  ConversationMessage,
  ConversationTurn,
  AgentTaskRun,
  AgentTaskRunListItem,
  AgentTaskRunEvent,
  AgentSubtaskRun,
  AgentExecutionGraph,
  AgentTaskArtifact,
  AgentTaskArtifactSummary,
  AgentTaskArtifactVersion,
  CreateAgentTaskArtifactInput,
  UpdateAgentTaskArtifactInput,
  PluginManifest,
  ToolAccessInfo,
  ConversationStats,
  ConversationSearchResult,
  ImageAttachment,
  Checkpoint,
  CheckpointBranch,
  FileCheckpoint,
  FileCheckpointRestore,
  UserMemory,
  AgentProceduralMemory,
  WebSearchConfig,
  WebSearchProviderStatus,
} from "../types/conversation";
import type {
  McpServer,
  SaveMcpServerInput,
  McpToolInfo,
  DiscoveredSkillBundle,
  Skill,
  SaveSkillInput,
  SkillChangeProposal,
  SkillProposalStatus,
  AppliedSkillChange,
} from "../types/extensions";
import type { TraceSummary, AgentTrace } from "../types/trace";
import type { QualityEvalReport } from "../types/qualityEval";
import type {
  BrowserEvidenceCapture,
  InvestigationGraph,
  LearningGovernanceSnapshot,
  SaveWorkflowAutomationInput,
  TaskResumeCheckpoint,
  TaskResumePrompt,
  WorkflowAutomation,
  WorkflowAutomationDueRun,
  WorkflowAutomationRun,
} from "../types/workflows";
import type { ProviderPreset } from "./providerPresets";

// ── Sources ─────────────────────────────────────────────────────────────

export const addSource = (
  rootPath: string,
  includeGlobs: string[],
  excludeGlobs: string[],
) => invoke<Source>("add_source", { kind: "local_folder", rootPath, includeGlobs, excludeGlobs });

export const listSources = () => invoke<Source[]>("list_sources");

export const deleteSource = (sourceId: string) =>
  invoke<void>("delete_source", { sourceId });

export const scanSource = (sourceId: string) =>
  invoke<IngestResult>("scan_source", { sourceId });

export const scanAllSources = () =>
  invoke<IngestResult[]>("scan_all_sources");

export const getScanErrors = (sourceId: string) =>
  invoke<ScanError[]>('get_scan_errors_cmd', { sourceId });

export const clearScanErrors = (sourceId: string) =>
  invoke<void>('clear_scan_errors_cmd', { sourceId });

export const clearScanError = (sourceId: string, path: string) =>
  invoke<void>('clear_scan_error_cmd', { sourceId, path });

// ── Search ──────────────────────────────────────────────────────────────

export const search = (queryText: string, limit?: number, offset?: number, filters?: SearchFilters) =>
  invoke<SearchResult>("search", { queryText, limit, offset, filters });

export const getEvidenceCard = (chunkId: string) =>
  invoke<EvidenceCard>("get_evidence_card", { chunkId });

export const getEvidenceCards = (chunkIds: string[]) =>
  invoke<EvidenceCard[]>('get_evidence_cards', { chunkIds });

// ── Index ───────────────────────────────────────────────────────────────

export const getIndexStats = () => invoke<IndexStats>("get_index_stats");

export const rebuildIndex = () => invoke<void>("rebuild_index");

// ── Playbooks ───────────────────────────────────────────────────────────

export const createPlaybook = (
  title: string,
  description: string,
  queryText: string,
) => invoke<Playbook>("create_playbook", { title, description, queryText });

export const listPlaybooks = () => invoke<Playbook[]>("list_playbooks");

export const getPlaybook = (playbookId: string) =>
  invoke<Playbook>("get_playbook", { playbookId });

export const updatePlaybook = (
  playbookId: string,
  title: string,
  description: string,
) => invoke<Playbook>("update_playbook", { playbookId, title, description });

export const deletePlaybook = (playbookId: string) =>
  invoke<void>("delete_playbook", { playbookId });

export const addCitation = (
  playbookId: string,
  chunkId: string,
  note: string,
  sortOrder: number,
) => invoke<PlaybookCitation>("add_citation", { playbookId, chunkId, note, sortOrder });

export const listCitations = (playbookId: string) =>
  invoke<PlaybookCitation[]>("list_citations", { playbookId });

export const removeCitation = (citationId: string) =>
  invoke<void>("remove_citation", { citationId });

// ── Query Log ───────────────────────────────────────────────────────────

export const getRecentQueries = (limit?: number) =>
  invoke<QueryLog[]>("get_recent_queries", { limit });

export const clearRecentQueries = () =>
  invoke<void>("clear_recent_queries");

// ── Hybrid Search ───────────────────────────────────────────────────────

export const hybridSearch = (queryText: string, limit?: number, offset?: number, filters?: SearchFilters) =>
  invoke<SearchResult>('hybrid_search', { queryText, filters, limit, offset });

// ── Embeddings ──────────────────────────────────────────────────────────

export const embedSource = (sourceId: string) =>
  invoke<EmbedResult>('embed_source', { sourceId });

export const rebuildEmbeddings = () =>
  invoke<EmbedResult>('rebuild_embeddings');

// ── Feedback ────────────────────────────────────────────────────────────

export const addFeedback = (chunkId: string, queryText: string, action: string) =>
  invoke<Feedback>('add_feedback', { chunkId, queryText, action });

export const getFeedbackForQuery = (queryText: string) =>
  invoke<Feedback[]>('get_feedback_for_query', { queryText });

export const deleteFeedback = (feedbackId: string) =>
  invoke<void>('delete_feedback', { feedbackId });

export interface MessageFeedback {
  id: string;
  messageId: string;
  conversationId: string;
  rating: number;
  note: string | null;
  createdAt: string;
}

/** Save message-level thumbs up/down. rating: +1 upvote, -1 downvote, 0 clear. */
export const setMessageFeedback = (
  messageId: string,
  conversationId: string,
  rating: number,
  note?: string,
) =>
  invoke<MessageFeedback>('set_message_feedback_cmd', {
    messageId,
    conversationId,
    rating,
    note: note ?? null,
  });

export const getMessageFeedback = (messageId: string) =>
  invoke<MessageFeedback | null>('get_message_feedback_cmd', { messageId });

// ── Sources (extra) ─────────────────────────────────────────────────────

export const getSource = (sourceId: string) =>
  invoke<Source>('get_source', { sourceId });

export interface SourceTreeNode {
  name: string;
  path: string;
  relativePath: string;
  kind: 'directory' | 'file';
  extension?: string | null;
  sizeBytes?: number | null;
  modifiedAt?: string | null;
  indexed: boolean;
  documentId?: string | null;
  chunkCount?: number | null;
  children?: SourceTreeNode[] | null;
  childrenTruncated: boolean;
}

export interface SourceTree {
  sourceId: string;
  rootPath: string;
  relativePath: string;
  nodes: SourceTreeNode[];
  totalEntries: number;
  truncated: boolean;
}

export const listSourceTree = (
  sourceId: string,
  relativePath?: string,
  depth?: number,
  limit?: number,
) => invoke<SourceTree>('list_source_tree_cmd', {
  sourceId,
  relativePath: relativePath ?? null,
  depth: depth ?? null,
  limit: limit ?? null,
});

export const updateSource = (
  sourceId: string,
  includeGlobs: string[],
  excludeGlobs: string[],
  watchEnabled: boolean,
  rootPath?: string | null,
) => invoke<Source>('update_source', {
  sourceId,
  rootPath: rootPath ?? null,
  includeGlobs,
  excludeGlobs,
  watchEnabled,
});

// ── Privacy ─────────────────────────────────────────────────────────────

export const getPrivacyConfig = () =>
  invoke<PrivacyConfig>('get_privacy_config');

export const savePrivacyConfig = (config: PrivacyConfig) =>
  invoke<void>('save_privacy_config', { config });

// ── Embedder Config ──────────────────────────────────────────────────

export const getEmbedderConfig = () =>
  invoke<EmbedderConfig>('get_embedder_config_cmd');

export const saveEmbedderConfig = (config: EmbedderConfig) =>
  invoke<void>('save_embedder_config_cmd', { config });

export const testApiConnection = (apiKey: string, baseUrl: string) =>
  invoke<boolean>('test_api_connection_cmd', { apiKey, baseUrl });

export const checkLocalModel = (localModel?: string) =>
  invoke<boolean>('check_local_model_cmd', { localModel });

export const downloadLocalModel = (localModel?: string) =>
  invoke<void>('download_local_model_cmd', { localModel });

export const cancelModelDownload = () =>
  invoke<void>('cancel_model_download_cmd');

export const deleteLocalModel = (localModel?: string) =>
  invoke<void>('delete_local_model_cmd', { localModel });

// ── File ────────────────────────────────────────────────────────────────

export const openFileInDefaultApp = (path: string) =>
  invoke<void>('open_file_in_default_app', { path });

export const showInFileExplorer = (path: string) =>
  invoke<void>('show_in_file_explorer', { path });

export interface FilePreview {
  path: string;
  displayName: string;
  sourceId: string;
  sourceName: string;
  extension: string;
  mimeType: string;
  kind: 'markdown' | 'text' | 'code' | 'document' | 'binary' | string;
  language?: string | null;
  content?: string | null;
  encoding?: string | null;
  editable: boolean;
  sizeBytes: number;
  modifiedAt?: string | null;
  hash: string;
  lineCount: number;
  truncated: boolean;
  warning?: string | null;
  structuredPreview?: StructuredPreview | null;
  renderedPreview?: RenderedPreview | null;
  capabilities?: PreviewCapabilities | null;
}

export interface PreviewCapabilities {
  canRenderStructured: boolean;
  canExtractText: boolean;
  needsExternalRuntime: boolean;
  structuredUnavailableReason?: string | null;
}

export type StructuredPreview = DocumentStructuredPreview | WorkbookStructuredPreview;

export interface DocumentStructuredPreview {
  type: 'document';
  blocks: DocumentPreviewBlock[];
  assets: PreviewAsset[];
}

export interface WorkbookStructuredPreview {
  type: 'workbook';
  sheets: WorkbookPreviewSheet[];
  limits: WorkbookPreviewLimits;
  truncated: boolean;
}

export type DocumentPreviewBlock =
  | DocumentHeadingBlock
  | DocumentParagraphBlock
  | DocumentListBlock
  | DocumentTableBlock
  | DocumentImageBlock
  | DocumentPageBreakBlock
  | DocumentUnsupportedBlock;

export interface DocumentHeadingBlock {
  type: 'heading';
  level: number;
  runs: DocumentPreviewRun[];
  alignment?: string | null;
}

export interface DocumentParagraphBlock {
  type: 'paragraph';
  runs: DocumentPreviewRun[];
  alignment?: string | null;
}

export interface DocumentListBlock {
  type: 'list';
  ordered: boolean;
  level: number;
  items: DocumentPreviewListItem[];
}

export interface DocumentTableBlock {
  type: 'table';
  rows: DocumentPreviewTableRow[];
}

export interface DocumentImageBlock {
  type: 'image';
  assetId: string;
  alt?: string | null;
}

export interface DocumentPageBreakBlock {
  type: 'pageBreak';
}

export interface DocumentUnsupportedBlock {
  type: 'unsupported';
  message: string;
}

export interface DocumentPreviewRun {
  text: string;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  color?: string | null;
  backgroundColor?: string | null;
  fontSize?: string | null;
  hyperlink?: string | null;
}

export interface DocumentPreviewListItem {
  runs: DocumentPreviewRun[];
}

export interface DocumentPreviewTableRow {
  cells: DocumentPreviewTableCell[];
}

export interface DocumentPreviewTableCell {
  blocks: DocumentPreviewBlock[];
}

export interface PreviewAsset {
  id: string;
  kind: string;
  mimeType: string;
  path: string;
  width?: number | null;
  height?: number | null;
}

export interface WorkbookPreviewLimits {
  maxSheets: number;
  maxRows: number;
  maxColumns: number;
}

export interface WorkbookPreviewSheet {
  name: string;
  index: number;
  rowCount: number;
  columnCount: number;
  previewRowCount: number;
  previewColumnCount: number;
  cells: WorkbookPreviewCell[];
  mergedRanges: WorkbookPreviewMergedRange[];
  truncated: boolean;
}

export interface WorkbookPreviewCell {
  row: number;
  column: number;
  value: string;
  dataType: string;
  formula?: string | null;
}

export interface WorkbookPreviewMergedRange {
  startRow: number;
  startColumn: number;
  endRow: number;
  endColumn: number;
}

export interface RenderedPreviewPage {
  page: number;
  path: string;
}

export interface RenderedPreview {
  kind: 'office-pages' | string;
  dpi: number;
  pageCount: number;
  truncated: boolean;
  pages: RenderedPreviewPage[];
}

export interface FileSaveResult {
  preview: FilePreview;
  checkpointId: string;
  bytesWritten: number;
  reindexStatus: string;
  reindexDetail?: string | null;
}

export interface WorkflowCatalogTask {
  id: string;
  roleId: string;
  roleLabel: string;
  task: string;
  expectedOutput: string;
  deliverableStyle: string;
  acceptanceCriteria: string[];
}

export interface WorkflowCatalogTemplate {
  id: string;
  label: string;
  description: string;
  maxParallel: number;
  promptTemplate: string;
  tasks: WorkflowCatalogTask[];
}

export const previewFile = (path: string) =>
  invoke<FilePreview>('preview_file_cmd', {
    path,
  });

export const saveTextFile = (
  path: string,
  content: string,
  expectedHash?: string | null,
) =>
  invoke<FileSaveResult>('save_text_file_cmd', {
    input: {
      path,
      content,
      expectedHash: expectedHash ?? null,
    },
  });

export interface SaveGeneratedImageInput {
  outputPath: string;
  sourcePath?: string | null;
  dataUrl?: string | null;
  mediaType?: string | null;
}

export interface SaveGeneratedImageResult {
  path: string;
  bytesWritten: number;
}

export const readGeneratedImageDataUrl = (path: string, mediaType?: string | null) =>
  invoke<string>('read_generated_image_data_url_cmd', {
    path,
    mediaType: mediaType ?? null,
  });

export const saveGeneratedImage = (input: SaveGeneratedImageInput) =>
  invoke<SaveGeneratedImageResult>('save_generated_image_cmd', { input });

// ── Index (extra) ───────────────────────────────────────────────────────

export const optimizeFtsIndex = () =>
  invoke<void>('optimize_fts_index');

// ── Citations (extra) ───────────────────────────────────────────────────

export const updateCitationNote = (citationId: string, note: string) =>
  invoke<void>('update_citation_note', { citationId, note });

export const reorderCitations = (playbookId: string, citationIds: string[]) =>
  invoke<void>('reorder_citations', { playbookId, citationIds });

// ── Watcher ─────────────────────────────────────────────────────────────

export interface WatchedSourceInfo {
  sourceId: string;
  rootPath: string;
}

export const startWatching = (sourceId: string) =>
  invoke<void>('start_watching', { sourceId });

export const stopWatching = (sourceId: string) =>
  invoke<void>('stop_watching', { sourceId });

export const getWatcherStatus = () =>
  invoke<WatchedSourceInfo[]>('get_watcher_status');

// ── Personas ─────────────────────────────────────────────────────────────

export interface PersonaProfile {
  id: string;
  name: string;
  description: string;
  instructions: string;
  enabled: boolean;
  builtin?: boolean;
  defaultSkillIds?: string[];
  createdAt?: string;
  updatedAt?: string;
}

export interface SavePersonaInput {
  id?: string | null;
  name: string;
  description: string;
  instructions: string;
  enabled: boolean;
  defaultSkillIds: string[];
}

export const listPersonas = () =>
  invoke<PersonaProfile[]>('list_personas_cmd');

export const savePersona = (input: SavePersonaInput) =>
  invoke<PersonaProfile>('save_persona_cmd', { input });

export const deletePersona = (id: string) =>
  invoke<void>('delete_persona_cmd', { id });

export const togglePersona = (id: string, enabled: boolean) =>
  invoke<void>('toggle_persona_cmd', { id, enabled });

// ── Agent Config ────────────────────────────────────────────────────────

export const listAgentConfigs = () => invoke<AgentConfig[]>('list_agent_configs_cmd');

export const saveAgentConfig = (config: SaveAgentConfigInput) =>
  invoke<AgentConfig>('save_agent_config_cmd', { config });

export const deleteAgentConfig = (id: string) =>
  invoke<void>('delete_agent_config_cmd', { id });

export const setDefaultAgentConfig = (id: string) =>
  invoke<void>('set_default_agent_config_cmd', { id });

export const testAgentConnection = (config: SaveAgentConfigInput) =>
  invoke<string[]>('test_agent_connection_cmd', { config });

export const listProviderPresets = () =>
  invoke<ProviderPreset[]>('list_provider_presets_cmd');

export const listWorkflowTemplates = () =>
  invoke<WorkflowCatalogTemplate[]>('list_workflow_templates_cmd');

export const saveWorkflowAutomation = (input: SaveWorkflowAutomationInput) =>
  invoke<WorkflowAutomation>('save_workflow_automation_cmd', { input });

export const listWorkflowAutomations = () =>
  invoke<WorkflowAutomation[]>('list_workflow_automations_cmd');

export const deleteWorkflowAutomation = (id: string) =>
  invoke<void>('delete_workflow_automation_cmd', { id });

export const setWorkflowAutomationEnabled = (id: string, enabled: boolean) =>
  invoke<WorkflowAutomation>('set_workflow_automation_enabled_cmd', { id, enabled });

export const listDueWorkflowAutomations = (now?: string | null) =>
  invoke<WorkflowAutomationDueRun[]>('list_due_workflow_automations_cmd', { now: now ?? null });

export const previewWorkflowAutomationPrompt = (id: string) =>
  invoke<string>('preview_workflow_automation_prompt_cmd', { id });

export const recordWorkflowAutomationRun = (
  automationId: string,
  status: string,
  taskRunId?: string | null,
  summary?: string | null,
) => invoke<WorkflowAutomationRun>('record_workflow_automation_run_cmd', {
  automationId,
  taskRunId: taskRunId ?? null,
  status,
  summary: summary ?? null,
});

// ── Conversations ───────────────────────────────────────────────────────

export const createConversation = (
  provider: string,
  model: string,
  systemPrompt?: string,
  projectId?: string,
  personaId?: string | null,
) =>
  invoke<Conversation>('create_conversation_cmd', {
    provider,
    model,
    systemPrompt,
    projectId,
    personaId: personaId ?? null,
  });

export const createConversationWithContext = (
  provider: string,
  model: string,
  systemPrompt?: string,
  collectionContext?: Conversation['collectionContext'],
  projectId?: string,
  personaId?: string | null,
) => invoke<Conversation>('create_conversation_cmd', {
  provider,
  model,
  systemPrompt,
  collectionContext,
  projectId,
  personaId: personaId ?? null,
});

export const listConversations = () => invoke<Conversation[]>('list_conversations_cmd');

export const getConversation = (id: string) =>
  invoke<[Conversation, ConversationMessage[]]>('get_conversation_cmd', { id });

export const getConversationTurns = (conversationId: string) =>
  invoke<ConversationTurn[]>('get_conversation_turns_cmd', { conversationId });

export const getAgentTaskRuns = (conversationId: string) =>
  invoke<AgentTaskRun[]>('get_agent_task_runs_cmd', { conversationId });

export const listRecentAgentTaskRuns = (limit = 50) =>
  invoke<AgentTaskRunListItem[]>('list_recent_agent_task_runs_cmd', { limit });

export const getAgentTaskRunEvents = (runId: string) =>
  invoke<AgentTaskRunEvent[]>('get_agent_task_run_events_cmd', { runId });

export const getAgentSubtaskRuns = (runId: string) =>
  invoke<AgentSubtaskRun[]>('get_agent_subtask_runs_cmd', { runId });

export const getAgentExecutionGraph = (runId: string) =>
  invoke<AgentExecutionGraph>('get_agent_execution_graph_cmd', { runId });

export const getAgentTaskArtifacts = (runId: string) =>
  invoke<AgentTaskArtifactSummary[]>('get_agent_task_artifacts_cmd', { runId });

export const listPersistedAgentTaskArtifacts = (runId: string) =>
  invoke<AgentTaskArtifact[]>('list_persisted_agent_task_artifacts_cmd', { runId });

export const createAgentTaskArtifact = (runId: string, input: CreateAgentTaskArtifactInput) =>
  invoke<AgentTaskArtifact>('create_agent_task_artifact_cmd', { runId, input });

export const updateAgentTaskArtifact = (artifactId: string, input: UpdateAgentTaskArtifactInput) =>
  invoke<AgentTaskArtifact>('update_agent_task_artifact_cmd', { artifactId, input });

export const listAgentTaskArtifactVersions = (artifactId: string) =>
  invoke<AgentTaskArtifactVersion[]>('list_agent_task_artifact_versions_cmd', { artifactId });

export const pauseAgentTaskRun = (runId: string) =>
  invoke<TaskResumeCheckpoint>('pause_agent_task_run_cmd', { runId });

export const listTaskResumeCheckpoints = (runId: string) =>
  invoke<TaskResumeCheckpoint[]>('list_task_resume_checkpoints_cmd', { runId });

export const getTaskResumePrompt = (runId: string) =>
  invoke<TaskResumePrompt>('get_task_resume_prompt_cmd', { runId });

export const getInvestigationGraph = (runId: string) =>
  invoke<InvestigationGraph>('get_investigation_graph_cmd', { runId });

export const listToolAccessMap = () =>
  invoke<ToolAccessInfo[]>('list_tool_access_map_cmd');

export const getLearningGovernanceSnapshot = () =>
  invoke<LearningGovernanceSnapshot>('get_learning_governance_snapshot_cmd');

export const captureBrowserEvidence = (
  url: string,
  maxLength?: number | null,
  mode?: string | null,
) => invoke<BrowserEvidenceCapture>('capture_browser_evidence_cmd', {
  url,
  maxLength: maxLength ?? null,
  mode: mode ?? null,
});

export const listBuiltinPlugins = (options?: { includeRuntimeChecks?: boolean }) =>
  invoke<PluginManifest[]>('list_builtin_plugins_cmd', {
    includeRuntimeChecks: options?.includeRuntimeChecks ?? false,
  });

export const listProjectTools = (sourceScope?: string[] | null) =>
  invoke<import('../types/project-tool').ProjectToolCatalog>('list_project_tools_cmd', {
    sourceScope: sourceScope ?? null,
  });

export const deleteConversation = (id: string) =>
  invoke<void>('delete_conversation_cmd', { id });

export const deleteConversationsBatch = (ids: string[]) =>
  invoke<number>('delete_conversations_batch_cmd', { ids });

export const deleteAllConversations = () =>
  invoke<number>('delete_all_conversations_cmd');

export const renameConversation = (id: string, title: string) =>
  invoke<void>('rename_conversation_cmd', { id, title });

export const generateTitle = (conversationId: string) =>
  invoke<string>('generate_title_cmd', { conversationId });

export const updateConversationSystemPrompt = (id: string, systemPrompt: string) =>
  invoke<void>('update_conversation_system_prompt_cmd', { id, systemPrompt });

export const updateConversationCollectionContext = (
  id: string,
  collectionContext: Conversation['collectionContext'],
) => invoke<void>('update_conversation_collection_context_cmd', { id, collectionContext });

export const updateConversationPersona = (id: string, personaId?: string | null) =>
  invoke<Conversation>('update_conversation_persona_cmd', { id, personaId: personaId ?? null });

export const updateConversationModel = (id: string, provider: string, model: string) =>
  invoke<Conversation>('update_conversation_model_cmd', { id, provider, model });

// ── Projects ────────────────────────────────────────────────────────────

export const createProject = (input: CreateProjectInput) =>
  invoke<Project>('create_project_cmd', { input });

export const listProjects = () => invoke<Project[]>('list_projects_cmd');

export const getProject = (id: string) =>
  invoke<Project>('get_project_cmd', { id });

export const updateProject = (id: string, input: UpdateProjectInput) =>
  invoke<Project>('update_project_cmd', { id, input });

export const deleteProject = (id: string) =>
  invoke<void>('delete_project_cmd', { id });

export interface ProjectMemory {
  id: string;
  projectId: string;
  kind: string;
  title: string;
  content: string;
  source: string;
  pinned: boolean;
  archived: boolean;
  confidence?: number;
  expiresAt?: string | null;
  conflictStatus?: string;
  createdAt: string;
  updatedAt: string;
}

export interface CreateProjectMemoryInput {
  kind?: string | null;
  title?: string | null;
  content: string;
  pinned?: boolean | null;
  source?: string | null;
  confidence?: number | null;
  expiresAt?: string | null;
  conflictStatus?: string | null;
}

export interface UpdateProjectMemoryInput {
  kind?: string | null;
  title?: string | null;
  content?: string | null;
  pinned?: boolean | null;
  archived?: boolean | null;
  confidence?: number | null;
  expiresAt?: string | null;
  conflictStatus?: string | null;
}

export const listProjectMemories = (projectId: string) =>
  invoke<ProjectMemory[]>('list_project_memories_cmd', { projectId });

export const createProjectMemory = (projectId: string, input: CreateProjectMemoryInput) =>
  invoke<ProjectMemory>('create_project_memory_cmd', { projectId, input });

export const updateProjectMemory = (id: string, input: UpdateProjectMemoryInput) =>
  invoke<ProjectMemory>('update_project_memory_cmd', { id, input });

export const deleteProjectMemory = (id: string) =>
  invoke<void>('delete_project_memory_cmd', { id });

export const moveConversationToProject = (conversationId: string, projectId: string) =>
  invoke<void>('move_conversation_to_project_cmd', { conversationId, projectId });

export const removeConversationFromProject = (conversationId: string) =>
  invoke<void>('remove_conversation_from_project_cmd', { conversationId });

// ── Agent Chat ──────────────────────────────────────────────────────────

export const agentChat = (
  conversationId: string,
  message: string,
  attachments?: ImageAttachment[],
  agentConfigId?: string | null,
  personaId?: string | null,
) =>
  invoke<void>('agent_chat_cmd', {
    conversationId,
    message,
    attachments: attachments ?? null,
    agentConfigId: agentConfigId ?? null,
    personaId: personaId ?? null,
  });

export const agentSteer = (conversationId: string, message: string) =>
  invoke<void>('agent_steer_cmd', { conversationId, message });

export const agentStop = (conversationId: string) =>
  invoke<void>('agent_stop_cmd', { conversationId });

export const getModelContextWindow = (model: string) =>
  invoke<number>('get_model_context_window', { model });

// ── Image Attachment ────────────────────────────────────────────────────

export const prepareImageAttachment = (path: string) =>
  invoke<ImageAttachment>('prepare_image_attachment', { path });

// ── Conversation Sources ────────────────────────────────────────────────

export const setConversationSources = (conversationId: string, sourceIds: string[]) =>
  invoke<void>('set_conversation_sources_cmd', { conversationId, sourceIds });

export const getConversationSources = (conversationId: string) =>
  invoke<string[]>('get_conversation_sources_cmd', { conversationId });

// ── Conversation Maintenance ────────────────────────────────────────

export const getConversationStats = () =>
  invoke<ConversationStats>('get_conversation_stats_cmd');

export const cleanupEmptyConversations = (daysOld: number) =>
  invoke<number>('cleanup_empty_conversations_cmd', { daysOld });

export const compactConversation = (conversationId: string) =>
  invoke<void>('compact_conversation_cmd', { conversationId });

export const searchConversations = (query: string, limit?: number) =>
  invoke<ConversationSearchResult[]>('search_conversations_cmd', { query, limit });

// ── Checkpoints ─────────────────────────────────────────────────────

export const listCheckpoints = (conversationId: string) =>
  invoke<Checkpoint[]>('list_checkpoints_cmd', { conversationId });

export const restoreCheckpoint = (checkpointId: string) =>
  invoke<ConversationMessage[]>('restore_checkpoint_cmd', { checkpointId });

export const branchCheckpoint = (checkpointId: string) =>
  invoke<CheckpointBranch>('branch_checkpoint_cmd', { checkpointId });

export const deleteCheckpoint = (checkpointId: string) =>
  invoke<void>('delete_checkpoint_cmd', { checkpointId });

export const listFileCheckpoints = (conversationId?: string | null) =>
  invoke<FileCheckpoint[]>('list_file_checkpoints_cmd', { conversationId: conversationId ?? null });

export const restoreFileCheckpoint = (checkpointId: string) =>
  invoke<FileCheckpointRestore>('restore_file_checkpoint_cmd', { checkpointId });

export const deleteFileCheckpoint = (checkpointId: string) =>
  invoke<void>('delete_file_checkpoint_cmd', { checkpointId });

// ── User Memory ────────────────────────────────────────────────────────

export const listUserMemories = () =>
  invoke<UserMemory[]>('list_user_memories_cmd');

export const createUserMemory = (content: string) =>
  invoke<UserMemory>('create_user_memory_cmd', { content });

export const updateUserMemory = (id: string, content: string) =>
  invoke<UserMemory>('update_user_memory_cmd', { id, content });

export const deleteUserMemory = (id: string) =>
  invoke<void>('delete_user_memory_cmd', { id });

export const listAgentProceduralMemories = (limit = 20) =>
  invoke<AgentProceduralMemory[]>('list_agent_procedural_memories_cmd', { limit });

export const deleteAgentProceduralMemory = (id: string) =>
  invoke<void>('delete_agent_procedural_memory_cmd', { id });

// ── OCR ─────────────────────────────────────────────────────────────

export const getOcrConfig = () =>
  invoke<OcrConfig>('get_ocr_config_cmd');

export const saveOcrConfig = (config: OcrConfig) =>
  invoke<void>('save_ocr_config_cmd', { config });

export const checkOcrModels = (config: OcrConfig) =>
  invoke<boolean>('check_ocr_models_cmd', { config });

export const downloadOcrModels = (config: OcrConfig) =>
  invoke<void>('download_ocr_models_cmd', { config });

// ── App Config ──────────────────────────────────────────────────────

export const getAppConfig = () =>
  invoke<AppConfig>('get_app_config_cmd');

export const saveAppConfig = (config: AppConfig) =>
  invoke<void>('save_app_config_cmd', { config });

export const getWebSearchStatus = (webSearch?: WebSearchConfig) =>
  invoke<WebSearchProviderStatus[]>('get_web_search_status_cmd', { webSearch });

export type OfficeRuntimeStatus = 'ready' | 'degraded' | 'missing' | 'blocked';

export interface OfficeDependencyStatus {
  id: string;
  label: string;
  kind: string;
  required: boolean;
  status: string;
  version?: string | null;
  path?: string | null;
  detail?: string | null;
  installHint?: string | null;
}

export interface OfficeRuntimeReadiness {
  status: OfficeRuntimeStatus;
  summary: string;
  pythonPath?: string | null;
  appManagedPythonPath?: string | null;
  appManagedEnvPath: string;
  skillScriptPath: string;
  requirementsPath: string;
  canPrepare: boolean;
  canInstallPythonPackages: boolean;
  needsPythonInstall: boolean;
  pythonDownloadUrl: string;
  dependencies: OfficeDependencyStatus[];
}

export interface OfficePrepareAction {
  name: string;
  status: string;
  detail?: string | null;
}

export interface OfficePrepareResult {
  success: boolean;
  actions: OfficePrepareAction[];
  readiness: OfficeRuntimeReadiness;
}

export const checkOfficeRuntime = () =>
  invoke<OfficeRuntimeReadiness>('check_office_runtime_cmd');

export const prepareOfficeRuntime = () =>
  invoke<OfficePrepareResult>('prepare_office_runtime_cmd');

// ── Setup Wizard ────────────────────────────────────────────────────

export const getWizardState = () =>
  invoke<import('../types/wizard').WizardState>('get_wizard_state_cmd');

export const setWizardCompleted = () =>
  invoke<void>('set_wizard_completed_cmd');

export const resetWizard = () =>
  invoke<void>('reset_wizard_cmd');

// ── Video ───────────────────────────────────────────────────────────

export const getVideoConfig = () =>
  invoke<VideoConfig>('get_video_config_cmd');

export const saveVideoConfig = (config: VideoConfig) =>
  invoke<void>('save_video_config_cmd', { config });

export const checkWhisperModel = (config: VideoConfig) =>
  invoke<boolean>('check_whisper_model_cmd', { config });

export const downloadWhisperModel = (config: VideoConfig) =>
  invoke<void>('download_whisper_model_cmd', { config });

export const deleteWhisperModel = () =>
  invoke<void>('delete_whisper_model_cmd');

export const checkFfmpeg = (config: VideoConfig) =>
  invoke<boolean>('check_ffmpeg_cmd', { config });

export const downloadFfmpeg = () =>
  invoke<string>('download_ffmpeg_cmd');

export const transcribeAudioBuffer = (audioData: number[]) =>
  invoke<string>('transcribe_audio_buffer_cmd', { audioData });

export const clearAnswerCache = () =>
  invoke<number>('clear_answer_cache');

// ── Skills ──────────────────────────────────────────────────────────────

export const listSkills = () =>
  invoke<Skill[]>('list_skills_cmd');

export const saveSkill = (input: SaveSkillInput) =>
  invoke<Skill>('save_skill_cmd', { input });

export const deleteSkill = (id: string) =>
  invoke<void>('delete_skill_cmd', { id });

export const toggleSkill = (id: string, enabled: boolean) =>
  invoke<void>('toggle_skill_cmd', { id, enabled });

export const listBuiltinSkills = () =>
  invoke<Skill[]>('list_builtin_skills_cmd');

const isLegacyBuiltinSkillRow = (skill: Skill) =>
  skill.builtin === true || skill.id.startsWith('builtin-');

const normalizeSkillList = (skills: Skill[] | null | undefined): Skill[] =>
  Array.isArray(skills) ? skills : [];

export const listAllSkills = async () => {
  const [builtinResult, userResult] = await Promise.all([
    listBuiltinSkills(),
    listSkills(),
  ]);
  const builtins = normalizeSkillList(builtinResult);
  const userSkills = normalizeSkillList(userResult);
  const builtinIds = new Set(builtins.map((skill) => skill.id));
  const filteredUserSkills = userSkills.filter(
    (skill) => !isLegacyBuiltinSkillRow(skill) && !builtinIds.has(skill.id),
  );
  return [...builtins, ...filteredUserSkills];
};

export const listActiveSkills = async () =>
  (await listAllSkills()).filter((skill) => skill.enabled);

export const listSelectedSkills = (query: string, personaId?: string | null) =>
  invoke<Skill[]>('list_selected_skills_cmd', {
    query,
    personaId: personaId ?? null,
  });

export const importSkillFromMd = (content: string) =>
  invoke<Skill>('import_skill_from_md_cmd', { content });

export const exportSkillToMd = (skillId: string) =>
  invoke<string>('export_skill_to_md_cmd', { skillId });

export const scanSkillContent = (content: string) =>
  invoke<import('../types/extensions').SkillWarning[]>('scan_skill_content_cmd', { content });

export const listSkillChangeProposals = (
  status: SkillProposalStatus | null = 'pending',
  limit = 20,
) =>
  invoke<SkillChangeProposal[]>('list_skill_change_proposals_cmd', { status, limit });

export const applySkillChangeProposal = (id: string) =>
  invoke<AppliedSkillChange>('apply_skill_change_proposal_cmd', { id });

export const rejectSkillChangeProposal = (id: string) =>
  invoke<SkillChangeProposal>('reject_skill_change_proposal_cmd', { id });

export const discoverSkillsInDirectory = (directory: string) =>
  invoke<DiscoveredSkillBundle[]>('discover_skills_in_directory_cmd', { directory });

export const importSkillsFromDirectory = (directory: string) =>
  invoke<Skill[]>('import_skills_from_directory_cmd', { directory });

// ── MCP Servers ─────────────────────────────────────────────────────────

export const listMcpServers = () =>
  invoke<McpServer[]>('list_mcp_servers_cmd');

export const saveMcpServer = (input: SaveMcpServerInput) =>
  invoke<McpServer>('save_mcp_server_cmd', { input });

export const deleteMcpServer = (id: string) =>
  invoke<void>('delete_mcp_server_cmd', { id });

export const toggleMcpServer = (id: string, enabled: boolean) =>
  invoke<void>('toggle_mcp_server_cmd', { id, enabled });

export const testMcpServer = (id: string) =>
  invoke<McpToolInfo[]>('test_mcp_server_cmd', { id });

export const testMcpServerDirect = (input: {
  name: string;
  transport: string;
  command?: string | null;
  args?: string | null;
  url?: string | null;
  envJson?: string | null;
  headersJson?: string | null;
}) =>
  invoke<McpToolInfo[]>('test_mcp_server_direct_cmd', input);

export const listMcpTools = (serverId: string) =>
  invoke<McpToolInfo[]>('list_mcp_tools_cmd', { serverId });

// ── Video Analysis ──────────────────────────────────────────────────

export const analyzeVideo = (path: string) =>
  invoke<{
    transcript: string;
    segmentCount: number;
    durationSecs: number | null;
    frameTextsCount: number;
    thumbnailPath: string | null;
    metadata: VideoMetadata | null;
  }>('analyze_video_cmd', { path });

export const getVideoTranscript = (filePath: string) =>
  invoke<TranscriptChunk[]>('get_video_transcript_cmd', { filePath });

export const getVideoMetadata = (filePath: string) =>
  invoke<VideoMetadata>('get_video_metadata_cmd', { filePath });

// ── Trace Analytics ─────────────────────────────────────────────────

export const getTraceSummary = () =>
  invoke<TraceSummary>('get_trace_summary');

export const getRecentTraces = (limit?: number) =>
  invoke<AgentTrace[]>('get_recent_traces', { limit });

export const runAgentQualityEval = () =>
  invoke<QualityEvalReport>('run_agent_quality_eval_cmd');

// ── Knowledge Compilation ───────────────────────────────────────────

import type {
  CompileResult,
  CompileStats,
  KnowledgeMap,
  HealthReport,
} from '../types/knowledge';

export const compileDocument = (docId: string) =>
  invoke<CompileResult>('compile_document_cmd', { docId });

export const compilePendingDocuments = (limit?: number) =>
  invoke<CompileResult[]>('compile_pending_documents_cmd', { limit: limit ?? 10 });

export const getCompileStats = () =>
  invoke<CompileStats>('get_compile_stats_cmd');

export const getKnowledgeMap = (limit?: number) =>
  invoke<KnowledgeMap>('get_knowledge_map_cmd', { limit: limit ?? 50 });

export const runKnowledgeHealthCheck = (staleDays?: number) =>
  invoke<HealthReport>('run_knowledge_health_check_cmd', { staleDays: staleDays ?? 90 });

export const compileAfterScan = (limit?: number) =>
  invoke<CompileResult[]>('compile_after_scan_cmd', { limit: limit ?? 10 });

// ── Knowledge Loop ──────────────────────────────────────────────────

export interface KnowledgeGap {
  topic: string;
  queryCount: number;
  avgConfidence: number;
  suggestion: string;
}

export const getKnowledgeGaps = (minQueries?: number) =>
  invoke<KnowledgeGap[]>('get_knowledge_gaps_cmd', { minQueries: minQueries ?? 2 });

export const suggestExplorations = (limit?: number) =>
  invoke<string[]>('suggest_explorations_cmd', { limit: limit ?? 10 });


// ── Tool Approval ────────────────────────────────────────────────────
import type { ApprovalDecisionValue, ApprovalPolicyList } from '../types';

export const approveToolCall = (requestId: string, decision: ApprovalDecisionValue) =>
  invoke<void>('approve_tool_call_cmd', { requestId, decision });

export const listToolApprovalPolicies = () =>
  invoke<ApprovalPolicyList>('list_tool_approval_policies_cmd');

export const deleteToolApprovalPolicy = (
  toolName: string,
  scope: 'session' | 'forever',
  permissionKey?: string | null,
) =>
  invoke<void>('delete_tool_approval_policy_cmd', { toolName, scope, permissionKey });

export const clearToolApprovalPolicies = () =>
  invoke<void>('clear_tool_approval_policies_cmd');
