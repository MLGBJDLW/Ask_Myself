import { invoke } from '@tauri-apps/api/core';

export const recordAgentFrontendPaint = (
  conversationId: string,
  runId: string,
  turnId: string,
  elapsedMs: number,
) => invoke<void>('record_agent_frontend_paint_cmd', {
  conversationId,
  runId,
  turnId,
  elapsedMs: Math.max(0, Math.round(elapsedMs)),
});
