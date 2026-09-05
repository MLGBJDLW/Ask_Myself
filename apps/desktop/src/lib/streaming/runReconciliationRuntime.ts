import { invoke } from '@tauri-apps/api/core';

import type {
  AgentRunEventPage,
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
  listRunEventPage: (runId, afterEventSeq, durableHighWater) =>
    invoke<AgentRunEventPage>('get_agent_run_event_page_cmd', {
      runId,
      afterEventSeq,
      durableHighWater,
    }),
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
