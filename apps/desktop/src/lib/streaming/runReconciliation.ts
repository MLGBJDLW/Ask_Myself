import type {
  AgentRunEvent,
  AgentTaskRun,
  AgentTaskRunEvent,
  Conversation,
  ConversationMessage,
  ConversationTurn,
} from '../../types/conversation';

const DEFAULT_QUERY_TIMEOUT_MS = 10_000;
const MISSING_RUN_CONFIRMATIONS = 3;
const MAX_GAP_RECOVERY_ATTEMPTS = 4;

export interface DurableRunReconciliationPort {
  listTaskRuns(conversationId: string): Promise<AgentTaskRun[]>;
  listRunEvents(runId: string): Promise<AgentRunEvent[]>;
  listTaskEvents(runId: string): Promise<AgentTaskRunEvent[]>;
  loadConversation(
    conversationId: string,
  ): Promise<[Conversation, ConversationMessage[]]>;
  listTurns(conversationId: string): Promise<ConversationTurn[]>;
}

export interface DurableRunSnapshot {
  taskRun: AgentTaskRun;
  runEvents: AgentRunEvent[];
  taskEvents: AgentTaskRunEvent[];
}

interface ReconciliationRequestBase {
  conversationId: string;
  taskRuns?: AgentTaskRun[];
  isCurrent?: () => boolean;
}

export interface HydrationReconciliationRequest extends ReconciliationRequestBase {
  reason: 'hydration';
}

export interface WatchdogReconciliationRequest extends ReconciliationRequestBase {
  reason: 'watchdog';
  expectedRunId?: string;
  expectedTurnId?: string;
  missingRunConfirmations: number;
}

export type DurableRunReconciliationRequest =
  | HydrationReconciliationRequest
  | WatchdogReconciliationRequest;

export type DurableRunReconciliationOutcome =
  | { kind: 'idle' }
  | { kind: 'stale' }
  | { kind: 'unavailable'; error: string }
  | { kind: 'missing'; confirmations: number; exhausted: boolean }
  | { kind: 'active'; snapshot: DurableRunSnapshot }
  | { kind: 'suspended'; snapshot: DurableRunSnapshot }
  | { kind: 'completed'; snapshot: DurableRunSnapshot; finalMessage: ConversationMessage }
  | {
    kind: 'terminal';
    snapshot: DurableRunSnapshot;
    status: 'cancelled' | 'failed' | 'timed_out';
  }
  | { kind: 'pending'; snapshot: DurableRunSnapshot; reason: 'finalMessage' | 'status' };

export interface GapRecoveryRequest {
  runId: string;
  isCurrent: () => boolean;
  /** Apply the canonical replay and return true while a sequence gap remains. */
  accept(events: AgentRunEvent[]): boolean;
}

export type GapRecoveryOutcome =
  | { kind: 'recovered' }
  | { kind: 'stale' }
  | { kind: 'exhausted' };

export interface DurableRunReconcilerOptions {
  queryTimeoutMs?: number;
  delay?: (delayMs: number) => Promise<void>;
}

export function taskRunIsActive(taskRun: AgentTaskRun): boolean {
  return ['queued', 'running', 'waiting_approval', 'cancelling']
    .includes(taskRun.status.toLowerCase());
}

function taskRunIsSuspended(taskRun: AgentTaskRun): boolean {
  const status = taskRun.status.toLowerCase();
  return taskRun.phase === 'awaiting_user_input' || status === 'paused';
}

function newestFirst(taskRuns: AgentTaskRun[]): AgentTaskRun[] {
  return [...taskRuns].sort((left, right) =>
    Date.parse(right.updatedAt) - Date.parse(left.updatedAt));
}

function finalAssistantMessageForTaskRun(
  taskRun: AgentTaskRun,
  turns: ConversationTurn[],
  messages: ConversationMessage[],
): ConversationMessage | null {
  const turn = turns.find(candidate => candidate.id === taskRun.turnId);
  if (turn?.assistantMessageId) {
    const mapped = messages.find(message => message.id === turn.assistantMessageId);
    if (mapped?.role === 'assistant' && mapped.content.trim()) return mapped;
  }

  const taskUserIndex = messages.findIndex(message => message.id === taskRun.userMessageId);
  if (taskUserIndex < 0) return null;
  for (let index = taskUserIndex + 1; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role === 'user') break;
    if (message.role === 'assistant' && message.content.trim()) return message;
  }
  return null;
}

function defaultDelay(delayMs: number): Promise<void> {
  return new Promise(resolve => globalThis.setTimeout(resolve, delayMs));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class DurableRunReconciler {
  private readonly queryTimeoutMs: number;
  private readonly delay: (delayMs: number) => Promise<void>;

  constructor(
    private readonly port: DurableRunReconciliationPort,
    options: DurableRunReconcilerOptions = {},
  ) {
    this.queryTimeoutMs = options.queryTimeoutMs ?? DEFAULT_QUERY_TIMEOUT_MS;
    this.delay = options.delay ?? defaultDelay;
  }

  async reconcile(
    request: DurableRunReconciliationRequest,
  ): Promise<DurableRunReconciliationOutcome> {
    const isCurrent = request.isCurrent ?? (() => true);
    if (!isCurrent()) return { kind: 'stale' };

    let taskRuns: AgentTaskRun[];
    try {
      taskRuns = request.taskRuns ?? await this.withTimeout(
        this.port.listTaskRuns(request.conversationId),
        'Task-run recovery query',
      );
    } catch (error) {
      return isCurrent()
        ? { kind: 'unavailable', error: errorMessage(error) }
        : { kind: 'stale' };
    }
    if (!isCurrent()) return { kind: 'stale' };

    const candidates = newestFirst(taskRuns);
    const taskRun = request.reason === 'hydration'
      ? candidates.find(taskRunIsActive)
      : this.selectWatchdogRun(candidates, request);
    if (!taskRun) {
      if (request.reason === 'hydration') return { kind: 'idle' };
      const confirmations = request.expectedRunId
        ? request.missingRunConfirmations + 1
        : 0;
      return {
        kind: 'missing',
        confirmations,
        exhausted: Boolean(request.expectedRunId && confirmations >= MISSING_RUN_CONFIRMATIONS),
      };
    }

    const runEventsQuery = this.port.listRunEvents(taskRun.id);
    const taskEventsQuery = this.port.listTaskEvents(taskRun.id);
    let runEvents: AgentRunEvent[];
    let taskEvents: AgentTaskRunEvent[];
    try {
      [runEvents, taskEvents] = await this.withTimeout(
        Promise.all(request.reason === 'hydration'
          ? [
            runEventsQuery.catch((): AgentRunEvent[] => []),
            taskEventsQuery.catch((): AgentTaskRunEvent[] => []),
          ]
          : [runEventsQuery, taskEventsQuery]),
        'Durable-event recovery query',
      );
    } catch (error) {
      return isCurrent()
        ? { kind: 'unavailable', error: errorMessage(error) }
        : { kind: 'stale' };
    }
    if (!isCurrent()) return { kind: 'stale' };
    const snapshot: DurableRunSnapshot = {
      taskRun,
      runEvents: [...runEvents].sort((left, right) => left.eventSeq - right.eventSeq),
      taskEvents: taskEvents.slice(-256),
    };

    if (request.reason === 'hydration' || taskRunIsActive(taskRun) && !taskRunIsSuspended(taskRun)) {
      return { kind: 'active', snapshot };
    }
    if (taskRunIsSuspended(taskRun)) return { kind: 'suspended', snapshot };

    const status = taskRun.status.toLowerCase();
    if (status === 'completed') {
      let messages: ConversationMessage[];
      let turns: ConversationTurn[];
      try {
        [[, messages], turns] = await this.withTimeout(
          Promise.all([
            this.port.loadConversation(request.conversationId),
            this.port.listTurns(request.conversationId),
          ]),
          'Final-answer recovery query',
        );
      } catch (error) {
        return isCurrent()
          ? { kind: 'unavailable', error: errorMessage(error) }
          : { kind: 'stale' };
      }
      if (!isCurrent()) return { kind: 'stale' };
      const finalMessage = finalAssistantMessageForTaskRun(taskRun, turns, messages);
      return finalMessage
        ? { kind: 'completed', snapshot, finalMessage }
        : { kind: 'pending', snapshot, reason: 'finalMessage' };
    }
    if (status === 'cancelled' || status === 'failed' || status === 'timed_out') {
      return { kind: 'terminal', snapshot, status };
    }
    return { kind: 'pending', snapshot, reason: 'status' };
  }

  async recoverGap(request: GapRecoveryRequest): Promise<GapRecoveryOutcome> {
    for (let attempt = 0; attempt < MAX_GAP_RECOVERY_ATTEMPTS; attempt += 1) {
      if (!request.isCurrent()) return { kind: 'stale' };
      try {
        const events = await this.withTimeout(
          this.port.listRunEvents(request.runId),
          'Settled run-event gap recovery query',
        );
        if (!request.isCurrent()) return { kind: 'stale' };
        const gapRemains = request.accept(
          [...events].sort((left, right) => left.eventSeq - right.eventSeq),
        );
        if (!gapRemains) return { kind: 'recovered' };
      } catch {
        if (!request.isCurrent()) return { kind: 'stale' };
      }

      if (attempt === MAX_GAP_RECOVERY_ATTEMPTS - 1) {
        return { kind: 'exhausted' };
      }
      await this.delay(Math.min(250 * (2 ** attempt), 2_000));
    }
    return { kind: 'exhausted' };
  }

  private selectWatchdogRun(
    candidates: AgentTaskRun[],
    request: WatchdogReconciliationRequest,
  ): AgentTaskRun | undefined {
    if (request.expectedRunId || request.expectedTurnId) {
      return candidates.find(run =>
        run.id === request.expectedRunId || run.turnId === request.expectedTurnId);
    }
    return candidates.find(taskRunIsActive) ?? candidates[0];
  }

  private async withTimeout<T>(query: Promise<T>, label: string): Promise<T> {
    let timeoutId: ReturnType<typeof setTimeout> | null = null;
    try {
      return await Promise.race([
        query,
        new Promise<T>((_resolve, reject) => {
          timeoutId = globalThis.setTimeout(() => {
            reject(new Error(`${label} exceeded ${this.queryTimeoutMs}ms`));
          }, this.queryTimeoutMs);
        }),
      ]);
    } finally {
      if (timeoutId !== null) globalThis.clearTimeout(timeoutId);
    }
  }
}
