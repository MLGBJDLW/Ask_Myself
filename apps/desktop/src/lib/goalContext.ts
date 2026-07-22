import type { ConversationMessage, ArtifactPayload } from "../types/conversation";
import type { ToolCallEvent } from "./streaming/protocol";

export type GoalLifecycleStatus =
  | "active"
  | "blocked"
  | "paused"
  | "complete"
  | "cleared";

export interface ActiveGoalContext {
  objective: string;
  status: Extract<GoalLifecycleStatus, "active" | "blocked" | "paused">;
  sourceMessageId: string;
  createdAt: string;
}

const GOAL_ARTIFACT_KINDS = new Set(["goal", "agentGoal"]);
const TERMINAL_GOAL_STATUSES = new Set(["complete", "completed", "clear", "cleared", "cancelled", "canceled"]);

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function artifactObjective(artifacts: Record<string, unknown>, fallback: string): string {
  const objective = artifacts.objective;
  if (typeof objective === "string" && objective.trim().length > 0) {
    return objective.trim();
  }
  return fallback.trim();
}

function artifactStatus(artifacts: Record<string, unknown>): GoalLifecycleStatus {
  const raw = artifacts.status;
  if (typeof raw !== "string") return "active";
  const normalized = raw.trim().toLowerCase();
  if (normalized === "paused") return "paused";
  if (normalized === "blocked") return "blocked";
  if (TERMINAL_GOAL_STATUSES.has(normalized)) {
    return normalized === "cleared" || normalized === "clear" ? "cleared" : "complete";
  }
  return "active";
}

function findGoalArtifact(value: unknown, depth = 0): Record<string, unknown> | null {
  if (depth > 5) return null;
  const record = asRecord(value);
  if (record) {
    const kind = typeof record.kind === "string" ? record.kind : null;
    if (kind && GOAL_ARTIFACT_KINDS.has(kind)) return record;
    for (const key of ["artifacts", "goal", "data", "toolOutput"]) {
      if (!(key in record)) continue;
      const nested = findGoalArtifact(record[key], depth + 1);
      if (nested) return nested;
    }
  }
  if (Array.isArray(value)) {
    for (let index = value.length - 1; index >= 0; index -= 1) {
      const nested = findGoalArtifact(value[index], depth + 1);
      if (nested) return nested;
    }
  }
  return null;
}

export function getActiveGoalContext(
  messages: ConversationMessage[],
  toolCalls: ToolCallEvent[] = [],
): ActiveGoalContext | null {
  let current: ActiveGoalContext | null = null;
  const applyArtifact = (
    value: unknown,
    fallbackObjective: string,
    sourceMessageId: string,
    createdAt: string,
  ) => {
    const artifacts = findGoalArtifact(value);
    if (!artifacts) return;
    const status = artifactStatus(artifacts);
    if (status === "complete" || status === "cleared") {
      current = null;
      return;
    }
    const objective = artifactObjective(artifacts, fallbackObjective);
    if (!objective) return;
    current = { objective, status, sourceMessageId, createdAt };
  };

  for (const message of messages) {
    applyArtifact(message.artifacts, message.content, message.id, message.createdAt);
  }
  for (const call of toolCalls) {
    applyArtifact(call.artifacts, "", call.callId, "");
  }
  return current;
}

export function buildGoalContinuationLlmContext(goal: ActiveGoalContext, userMessage: string): string {
  return [
    "Active conversation goal:",
    goal.objective,
    "",
    "Continue treating the user's message as part of this active goal. Keep work oriented toward the goal until it is explicitly replaced, cleared, or completed.",
    "",
    "User message:",
    userMessage,
  ].join("\n");
}

export function mergeGoalContextArtifact(
  artifact: ArtifactPayload | null | undefined,
  goal: ActiveGoalContext,
  llmContextContent: string,
): ArtifactPayload {
  const goalPayload = {
    objective: goal.objective,
    status: goal.status,
    sourceMessageId: goal.sourceMessageId,
  };

  if (artifact && !Array.isArray(artifact) && typeof artifact === "object") {
    return {
      ...artifact,
      activeGoal: goalPayload,
      llmContextContent,
      llmContextVersion: 1,
    };
  }

  return {
    kind: "goalContinuation",
    version: 1,
    activeGoal: goalPayload,
    llmContextContent,
    llmContextVersion: 1,
  };
}
