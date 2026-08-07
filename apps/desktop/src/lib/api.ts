import { invoke } from "@tauri-apps/api/core";
import type {
  Source,
  ScanError,
  EvidenceCard,
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
import type {
  MediaAnalysisWarning,
  MediaRuntimeStatus,
  VideoConfig,
  TranscriptChunk,
  TranscriptSegment,
  VideoMetadata,
  VisualEvent,
} from "../types/video";
import type {
  CreateMediaJobRequest,
  DeleteMediaAssetOccurrenceRequest,
  MediaAssetRecord,
  MediaJobSnapshot,
  MediaProviderEventRecord,
  RequestMediaJobCancellation,
  RequestMediaJobRemoteDeletion,
  RequestMediaAssetDeletion,
} from "../types/mediaGeneration";
import type {
  AddVideoWorkflowShotRequest,
  AddVideoTimelineClipRequest,
  CancelVideoTimelineExportRequest,
  CancelVideoVariantRequest,
  CreateVideoTimelineExportRequest,
  CreateVideoWorkflowRequest,
  DeleteVideoWorkflowShotRequest,
  PreviewVideoShotQueueRequest,
  QueueVideoShotVariantsRequest,
  ReorderVideoWorkflowShotsRequest,
  RetryVideoTimelineExportRequest,
  ReorderVideoWorkflowVariantsRequest,
  RefreshVideoTimelineClipRequest,
  RemoveVideoTimelineClipRequest,
  ReorderVideoTimelineClipsRequest,
  RetryVideoVariantRequest,
  SaveVideoProviderConnectionRequest,
  SelectVideoWorkflowVariantRequest,
  UpdateVideoWorkflowRequest,
  UpdateVideoWorkflowShotRequest,
  UpdateVideoTimelineClipRequest,
  VideoProviderConnectionRecord,
  VideoProviderPreset,
  VideoQueueDisclosure,
  VideoTimelineExportRecord,
  VideoTimelineSnapshot,
  VerifiedVideoReferenceImage,
  VideoWorkflowSnapshot,
} from "../types/videoWorkflow";
import type {
  SettingsMigrationReportV2,
  CapabilityBindingV2,
  SettingsProfileV2,
  SettingsSchemaStateV2,
  SettingsScopeV2,
} from "../types/settingsSchemaV2";
import type {
  CapabilityRegistryProjection,
  RegistryActivationRecord,
  RegistryReadMode,
  RegistryScope,
} from "../types/capabilityRegistry";
import type {
  AgentConfig,
  AppConfig,
  SaveAgentConfigInput,
  Conversation,
  ConversationMessage,
  ConversationTurn,
  InteractionRequest,
  InteractionResponse,
  SubmitInteractionResponse,
  AgentTaskRun,
  AgentTaskRunListItem,
  AgentTaskRunPageCursor,
  AgentTaskRunSummaryPage,
  AgentTaskRunEvent,
  AgentRunEvent,
  UsageSnapshot,
  AgentTurnHandle,
  AgentSubtaskRun,
  AgentExecutionGraph,
  AgentTaskArtifact,
  AgentTaskArtifactSummary,
  AgentTaskArtifactVersion,
  CreateAgentTaskArtifactInput,
  UpdateAgentTaskArtifactInput,
  CapabilityPackageView,
  PackageHealthState,
  PackageHostSnapshot,
  ToolAccessInfo,
  ConversationStats,
  ConversationSearchResult,
  ImageAttachment,
  VisionTurnOverride,
  ArtifactPayload,
  Checkpoint,
  CheckpointBranch,
  FileCheckpoint,
  FileCheckpointRestore,
  UserMemory,
  AgentProceduralMemory,
  WebSearchConfig,
  WebSearchProviderStatus,
  TextToSpeechConfig,
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
import type {
  TraceSummary,
  AgentTrace,
  Trajectory,
  TrajectoryRedactionProfile,
  TrajectoryStoreSummary,
  EvalPack,
  EvalReport,
  StoredTrajectoryEvalReport,
  TrajectoryReplayRequest,
  TrajectoryReplayReport,
  TrajectoryReplayExecution,
  TrajectoryReplayRuntimeMode,
} from "../types/trace";
import type { DeveloperEvalSmokeReport, QualityEvalReport } from "../types/qualityEval";
import type {
  BrowserEvidenceCapture,
  InvestigationGraph,
  LearningGovernanceSnapshot,
  SaveWorkflowAutomationInput,
  TaskOrchestratorDeliveryEnvelope,
  TaskOrchestratorExecutionTicket,
  TaskOrchestratorWorkflowLaunch,
  TaskResumeCheckpoint,
  TaskResumePrompt,
  WorkflowAutomation,
  WorkflowAutomationDueRun,
  WorkflowAutomationRun,
  WorkflowAutomationSchedulerEvent,
} from "../types/workflows";
import type {
  DreamArtifact,
  DreamArtifactFilters,
  DreamRun,
  DreamRunEvent,
  StartDreamInput,
  UpdateDreamArtifactInput,
} from "../types/dreaming";
import type { ProviderPreset } from "./providerPresets";
import type { ProviderModelCatalogSnapshot } from "./providerModelCatalog";

export type {
  DreamArtifact,
  DreamArtifactFilters,
  DreamRun,
  DreamRunEvent,
  StartDreamInput,
  UpdateDreamArtifactInput,
} from "../types/dreaming";

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

export const testApiConnection = (apiKey: string, baseUrl: string, model: string, dimensions: number) =>
  invoke<boolean>('test_api_connection_cmd', { apiKey, baseUrl, model, dimensions });

export const checkLocalModel = (localModel?: string, modelPath?: string) =>
  invoke<boolean>('check_local_model_cmd', { localModel, modelPath });

export const downloadLocalModel = (localModel?: string, modelPath?: string) =>
  invoke<void>('download_local_model_cmd', { localModel, modelPath });

export const cancelModelDownload = () =>
  invoke<void>('cancel_model_download_cmd');

export const deleteLocalModel = (localModel?: string, modelPath?: string) =>
  invoke<void>('delete_local_model_cmd', { localModel, modelPath });

// ── File ────────────────────────────────────────────────────────────────

export const openFileInDefaultApp = (path: string) =>
  invoke<void>('open_file_in_default_app', { path });

export const showInFileExplorer = (path: string) =>
  invoke<void>('show_in_file_explorer', { path });

// ── Terminal ────────────────────────────────────────────────────────────

export type TerminalShell = 'default' | 'powershell' | 'cmd' | 'bash';

export interface TerminalStartInput {
  cwd?: string | null;
  shell?: TerminalShell | string | null;
  rows?: number | null;
  cols?: number | null;
  conversationId?: string | null;
}

export interface TerminalSessionInfo {
  id: string;
  shell: string;
  cwd: string;
  processId?: number | null;
  conversationId?: string | null;
}

export interface TerminalSessionSnapshot {
  session: TerminalSessionInfo;
  output: string;
  outputStart: number;
  outputEnd: number;
}

export interface TerminalEvent {
  sessionId: string;
  kind: 'data' | 'exit' | 'error';
  data?: string | null;
  exitCode?: number | null;
  signal?: string | null;
}

export const startTerminalSession = (input: TerminalStartInput) =>
  invoke<TerminalSessionInfo>('terminal_start_session_cmd', { input });

export const writeTerminalSession = (sessionId: string, data: string) =>
  invoke<void>('terminal_write_session_cmd', { sessionId, data });

export const resizeTerminalSession = (sessionId: string, rows: number, cols: number) =>
  invoke<void>('terminal_resize_session_cmd', { sessionId, rows, cols });

export const closeTerminalSession = (sessionId: string) =>
  invoke<void>('terminal_close_session_cmd', { sessionId });

export const bindTerminalSession = (sessionId: string, conversationId?: string | null) =>
  invoke<TerminalSessionInfo>('terminal_bind_session_cmd', {
    sessionId,
    conversationId: conversationId ?? null,
  });

export const snapshotTerminalSession = (sessionId: string, maxChars = 24_000) =>
  invoke<TerminalSessionSnapshot>('terminal_snapshot_session_cmd', { sessionId, maxChars });

export const listTerminalSessions = () =>
  invoke<TerminalSessionInfo[]>('terminal_list_sessions_cmd');

export const activeTerminalSession = (conversationId: string) =>
  invoke<TerminalSessionInfo | null>('terminal_active_session_cmd', { conversationId });

// ── Browser Workspace ─────────────────────────────────────────────────

export type BrowserControlOwner =
  | { type: 'none' }
  | { type: 'user' }
  | { type: 'agent'; callId: string };

export interface BrowserBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface BrowserTabInfo {
  id: string;
  sessionId: string;
  url: string;
  title: string;
  active: boolean;
  loading: boolean;
  status: string;
}

export interface BrowserSessionInfo {
  id: string;
  conversationId?: string | null;
  profileId: string;
  activeTabId?: string | null;
  tabs: BrowserTabInfo[];
  controlOwner: BrowserControlOwner;
}

export interface BrowserCreateInput {
  conversationId?: string | null;
  profileId?: string | null;
  url?: string | null;
  openInitialUrlOnReuse?: boolean;
  bounds?: BrowserBounds | null;
}

export interface BrowserEvent {
  kind: string;
  payload: Record<string, unknown>;
}

export interface BrowserElementArtifact {
  kind: 'element';
  url: string;
  title: string;
  ref: string;
  tag: string;
  role: string;
  name: string;
  href?: string | null;
  inputType?: string | null;
  bounds: BrowserBounds;
  locatorFingerprint: Record<string, string | null | undefined>;
  userEpoch: number;
}

export interface BrowserRegionArtifact {
  kind: 'region';
  capture: 'coordinatesOnly';
  url: string;
  title: string;
  bounds: BrowserBounds;
  userEpoch: number;
}

export type BrowserPickArtifact = BrowserElementArtifact | BrowserRegionArtifact;

export const createBrowserSession = (input: BrowserCreateInput) =>
  invoke<BrowserSessionInfo>('browser_create_session_cmd', { input });

export const listBrowserSessions = () =>
  invoke<BrowserSessionInfo[]>('browser_list_sessions_cmd');

export const activeBrowserSession = (conversationId: string) =>
  invoke<BrowserSessionInfo | null>('browser_active_session_cmd', { conversationId });

export const openBrowserTab = (sessionId: string, url: string, bounds?: BrowserBounds | null) =>
  invoke<BrowserTabInfo>('browser_open_tab_cmd', { sessionId, url, bounds: bounds ?? null });

export const openBrowserPopup = (
  sessionId: string,
  sourceTabId: string,
  url: string,
  bounds?: BrowserBounds | null,
) => invoke<BrowserTabInfo>('browser_open_popup_cmd', {
  sessionId,
  sourceTabId,
  url,
  bounds: bounds ?? null,
});

export const navigateBrowserTab = (sessionId: string, tabId: string, url: string) =>
  invoke<BrowserTabInfo>('browser_navigate_cmd', { sessionId, tabId, url });

export const activateBrowserTab = (sessionId: string, tabId: string) =>
  invoke<BrowserSessionInfo>('browser_activate_tab_cmd', { sessionId, tabId });

export const setBrowserBounds = (sessionId: string, bounds: BrowserBounds, visible: boolean) =>
  invoke<void>('browser_set_bounds_cmd', { sessionId, bounds, visible });

export const goBackBrowserTab = (sessionId: string, tabId: string) =>
  invoke<void>('browser_go_back_cmd', { sessionId, tabId });

export const goForwardBrowserTab = (sessionId: string, tabId: string) =>
  invoke<void>('browser_go_forward_cmd', { sessionId, tabId });

export const reloadBrowserTab = (sessionId: string, tabId: string) =>
  invoke<void>('browser_reload_cmd', { sessionId, tabId });

export const stopBrowserTab = (sessionId: string, tabId: string) =>
  invoke<void>('browser_stop_cmd', { sessionId, tabId });

export const beginBrowserElementPick = (sessionId: string, tabId: string) =>
  invoke<void>('browser_begin_element_pick_cmd', { sessionId, tabId });

export const beginBrowserRegionPick = (sessionId: string, tabId: string) =>
  invoke<void>('browser_begin_region_pick_cmd', { sessionId, tabId });

export const takeBrowserPick = (sessionId: string, tabId: string) =>
  invoke<BrowserPickArtifact | null>('browser_take_pick_cmd', { sessionId, tabId });

export const selectedBrowserText = (sessionId: string, tabId: string) =>
  invoke<string>('browser_selected_text_cmd', { sessionId, tabId });

export const acquireBrowserControl = (sessionId: string, owner: 'user' | 'none') =>
  invoke<BrowserSessionInfo>('browser_acquire_control_cmd', { sessionId, owner });

export const closeBrowserTab = (sessionId: string, tabId: string) =>
  invoke<BrowserSessionInfo>('browser_close_tab_cmd', { sessionId, tabId });

export const closeBrowserSession = (sessionId: string) =>
  invoke<void>('browser_close_session_cmd', { sessionId });

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

export interface WebPreviewProbe {
  embeddable: boolean;
  reason?: string | null;
  document?: string | null;
}

export const probeWebPreview = (url: string) =>
  invoke<WebPreviewProbe>('probe_web_preview_cmd', { url });

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

export const getSettingsSchemaStateV2 = () =>
  invoke<SettingsSchemaStateV2>('get_settings_schema_state_v2_cmd');

export const listSettingsProfilesV2 = () =>
  invoke<SettingsProfileV2[]>('list_settings_profiles_v2_cmd');

export const saveSettingsProfileV2 = (
  profile: SettingsProfileV2,
  expectedRevision?: number | null,
) =>
  invoke<SettingsProfileV2>('save_settings_profile_v2_cmd', {
    profile,
    expectedRevision: expectedRevision ?? null,
  });

export const saveCapabilityBindingV2 = (
  scope: SettingsScopeV2,
  capabilityId: string,
  binding: CapabilityBindingV2,
  expectedProfileRevision: number,
) => invoke<SettingsProfileV2>('save_capability_binding_v2_cmd', {
  scope,
  capabilityId,
  binding,
  expectedProfileRevision,
});

export const deleteVisionObservationCache = (
  attachmentHash: string,
  profileHash?: string | null,
) => invoke<number>('delete_vision_observation_cache_cmd', {
  attachmentHash,
  profileHash: profileHash ?? null,
});

export const clearVisionObservationCache = () =>
  invoke<number>('clear_vision_observation_cache_cmd');

export const migrateSettingsSchemaV2 = () =>
  invoke<SettingsMigrationReportV2>('migrate_settings_schema_v2_cmd');

export const rollbackSettingsSchemaV2 = () =>
  invoke<boolean>('rollback_settings_schema_v2_cmd');

export const getCapabilityRegistryProjection = (scope: RegistryScope = {}) =>
  invoke<CapabilityRegistryProjection>('get_capability_registry_projection_cmd', { scope });

export const setCapabilityRegistryReadMode = (
  capabilityId: string,
  scope: SettingsScopeV2,
  mode: RegistryReadMode,
  expectedRevision: number,
) => invoke<RegistryActivationRecord>('set_capability_registry_read_mode_cmd', {
  capabilityId,
  scope,
  mode,
  expectedRevision,
});

export const testAgentConnection = (config: SaveAgentConfigInput) =>
  invoke<ProviderModelCatalogSnapshot>('test_agent_connection_cmd', { config });

export const refreshProviderModelCatalog = (config: SaveAgentConfigInput) =>
  invoke<ProviderModelCatalogSnapshot>('refresh_provider_model_catalog_cmd', { config });

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

export const prepareWorkflowAutomationDelivery = (id: string) =>
  invoke<TaskOrchestratorDeliveryEnvelope>('prepare_workflow_automation_delivery_cmd', { id });

export const prepareDueWorkflowAutomationDelivery = (id: string, now?: string | null) =>
  invoke<TaskOrchestratorDeliveryEnvelope>('prepare_due_workflow_automation_delivery_cmd', {
    id,
    now: now ?? null,
  });

export const queueWorkflowAutomationDelivery = (id: string, summary?: string | null) =>
  invoke<TaskOrchestratorExecutionTicket>('queue_workflow_automation_delivery_cmd', {
    id,
    summary: summary ?? null,
  });

export const queueDueWorkflowAutomationDelivery = (
  id: string,
  now?: string | null,
  summary?: string | null,
) =>
  invoke<TaskOrchestratorExecutionTicket>('queue_due_workflow_automation_delivery_cmd', {
    id,
    now: now ?? null,
    summary: summary ?? null,
  });

export const startDueWorkflowAutomationRun = (
  id: string,
  now?: string | null,
  conversationId?: string | null,
  agentConfigId?: string | null,
  personaId?: string | null,
  skillIds?: string[] | null,
  executionMode?: AgentExecutionMode | null,
  summary?: string | null,
) =>
  invoke<TaskOrchestratorWorkflowLaunch>('start_due_workflow_automation_run_cmd', {
    id,
    now: now ?? null,
    conversationId: conversationId ?? null,
    agentConfigId: agentConfigId ?? null,
    personaId: personaId ?? null,
    skillIds: skillIds && skillIds.length > 0 ? skillIds : null,
    executionMode: executionMode ?? null,
    summary: summary ?? null,
  });

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

export const listWorkflowAutomationSchedulerEvents = (
  automationId?: string | null,
  limit?: number | null,
) =>
  invoke<WorkflowAutomationSchedulerEvent[]>('list_workflow_automation_scheduler_events_cmd', {
    automationId: automationId ?? null,
    limit: limit ?? null,
  });

export const listWorkflowAutomationSchedulerEventsForTaskRun = (
  taskRunId: string,
  limit?: number | null,
) =>
  invoke<WorkflowAutomationSchedulerEvent[]>('list_workflow_automation_scheduler_events_for_task_run_cmd', {
    taskRunId,
    limit: limit ?? null,
  });

export const exportWorkflowAutomationTrajectory = (
  workflowRunId: string,
  redactionProfile?: TrajectoryRedactionProfile,
) =>
  invoke<Trajectory>('export_workflow_automation_trajectory_cmd', {
    workflowRunId,
    redactionProfile,
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

export const listArchivedConversations = () =>
  invoke<Conversation[]>('list_archived_conversations_cmd');

export const getConversation = (id: string) =>
  invoke<[Conversation, ConversationMessage[]]>('get_conversation_cmd', { id });

export const getConversationTurns = (conversationId: string) =>
  invoke<ConversationTurn[]>('get_conversation_turns_cmd', { conversationId });

export const listInteractionRequests = (
  conversationId: string | null = null,
  includeTerminal = false,
) => invoke<InteractionRequest[]>('list_interaction_requests_cmd', {
  conversationId,
  includeTerminal,
});

export const getInteractionRequest = (interactionId: string) =>
  invoke<InteractionRequest>('get_interaction_request_cmd', { interactionId });

export const markInteractionPresented = (interactionId: string) =>
  invoke<InteractionRequest>('mark_interaction_presented_cmd', { interactionId });

export const markInteractionPartiallyAnswered = (interactionId: string) =>
  invoke<InteractionRequest>('mark_interaction_partially_answered_cmd', { interactionId });

export const appendInteractionSupplement = (interactionId: string, content: string) =>
  invoke<ConversationMessage>('append_interaction_supplement_cmd', { interactionId, content });

export const submitInteractionResponse = (input: SubmitInteractionResponse) =>
  invoke<InteractionResponse>('submit_interaction_response_cmd', { input });

export const getInteractionResponse = (interactionId: string) =>
  invoke<InteractionResponse>('get_interaction_response_cmd', { interactionId });

export const acknowledgeInteraction = (interactionId: string) =>
  invoke<InteractionRequest>('acknowledge_interaction_cmd', { interactionId });

export const cancelInteraction = (interactionId: string) =>
  invoke<InteractionRequest>('cancel_interaction_cmd', { interactionId });

export const supersedeInteraction = (interactionId: string) =>
  invoke<InteractionRequest>('supersede_interaction_cmd', { interactionId });

export const failInteraction = (interactionId: string) =>
  invoke<InteractionRequest>('fail_interaction_cmd', { interactionId });

export const getAgentTaskRuns = (conversationId: string) =>
  invoke<AgentTaskRun[]>('get_agent_task_runs_cmd', { conversationId });

export const listRecentAgentTaskRuns = (limit = 50) =>
  invoke<AgentTaskRunListItem[]>('list_recent_agent_task_runs_cmd', { limit });

export const listAgentTaskRunSummaries = (
  limit = 25,
  cursor: AgentTaskRunPageCursor | null = null,
  status: string | null = null,
  projectId: string | null = null,
) => invoke<AgentTaskRunSummaryPage>('list_agent_task_run_summaries_cmd', {
  limit,
  cursor,
  status,
  projectId,
});

export const getAgentTaskRunEvents = (runId: string) =>
  invoke<AgentTaskRunEvent[]>('get_agent_task_run_events_cmd', { runId });

export const getAgentRunEvents = (runId: string) =>
  invoke<AgentRunEvent[]>('get_agent_run_events_cmd', { runId });

export const getRunUsageSnapshot = (runId: string) =>
  invoke<UsageSnapshot | null>('get_run_usage_snapshot_cmd', { runId });

export const getConversationUsageSnapshot = (conversationId: string) =>
  invoke<UsageSnapshot | null>('get_conversation_usage_snapshot_cmd', { conversationId });

export interface UsageAnalyticsFilter {
  startAt?: string | null;
  endAt?: string | null;
  providerId?: string | null;
  modelId?: string | null;
  operationKind?: string | null;
  timeBucket?: 'day' | 'week' | 'month' | null;
}

export interface UsageTotals {
  requestCount: number;
  agentRunCount: number;
  promptTokens: number;
  completionTokens: number;
  thinkingTokens: number;
  totalTokens: number;
  cacheReadTokens: number;
  cacheMissTokens: number;
  cacheCreationTokens: number;
  cacheHitRate?: number | null;
  estimatedCostMicros?: number | null;
  currency?: string | null;
  providerReportedPercent: number;
  normalizedPercent: number;
  estimatedPercent: number;
  unknownPercent: number;
}

export interface UsageBreakdownRow {
  key: string;
  providerId?: string | null;
  modelId?: string | null;
  requestCount: number;
  agentRunCount: number;
  turnCount: number;
  successCount: number;
  promptTokens: number;
  completionTokens: number;
  thinkingTokens: number;
  totalTokens: number;
  cacheReadTokens: number;
  cacheMissTokens: number;
  estimatedCostMicros?: number | null;
}

export interface UsageTimeSeriesPoint {
  date: string;
  requestCount: number;
  promptTokens: number;
  completionTokens: number;
  thinkingTokens: number;
  cacheReadTokens: number;
  cacheMissTokens: number;
  cacheCreationTokens: number;
  estimatedCostMicros?: number | null;
}

export interface UsageAnalytics {
  totals: UsageTotals;
  byModel: UsageBreakdownRow[];
  byOperation: UsageBreakdownRow[];
  timeSeries: UsageTimeSeriesPoint[];
}

export const getAiUsageAnalytics = (filter: UsageAnalyticsFilter) =>
  invoke<UsageAnalytics>('get_ai_usage_analytics_cmd', { filter });

export const deleteAiUsageRecords = (filter: UsageAnalyticsFilter) =>
  invoke<number>('delete_ai_usage_records_cmd', { filter });

export const exportAiUsage = (filter: UsageAnalyticsFilter, format: 'csv' | 'json', path: string) =>
  invoke<void>('export_ai_usage_cmd', { filter, format, path });

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

export const listCapabilityPackages = (options?: { includeRuntimeChecks?: boolean }) =>
  invoke<CapabilityPackageView[]>('list_capability_packages_cmd', {
    includeRuntimeChecks: options?.includeRuntimeChecks ?? false,
  });

export const getPackageHostSnapshot = () =>
  invoke<PackageHostSnapshot>('get_package_host_snapshot_cmd');

export const setPackageHostPackageEnabled = (packageId: string, enabled: boolean) =>
  invoke<PackageHostSnapshot>('set_package_host_package_enabled_cmd', { packageId, enabled });

export const setPackageHostPackageHealth = (
  packageId: string,
  healthState: PackageHealthState,
) => invoke<PackageHostSnapshot>('set_package_host_package_health_cmd', {
  packageId,
  healthState,
});

export const listProjectTools = (sourceScope?: string[] | null) =>
  invoke<import('../types/project-tool').ProjectToolCatalog>('list_project_tools_cmd', {
    sourceScope: sourceScope ?? null,
  });

export const deleteConversation = (id: string) =>
  invoke<void>('delete_conversation_cmd', { id });

export const archiveConversation = (id: string) =>
  invoke<Conversation>('archive_conversation_cmd', { id });

export const unarchiveConversation = (id: string) =>
  invoke<Conversation>('unarchive_conversation_cmd', { id });

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

export type AgentExecutionMode = 'normal' | 'plan';
export type AgentPowerMode = 'standard' | 'nexus';
export type AgentCollaborationMode = 'direct' | 'mixtureOfAgents';
export type MoaPresetId = 'fastReview' | 'deepResearch' | 'crossModelCodeReview' | 'custom';
export type OrchestrationProfile = 'balanced' | 'deep' | 'codeUltra' | 'researchUltra' | 'custom';
export interface CustomOrchestrationOptions {
  maxIterations?: number | null;
  maxParallel?: number | null;
  maxCallsPerTurn?: number | null;
  delegatedTokenBudget?: number | null;
  verificationReservePercent?: number | null;
  retryLimit?: number | null;
  minEvidenceSources?: number | null;
}

export const agentChat = (
  conversationId: string,
  message: string,
  attachments?: ImageAttachment[],
  agentConfigId?: string | null,
  personaId?: string | null,
  skillIds?: string[],
  executionMode?: AgentExecutionMode | null,
  powerMode?: AgentPowerMode | null,
  collaborationMode?: AgentCollaborationMode | null,
  moaPreset?: MoaPresetId | null,
  orchestrationProfile?: OrchestrationProfile | null,
  customOrchestration?: CustomOrchestrationOptions | null,
  visionTurnOverride?: VisionTurnOverride | null,
  userArtifacts?: ArtifactPayload | null,
  taskOrchestratorRunId?: string | null,
) => {
  const interactionId = userArtifacts
    && !Array.isArray(userArtifacts)
    && userArtifacts.kind === 'questionResponse'
    && userArtifacts.version === 2
    && typeof userArtifacts.interactionId === 'string'
    ? userArtifacts.interactionId.trim()
    : '';
  const request = {
    version: 1,
    idempotencyKey: interactionId
      ? `interaction-response:${interactionId}`
      : crypto.randomUUID(),
    conversationId,
    message,
    attachments: attachments ?? [],
    agentConfigId: agentConfigId ?? null,
    personaId: personaId ?? null,
    skillIds: skillIds ?? [],
    executionMode: executionMode ?? 'normal',
    powerMode: powerMode ?? 'standard',
    collaborationMode: collaborationMode ?? 'direct',
    moaPreset: moaPreset ?? 'fastReview',
    orchestrationProfile: orchestrationProfile ?? 'balanced',
    customOrchestration: customOrchestration ?? null,
    visionTurnOverride: visionTurnOverride ?? null,
    userArtifacts: userArtifacts ?? null,
    taskOrchestratorRunId: taskOrchestratorRunId ?? null,
  };
  return invoke<AgentTurnHandle>('agent_chat_cmd', { request });
};

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

export interface CompactConversationResult {
  conversationId: string;
  messagesBefore: number;
  messagesAfter: number;
  tokensBefore: number;
  tokensAfter: number;
  evictedMessages: number;
}

export type ContextCompactionPhase =
  | 'queued'
  | 'planning'
  | 'summarizing'
  | 'validating'
  | 'committing';

export type ActivityState =
  | 'queued'
  | 'starting'
  | 'running'
  | 'ready'
  | 'waiting_input'
  | 'quiet'
  | 'suspended'
  | 'cancelling'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'orphaned'
  | 'superseded'
  | 'timed_out';

export interface ContextCompactionHandle {
  operationId: string;
  conversationId: string;
  snapshotVersion: string;
  state: ActivityState;
  phase: ContextCompactionPhase;
}

export interface ContextCompactionResult extends CompactConversationResult {
  checkpointId: string | null;
  summaryKind: string;
  fallbackReason: string | null;
}

export interface ActivityEvent {
  activityId: string;
  seq: number;
  timestamp: string;
  kind: string;
  payload: Record<string, unknown>;
}

export interface ActivityObservation {
  record: {
    activityId: string;
    state: ActivityState;
    startedAt: string;
    updatedAt: string;
    completedAt?: string | null;
    lastEventSeq: number;
  };
  cursor: number;
  events: ActivityEvent[];
  timedOut: boolean;
}

export const startContextCompaction = (
  conversationId: string,
  idempotencyKey: string,
) => invoke<ContextCompactionHandle>('start_context_compaction_cmd', {
  request: { conversationId, idempotencyKey },
});

export const observeContextCompaction = (operationId: string, afterSeq: number) =>
  invoke<ActivityObservation>('observe_context_compaction_cmd', { operationId, afterSeq });

export const cancelContextCompaction = (operationId: string) =>
  invoke<void>('cancel_context_compaction_cmd', { operationId });

export const createMediaGenerationJob = (request: CreateMediaJobRequest) =>
  invoke<MediaJobSnapshot>('create_media_generation_job_cmd', { request });

export const getMediaGenerationJob = (jobId: string) =>
  invoke<MediaJobSnapshot>('get_media_generation_job_cmd', { jobId });

export const listRecoverableMediaGenerationJobs = () =>
  invoke<MediaJobSnapshot[]>('list_recoverable_media_generation_jobs_cmd');

export const listMediaGenerationProviderEvents = (
  jobId: string,
  afterSequence = 0,
  limit = 100,
) => invoke<MediaProviderEventRecord[]>('list_media_generation_provider_events_cmd', {
  jobId,
  afterSequence,
  limit,
});

export const requestMediaGenerationCancellation = (request: RequestMediaJobCancellation) =>
  invoke<MediaJobSnapshot>('request_media_generation_cancellation_cmd', { request });

export const requestMediaGenerationRemoteDeletion = (request: RequestMediaJobRemoteDeletion) =>
  invoke<MediaJobSnapshot>('request_media_generation_remote_deletion_cmd', { request });

export const deleteMediaGenerationAssetOccurrence = (
  request: DeleteMediaAssetOccurrenceRequest,
) => invoke<MediaJobSnapshot>('delete_media_generation_asset_occurrence_cmd', { request });

export const deleteMediaGenerationAsset = (request: RequestMediaAssetDeletion) =>
  invoke<MediaAssetRecord>('delete_media_generation_asset_cmd', { request });

export const listVideoGenerationCapabilities = () =>
  invoke<VideoProviderPreset[]>('list_video_generation_capabilities_cmd');

export const saveVideoProviderConnection = (request: SaveVideoProviderConnectionRequest) =>
  invoke<VideoProviderConnectionRecord>('save_video_provider_connection_cmd', { request });

export const listVideoProviderConnections = () =>
  invoke<VideoProviderConnectionRecord[]>('list_video_provider_connections_cmd');

export const deleteVideoProviderConnection = (connectionId: string, expectedRevision: number) =>
  invoke<void>('delete_video_provider_connection_cmd', { connectionId, expectedRevision });

export const createVideoWorkflow = (request: CreateVideoWorkflowRequest) =>
  invoke<VideoWorkflowSnapshot>('create_video_workflow_cmd', { request });

export const updateVideoWorkflow = (request: UpdateVideoWorkflowRequest) =>
  invoke<VideoWorkflowSnapshot>('update_video_workflow_cmd', { request });

export const listVideoWorkflows = (projectId?: string | null) =>
  invoke<VideoWorkflowSnapshot[]>('list_video_workflows_cmd', { projectId: projectId ?? null });

export const getVideoWorkflow = (workflowId: string) =>
  invoke<VideoWorkflowSnapshot>('get_video_workflow_cmd', { workflowId });

export const addVideoWorkflowShot = (request: AddVideoWorkflowShotRequest) =>
  invoke<VideoWorkflowSnapshot>('add_video_workflow_shot_cmd', { request });

export const updateVideoWorkflowShot = (request: UpdateVideoWorkflowShotRequest) =>
  invoke<VideoWorkflowSnapshot>('update_video_workflow_shot_cmd', { request });

export const reorderVideoWorkflowShots = (request: ReorderVideoWorkflowShotsRequest) =>
  invoke<VideoWorkflowSnapshot>('reorder_video_workflow_shots_cmd', { request });

export const reorderVideoWorkflowVariants = (request: ReorderVideoWorkflowVariantsRequest) =>
  invoke<VideoWorkflowSnapshot>('reorder_video_workflow_variants_cmd', { request });

export const deleteVideoWorkflowShot = (request: DeleteVideoWorkflowShotRequest) =>
  invoke<VideoWorkflowSnapshot>('delete_video_workflow_shot_cmd', { request });

export const queueVideoShotVariants = (request: QueueVideoShotVariantsRequest) =>
  invoke<VideoWorkflowSnapshot>('queue_video_shot_variants_cmd', { request });

export const previewVideoShotQueue = (request: PreviewVideoShotQueueRequest) =>
  invoke<VideoQueueDisclosure>('preview_video_shot_queue_cmd', { request });

export const inspectVideoReferenceImage = (uri: string) =>
  invoke<VerifiedVideoReferenceImage>('inspect_video_reference_image_cmd', { uri });

export const retryVideoVariant = (request: RetryVideoVariantRequest) =>
  invoke<VideoWorkflowSnapshot>('retry_video_variant_cmd', { request });

export const cancelVideoVariant = (request: CancelVideoVariantRequest) =>
  invoke<VideoWorkflowSnapshot>('cancel_video_variant_cmd', { request });

export const selectVideoWorkflowVariant = (request: SelectVideoWorkflowVariantRequest) =>
  invoke<VideoWorkflowSnapshot>('select_video_workflow_variant_cmd', { request });

export const resolveMediaGenerationAssetPath = (assetId: string) =>
  invoke<string>('resolve_media_generation_asset_path_cmd', { assetId });

export const getVideoTimeline = (workflowId: string) =>
  invoke<VideoTimelineSnapshot>('get_video_timeline_cmd', { workflowId });

export const addVideoTimelineClip = (request: AddVideoTimelineClipRequest) =>
  invoke<VideoTimelineSnapshot>('add_video_timeline_clip_cmd', { request });

export const refreshVideoTimelineClip = (request: RefreshVideoTimelineClipRequest) =>
  invoke<VideoTimelineSnapshot>('refresh_video_timeline_clip_cmd', { request });

export const updateVideoTimelineClip = (request: UpdateVideoTimelineClipRequest) =>
  invoke<VideoTimelineSnapshot>('update_video_timeline_clip_cmd', { request });

export const reorderVideoTimelineClips = (request: ReorderVideoTimelineClipsRequest) =>
  invoke<VideoTimelineSnapshot>('reorder_video_timeline_clips_cmd', { request });

export const removeVideoTimelineClip = (request: RemoveVideoTimelineClipRequest) =>
  invoke<VideoTimelineSnapshot>('remove_video_timeline_clip_cmd', { request });

export const createVideoTimelineExport = (request: CreateVideoTimelineExportRequest) =>
  invoke<VideoTimelineExportRecord>('create_video_timeline_export_cmd', { request });

export const cancelVideoTimelineExport = (request: CancelVideoTimelineExportRequest) =>
  invoke<VideoTimelineExportRecord>('cancel_video_timeline_export_cmd', { request });

export const retryVideoTimelineExport = (request: RetryVideoTimelineExportRequest) =>
  invoke<VideoTimelineExportRecord>('retry_video_timeline_export_cmd', { request });

/** @deprecated Compatibility adapter for pre-operation-protocol callers. */
export const compactConversation = (conversationId: string) =>
  invoke<CompactConversationResult>('compact_conversation_cmd', { conversationId });

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

export const deleteOcrModels = (config: OcrConfig) =>
  invoke<void>('delete_ocr_models_cmd', { config });

export interface ManagedModelPaths {
  root: string;
  embedding: string;
  ocr: string;
  whisper: string;
}

export const getManagedModelPaths = (root?: string, localModel?: string) =>
  invoke<ManagedModelPaths>('get_managed_model_paths_cmd', { root, localModel });

// ── App Config ──────────────────────────────────────────────────────

export const getAppConfig = () =>
  invoke<AppConfig>('get_app_config_cmd');

export const saveAppConfig = (config: AppConfig) =>
  invoke<void>('save_app_config_cmd', { config });

export interface SpeechPreview {
  assetId: string;
  path: string;
  mediaType: string;
  bytes: number;
}

export const synthesizeSpeechPreview = (text: string, config?: TextToSpeechConfig) =>
  invoke<SpeechPreview>('synthesize_speech_preview_cmd', { text, config });

export interface ClearSpeechCacheResult {
  removedFiles: number;
  removedBytes: number;
}

export const clearSpeechCache = () =>
  invoke<ClearSpeechCacheResult>('clear_speech_cache_cmd');

export const refreshTtsVoiceCatalog = (config: TextToSpeechConfig) =>
  invoke<import('./ttsVoiceCatalog').TtsVoiceCatalogSnapshot>(
    'refresh_tts_voice_catalog_cmd',
    { config },
  );

export interface ThemeBackgroundAsset {
  assetId: string;
  path: string;
  mediaType: string;
  bytes: number;
}

export const importThemeBackground = (sourcePath: string) =>
  invoke<ThemeBackgroundAsset>('import_theme_background_cmd', { sourcePath });

export const resolveThemeBackground = (assetId: string) =>
  invoke<ThemeBackgroundAsset>('resolve_theme_background_cmd', { assetId });

export const garbageCollectThemeAssets = (retainedAssetIds: string[]) =>
  invoke<ClearSpeechCacheResult>('garbage_collect_theme_assets_cmd', { retainedAssetIds });

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

export const getMediaRuntimeStatus = () =>
  invoke<MediaRuntimeStatus>('get_media_runtime_status_cmd');

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

export interface VoiceAudioSpoolStarted {
  sessionId: string;
  sampleRate: number;
  maxChunkBytes: number;
  maxAudioBytes: number;
}

export interface VoiceAudioSpoolProgress {
  sessionId: string;
  acceptedBytes: number;
  audioBytes: number;
  durationMs: number;
  nextSequence: number;
}

export interface VoiceAudioSpoolDescriptor {
  sessionId: string;
  audioBytes: number;
  durationMs: number;
  sampleRate: number;
  checksumSha256: string;
  createdAtMs: number;
  expiresAtMs: number;
  target: {
    provider: string;
    apiStyle: string;
    model: string;
    configurationFingerprintSha256: string;
  };
}

export interface VoiceAudioSpoolListEntry {
  sessionId: string;
  state: 'recording' | 'ready' | 'deletionPending';
  descriptor?: VoiceAudioSpoolDescriptor;
}

export interface VoiceAudioSpoolTranscriptionResult {
  transcript: string;
  cleanupPending: boolean;
}

export const startVoiceAudioSpool = (sampleRate: number) =>
  invoke<VoiceAudioSpoolStarted>('start_voice_audio_spool_cmd', { sampleRate });

export const appendVoiceAudioSpool = (
  sessionId: string,
  sequence: number,
  audioData: Uint8Array,
) => invoke<VoiceAudioSpoolProgress>('append_voice_audio_spool_cmd', audioData, {
  headers: {
    'x-nexa-voice-session-id': sessionId,
    'x-nexa-voice-sequence': String(sequence),
  },
});

export const finishVoiceAudioSpool = (sessionId: string) =>
  invoke<VoiceAudioSpoolDescriptor>('finish_voice_audio_spool_cmd', { sessionId });

export const listVoiceAudioSpools = () =>
  invoke<VoiceAudioSpoolListEntry[]>('list_voice_audio_spools_cmd');

export const transcribeVoiceAudioSpool = (sessionId: string) =>
  invoke<VoiceAudioSpoolTranscriptionResult>('transcribe_voice_audio_spool_cmd', { sessionId });

export const cancelVoiceAudioSpool = (sessionId: string) =>
  invoke<void>('cancel_voice_audio_spool_cmd', { sessionId });

export const startRealtimeTranscription = () =>
  invoke<string>('start_realtime_transcription_cmd');

export const appendRealtimeTranscriptionAudio = (sessionId: string, audioData: Uint8Array) =>
  invoke<void>('append_realtime_transcription_audio_cmd', audioData, {
    headers: { 'x-nexa-session-id': sessionId },
  });

export const finishRealtimeTranscription = (sessionId: string) =>
  invoke<string>('finish_realtime_transcription_cmd', { sessionId });

export const cancelRealtimeTranscription = (sessionId: string) =>
  invoke<void>('cancel_realtime_transcription_cmd', { sessionId });

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
    (skill) => !builtinIds.has(skill.id),
  );
  return [...builtins, ...filteredUserSkills];
};

export const listActiveSkills = async () =>
  (await listAllSkills()).filter((skill) => skill.enabled);

export const importSkillFromMd = (content: string) =>
  invoke<Skill>('import_skill_from_md_cmd', { content });

export const parseSkillMarkdown = (content: string) =>
  invoke<SaveSkillInput>('parse_skill_markdown_cmd', { content });

export const inspectSkillInstallSource = (source: string) =>
  invoke<DiscoveredSkillBundle[]>('inspect_skill_install_source_cmd', { source });

export const installSkillsFromSource = (
  source: string,
  replaceExisting: boolean,
  acceptBlockedWarnings: boolean,
) => invoke<Skill[]>('install_skills_from_source_cmd', {
  source,
  replaceExisting,
  acceptBlockedWarnings,
});

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
    transcriptSegments: TranscriptSegment[];
    durationSecs: number | null;
    frameTextsCount: number;
    frameTexts: string[];
    visualEvents: VisualEvent[];
    warnings: MediaAnalysisWarning[];
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

export const exportAgentTaskTrajectory = (
  runId: string,
  redactionProfile?: TrajectoryRedactionProfile,
) =>
  invoke<Trajectory>('export_agent_task_trajectory_cmd', {
    runId,
    redactionProfile,
  });

export const saveAgentTrajectory = (trajectory: Trajectory) =>
  invoke<TrajectoryStoreSummary>('save_agent_trajectory_cmd', { trajectory });

export const loadAgentTrajectory = (trajectoryId: string) =>
  invoke<Trajectory>('load_agent_trajectory_cmd', { trajectoryId });

export const listAgentTrajectories = (limit?: number) =>
  invoke<TrajectoryStoreSummary[]>('list_agent_trajectories_cmd', { limit });

export const runTrajectoryEvalPack = (pack: EvalPack) =>
  invoke<EvalReport>('run_trajectory_eval_pack_cmd', { pack });

export const compareTrajectoryReplay = (request: TrajectoryReplayRequest) =>
  invoke<TrajectoryReplayReport>('compare_trajectory_replay_cmd', { request });

export const replayTrajectorySession = (
  trajectoryId: string,
  runtimeMode?: TrajectoryReplayRuntimeMode,
) =>
  invoke<TrajectoryReplayExecution>('replay_trajectory_session_cmd', {
    trajectoryId,
    runtimeMode,
  });

export const runStoredTrajectorySmokeEval = (limit?: number) =>
  invoke<StoredTrajectoryEvalReport>('run_stored_trajectory_smoke_eval_cmd', { limit });

export const runDeveloperEvalSmokeWorkflow = (trajectoryLimit?: number) =>
  invoke<DeveloperEvalSmokeReport>('run_developer_eval_smoke_workflow_cmd', { trajectoryLimit });

export const runDeveloperEvalNightlyWorkflow = () =>
  invoke<DeveloperEvalSmokeReport>('run_developer_eval_nightly_workflow_cmd');

export const runAgentQualityEval = () =>
  invoke<QualityEvalReport>('run_agent_quality_eval_cmd');

// ── Knowledge Compilation ───────────────────────────────────────────

import type {
  CompileResult,
  CompileStats,
  KnowledgeGraph,
  KnowledgeGraphFilters,
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

export const getKnowledgeGraph = (filters?: KnowledgeGraphFilters) =>
  invoke<KnowledgeGraph>('get_knowledge_graph_cmd', {
    limit: filters?.limit ?? 80,
    sourceId: filters?.sourceId ?? null,
    pathPrefix: filters?.pathPrefix ?? null,
    entityTypes: filters?.entityTypes ?? [],
    relationTypes: filters?.relationTypes ?? [],
    minStrength: filters?.minStrength ?? null,
  });

export const runKnowledgeHealthCheck = (staleDays?: number) =>
  invoke<HealthReport>('run_knowledge_health_check_cmd', { staleDays: staleDays ?? 90 });

export const compileAfterScan = (limit?: number) =>
  invoke<CompileResult[]>('compile_after_scan_cmd', { limit: limit ?? 10 });

// ── Dreaming / Insights ─────────────────────────────────────────────

export const startDream = (input?: StartDreamInput) =>
  invoke<DreamRun>('start_dream_cmd', { input: input ?? { triggerKind: 'manual' } });

export const listDreamRuns = (limit?: number) =>
  invoke<DreamRun[]>('list_dream_runs_cmd', { limit: limit ?? 20 });

export const listDreamRunEvents = (runId: string) =>
  invoke<DreamRunEvent[]>('list_dream_run_events_cmd', { runId });

export const listDreamArtifacts = (filters?: DreamArtifactFilters) =>
  invoke<DreamArtifact[]>('list_dream_artifacts_cmd', {
    status: filters?.status ?? null,
    kind: filters?.kind ?? null,
    limit: filters?.limit ?? 50,
  });

export const applyDreamArtifact = (artifactId: string) =>
  invoke<DreamArtifact>('apply_dream_artifact_cmd', { artifactId });

export const updateDreamArtifact = (artifactId: string, input: UpdateDreamArtifactInput) =>
  invoke<DreamArtifact>('update_dream_artifact_cmd', { artifactId, input });

export const rejectDreamArtifact = (artifactId: string) =>
  invoke<DreamArtifact>('reject_dream_artifact_cmd', { artifactId });

export const undoDreamArtifact = (artifactId: string) =>
  invoke<DreamArtifact>('undo_dream_artifact_cmd', { artifactId });

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
import type { ApprovalDecisionValue, ToolPermissionPolicyList } from '../types';

export const approveToolCall = (requestId: string, decision: ApprovalDecisionValue) =>
  invoke<void>('approve_tool_call_cmd', { requestId, decision });

export const listToolPermissionPolicies = () =>
  invoke<ToolPermissionPolicyList>('list_tool_permission_policies_cmd');

export const deleteToolPermissionPolicy = (
  scope: 'session' | 'forever',
  permissionKey: string,
) => invoke<void>('delete_tool_permission_policy_cmd', { scope, permissionKey });

export const clearToolPermissionPolicies = () =>
  invoke<void>('clear_tool_permission_policies_cmd');
