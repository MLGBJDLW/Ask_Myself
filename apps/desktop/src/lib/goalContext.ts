import type { ConversationMessage, ArtifactPayload } from "../types/conversation";

export type GoalLifecycleStatus =
  | "active"
  | "paused"
  | "complete"
  | "cleared";

export interface ActiveGoalContext {
  objective: string;
  status: Extract<GoalLifecycleStatus, "active" | "paused">;
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
  if (TERMINAL_GOAL_STATUSES.has(normalized)) {
    return normalized === "cleared" || normalized === "clear" ? "cleared" : "complete";
  }
  return "active";
}

export function getActiveGoalContext(messages: ConversationMessage[]): ActiveGoalContext | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "user") continue;

    const artifacts = asRecord(message.artifacts);
    if (!artifacts) continue;

    const kind = typeof artifacts.kind === "string" ? artifacts.kind : null;
    if (!kind || !GOAL_ARTIFACT_KINDS.has(kind)) continue;

    const status = artifactStatus(artifacts);
    if (status === "complete" || status === "cleared") {
      return null;
    }

    const objective = artifactObjective(artifacts, message.content);
    if (!objective) return null;

    return {
      objective,
      status,
      sourceMessageId: message.id,
      createdAt: message.createdAt,
    };
  }
  return null;
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
