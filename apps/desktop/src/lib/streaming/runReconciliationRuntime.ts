import { invoke } from '@tauri-apps/api/core';

import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
  Conversation,
  ConversationMessage,
  ConversationTurn,
} from '../../types/conversation';
import {
  DurableRunReconciler,
  type DurableRunReconciliationPort,
} from './runReconciliation';

const tauriDurableRunReconciliationPort: DurableRunReconciliationPort = {
  listTaskRuns: conversationId =>
    invoke<AgentTaskRun[]>('get_agent_task_runs_cmd', { conversationId }),
  listRunEvents: runId =>
    invoke<AgentRunEvent[]>('get_agent_run_events_cmd', { runId }),
  listTaskEvents: runId =>
    invoke<AgentTaskRunEvent[]>('get_agent_task_run_events_cmd', { runId }),
  loadConversation: conversationId =>
    invoke<[Conversation, ConversationMessage[]]>('get_conversation_cmd', { id: conversationId }),
  listTurns: conversationId =>
    invoke<ConversationTurn[]>('get_conversation_turns_cmd', { conversationId }),
};

export const durableRunReconciler = new DurableRunReconciler(
  tauriDurableRunReconciliationPort,
);
