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
import type {
  AgentConfig,
  SaveAgentConfigInput,
  Conversation,
  ConversationMessage,
  ConversationStats,
  ImageAttachment,
} from "../types/conversation";

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

export const listConversations = () => invoke<Conversation[]>('list_conversations_cmd');

export const getConversation = (id: string) =>
  invoke<[Conversation, ConversationMessage[]]>('get_conversation_cmd', { id });

export const deleteConversation = (id: string) =>
  invoke<void>('delete_conversation_cmd', { id });

export const renameConversation = (id: string, title: string) =>
  invoke<void>('rename_conversation_cmd', { id, title });

export const updateConversationSystemPrompt = (id: string, systemPrompt: string) =>
  invoke<void>('update_conversation_system_prompt_cmd', { id, systemPrompt });

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
