import { invoke } from "@tauri-apps/api/core";
import type {
  Source,
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
  ConversationStats,
  ConversationSearchResult,
  ImageAttachment,
  FileAttachment,
  Checkpoint,
  UserMemory,
} from "../types/conversation";
import type {
  McpServer,
  SaveMcpServerInput,
  McpToolInfo,
  Skill,
  SaveSkillInput,
} from "../types/extensions";
import type { TraceSummary, AgentTrace } from "../types/trace";

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

// ── Sources (extra) ─────────────────────────────────────────────────────

export const getSource = (sourceId: string) =>
  invoke<Source>('get_source', { sourceId });

export const updateSource = (
  sourceId: string,
  includeGlobs: string[],
  excludeGlobs: string[],
  watchEnabled: boolean,
) => invoke<void>('update_source', { sourceId, includeGlobs, excludeGlobs, watchEnabled });

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

// ── File ────────────────────────────────────────────────────────────────

export const openFileInDefaultApp = (path: string) =>
  invoke<void>('open_file_in_default_app', { path });

export const showInFileExplorer = (path: string) =>
  invoke<void>('show_in_file_explorer', { path });

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

// ── Conversations ───────────────────────────────────────────────────────

export const createConversation = (provider: string, model: string, systemPrompt?: string) =>
  invoke<Conversation>('create_conversation_cmd', { provider, model, systemPrompt });

export const createConversationWithContext = (
  provider: string,
  model: string,
  systemPrompt?: string,
  collectionContext?: Conversation['collectionContext'],
) => invoke<Conversation>('create_conversation_cmd', { provider, model, systemPrompt, collectionContext });

export const listConversations = () => invoke<Conversation[]>('list_conversations_cmd');

export const getConversation = (id: string) =>
  invoke<[Conversation, ConversationMessage[]]>('get_conversation_cmd', { id });

export const getConversationTurns = (conversationId: string) =>
  invoke<ConversationTurn[]>('get_conversation_turns_cmd', { conversationId });

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

// ── Agent Chat ──────────────────────────────────────────────────────────

export const agentChat = (conversationId: string, message: string, attachments?: ImageAttachment[]) =>
  invoke<void>('agent_chat_cmd', { conversationId, message, attachments: attachments ?? null });

export const agentStop = (conversationId: string) =>
  invoke<void>('agent_stop_cmd', { conversationId });

export const getModelContextWindow = (model: string) =>
  invoke<number>('get_model_context_window', { model });

// ── Image Attachment ────────────────────────────────────────────────────

export const prepareImageAttachment = (path: string) =>
  invoke<ImageAttachment>('prepare_image_attachment', { path });

export const prepareFileAttachment = (path: string) =>
  invoke<FileAttachment>('prepare_file_attachment', { path });

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

export const deleteCheckpoint = (checkpointId: string) =>
  invoke<void>('delete_checkpoint_cmd', { checkpointId });

// ── User Memory ────────────────────────────────────────────────────────

export const listUserMemories = () =>
  invoke<UserMemory[]>('list_user_memories_cmd');

export const createUserMemory = (content: string) =>
  invoke<UserMemory>('create_user_memory_cmd', { content });

export const updateUserMemory = (id: string, content: string) =>
  invoke<UserMemory>('update_user_memory_cmd', { id, content });

export const deleteUserMemory = (id: string) =>
  invoke<void>('delete_user_memory_cmd', { id });

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
