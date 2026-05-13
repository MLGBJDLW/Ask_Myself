import { useMemo } from 'react';
import type { AgentTaskRun, ConversationMessage } from '../../types/conversation';
import type { ToolCallEvent } from '../../lib/useAgentStream';
import { extractPlanArtifact, findLatestPlanArtifact } from '../../lib/taskArtifacts';
import { PlanProgressPanel } from './TaskPanels';

interface TaskBoardProps {
  messages: ConversationMessage[];
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
}

export function TaskBoard({
  messages,
  toolCalls,
  taskRun,
}: TaskBoardProps) {
  const plan = useMemo(
    () => findLatestUpdatePlanArtifact(messages, toolCalls)
      ?? findLatestPlanArtifact(messages, toolCalls, taskRun?.plan),
    [messages, taskRun?.plan, toolCalls],
  );

  if (!plan) {
    return null;
  }

  if (plan.routeKind === 'DirectResponse') {
    return null;
  }

  return (
    <div
      data-testid="task-board"
      className="shrink-0 border-t border-border/60 bg-surface-0/95 px-3 py-1.5 backdrop-blur"
    >
      <PlanProgressPanel plan={plan} />
    </div>
  );
}

function isUpdatePlanTool(toolName: string | null | undefined) {
  return toolName?.trim().toLowerCase() === 'update_plan';
}

function findLatestUpdatePlanArtifact(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[],
) {
  for (let i = toolCalls.length - 1; i >= 0; i -= 1) {
    const call = toolCalls[i];
    if (!isUpdatePlanTool(call.toolName)) continue;
    const artifact = extractPlanArtifact(call.artifacts);
    if (artifact) return artifact;
  }

  const updatePlanCallIds = new Set<string>();
  for (const message of messages) {
    for (const call of message.toolCalls) {
      if (isUpdatePlanTool(call.name)) {
        updatePlanCallIds.add(call.id);
      }
    }
  }

  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const message = messages[i];
    const hasUpdatePlanCall =
      message.toolCalls.some(call => isUpdatePlanTool(call.name))
      || (message.toolCallId ? updatePlanCallIds.has(message.toolCallId) : false);

    if (!hasUpdatePlanCall) continue;
    const artifact = extractPlanArtifact(message.artifacts);
    if (artifact) return artifact;
  }

  return null;
}
