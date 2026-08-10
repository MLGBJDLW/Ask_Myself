import type {
  AgentFrontendEvent,
  AgentRunDisplayKind,
  AgentRunEventImportance,
  AgentRunEventKind,
  AgentRunEventPersistence,
  AgentRunEventVisibility,
  AgentRunPhase,
} from '../../types/conversation';

const RUN_EVENT_KINDS = new Set<AgentRunEventKind>([
  'outputDelta',
  'streamReset',
  'thinking',
  'status',
  'planUpdated',
  'toolPreparing',
  'toolStarted',
  'toolProgress',
  'toolCompleted',
  'approvalRequested',
  'approvalResolved',
  'recoveryAttempt',
  'usageUpdated',
  'autoCompacted',
  'done',
  'error',
]);
const RUN_PHASES = new Set<AgentRunPhase>([
  'routing',
  'planning',
  'responding',
  'tooling',
  'approval',
  'awaiting_user_input',
  'paused',
  'compacting',
  'accounting',
  'done',
]);
const VISIBILITIES = new Set<AgentRunEventVisibility>(['user', 'developer', 'internal']);
const PERSISTENCE = new Set<AgentRunEventPersistence>(['durable', 'ephemeral']);
const DISPLAY_KINDS = new Set<AgentRunDisplayKind>([
  'output',
  'reasoning',
  'status',
  'plan',
  'tool',
  'approval',
  'recovery',
  'steering',
  'usage',
  'compaction',
  'completion',
  'error',
]);
const IMPORTANCE = new Set<AgentRunEventImportance>(['low', 'normal', 'high']);
const RUN_EVENT_KEYS = new Set([
  'version',
  'runId',
  'turnId',
  'eventSeq',
  'kind',
  'phase',
  'visibility',
  'persistence',
  'displayKind',
  'importance',
  'label',
  'status',
  'payload',
  'createdAt',
]);

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

export function parseAgentFrontendEvent(value: unknown): AgentFrontendEvent | null {
  const envelope = asRecord(value);
  if (!envelope) return null;
  if (Object.keys(envelope).some(key => key !== 'conversationId' && key !== 'runEvent')) {
    return null;
  }
  if (typeof envelope.conversationId !== 'string' || !envelope.conversationId.trim()) return null;

  const runEvent = asRecord(envelope.runEvent);
  if (!runEvent) return null;
  if (Object.keys(runEvent).some(key => !RUN_EVENT_KEYS.has(key))) return null;
  if (runEvent.version !== 2) return null;
  if (typeof runEvent.runId !== 'string' || !runEvent.runId.trim()) return null;
  if (typeof runEvent.turnId !== 'string' || !runEvent.turnId.trim()) return null;
  if (!Number.isInteger(runEvent.eventSeq) || Number(runEvent.eventSeq) <= 0) return null;
  if (!RUN_EVENT_KINDS.has(runEvent.kind as AgentRunEventKind)) return null;
  if (!RUN_PHASES.has(runEvent.phase as AgentRunPhase)) return null;
  if (typeof runEvent.label !== 'string') return null;
  if (runEvent.status !== undefined && runEvent.status !== null && typeof runEvent.status !== 'string') {
    return null;
  }
  if (!asRecord(runEvent.payload)) return null;
  if (!VISIBILITIES.has(runEvent.visibility as AgentRunEventVisibility)) return null;
  if (!PERSISTENCE.has(runEvent.persistence as AgentRunEventPersistence)) return null;
  if (!DISPLAY_KINDS.has(runEvent.displayKind as AgentRunDisplayKind)) return null;
  if (!IMPORTANCE.has(runEvent.importance as AgentRunEventImportance)) return null;
  if (
    runEvent.createdAt !== undefined
    && runEvent.createdAt !== null
    && typeof runEvent.createdAt !== 'string'
  ) return null;

  return envelope as unknown as AgentFrontendEvent;
}
