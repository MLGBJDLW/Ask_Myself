import { invoke } from '@tauri-apps/api/core';

import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
} from '../../types/conversation';

/** Narrow IPC seam used by the watchdog without importing the full desktop API catalog. */
export const getRecoveryTaskRuns = (conversationId: string) =>
  invoke<AgentTaskRun[]>('get_agent_task_runs_cmd', { conversationId });

export const getRecoveryRunEvents = (runId: string) =>
  invoke<AgentRunEvent[]>('get_agent_run_events_cmd', { runId });

export const getRecoveryTaskEvents = (runId: string) =>
  invoke<AgentTaskRunEvent[]>('get_agent_task_run_events_cmd', { runId });
