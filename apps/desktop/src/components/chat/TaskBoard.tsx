import { useMemo } from 'react';
import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  ConversationMessage,
} from '../../types/conversation';
import type { ToolCallEvent } from '../../lib/streaming/protocol';
import type { ActiveGoalContext } from '../../lib/goalContext';
import {
  extractPlanArtifact,
  findLatestPlanArtifact,
  findLatestSubtaskArtifacts,
} from '../../lib/taskArtifacts';
import { PlanProgressPanel } from './TaskPanels';

interface TaskBoardProps {
  messages: ConversationMessage[];
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
  taskEvents?: AgentTaskRunEvent[];
  goal?: ActiveGoalContext | null;
}

export function TaskBoard({
  messages,
  toolCalls,
  taskRun,
  taskEvents = [],
  goal = null,
}: TaskBoardProps) {
  const plan = useMemo(
    () => findLatestUpdatePlanArtifact(messages, toolCalls)
      ?? findLatestPlanArtifact(messages, toolCalls, taskRun?.plan),
    [messages, taskRun?.plan, toolCalls],
  );
  const subtasks = useMemo(
    () => findLatestSubtaskArtifacts(
      messages,
      toolCalls,
      taskRun?.artifacts,
      taskEvents,
    ),
    [messages, taskEvents, taskRun?.artifacts, toolCalls],
  );

  if (!plan && !goal) {
    return null;
  }

  if (!goal && plan?.routeKind === 'DirectResponse') {
    return null;
  }

  return (
    <div
      data-testid="task-board"
      className="pointer-events-none absolute right-3 top-14 z-20 w-[min(22rem,calc(100%-1.5rem))] md:right-4"
    >
      <PlanProgressPanel plan={plan} goal={goal} subtasks={subtasks} />
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
