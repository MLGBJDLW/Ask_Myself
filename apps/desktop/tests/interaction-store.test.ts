import { InteractionStore, type InteractionDraftStorage } from '../src/lib/interactionStore';
import type { InteractionKind, InteractionRequest } from '../src/types/conversation';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

class MemoryStorage implements InteractionDraftStorage {
  private values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

function request(
  interactionId: string,
  kind: InteractionKind,
  createdAt: string,
  conversationId = 'conversation-1',
): InteractionRequest {
  const queueSequence = interactionId === 'low-first'
    ? 1
    : interactionId === 'low-second'
      ? 2
      : interactionId === 'high'
        ? 3
        : 4;
  return {
    schemaVersion: 1,
    interactionId,
    conversationId,
    turnId: `turn-${interactionId}`,
    toolCallId: `call-${interactionId}`,
    kind,
    title: interactionId,
    questions: [{
      id: 'scope',
      header: 'Scope',
      question: 'Which scope?',
      type: 'single_choice',
      options: [{ label: 'App' }, { label: 'Repo' }],
    }],
    required: true,
    status: 'pending',
    riskPriority: kind === 'high_risk_confirmation' ? 400 : 100,
    queueSequence,
    createdAt,
    updatedAt: createdAt,
    resumeToken: `token-${interactionId}`,
  };
}

const storage = new MemoryStorage();
const store = new InteractionStore(storage);
const lowFirst = request('low-first', 'user_input', '2026-08-06 01:00:00');
const lowSecond = request('low-second', 'user_input', '2026-08-06 02:00:00');
const high = request('high', 'high_risk_confirmation', '2026-08-06 03:00:00');
store.replaceRequests('conversation-1', [lowSecond, high, lowFirst]);

assertEqual(store.queue()[0].interactionId, 'high', 'risk priority sorts before time');
assertEqual(store.queue()[1].interactionId, 'low-first', 'equal-risk requests remain FIFO');
assertEqual(store.queue()[2].interactionId, 'low-second', 'FIFO retains later request last');

store.setDraft('low-first', { scope: ['Repo'] }, 0);
const remountedStore = new InteractionStore(storage);
assertEqual(
  remountedStore.getState().draftsById['low-first'].answers.scope[0],
  'Repo',
  'draft survives store recreation',
);

remountedStore.replaceRequests('conversation-1', [lowFirst]);
remountedStore.upsertRequest({ ...lowFirst, status: 'submitted' });
assert(
  !remountedStore.getState().draftsById['low-first'],
  'submitted request clears the persisted draft',
);

const otherConversation = request(
  'other',
  'user_input',
  '2026-08-06 04:00:00',
  'conversation-2',
);
remountedStore.replaceRequests('conversation-2', [otherConversation]);
remountedStore.replaceRequests('conversation-1', [lowSecond]);
assert(
  Boolean(remountedStore.getState().requestsById.other),
  'conversation-scoped hydration preserves other conversations',
);

const credential = request(
  'credential',
  'credential_request',
  '2026-08-06 05:00:00',
);
remountedStore.upsertRequest(credential);
let rejectedCredentialDraft = false;
try {
  remountedStore.setDraft('credential', { secret: ['must-not-persist'] }, 0);
} catch {
  rejectedCredentialDraft = true;
}
assert(rejectedCredentialDraft, 'credential requests never persist inline answers');

let rejectedUnknownDraft = false;
try {
  remountedStore.setDraft('not-hydrated', { scope: ['Repo'] }, 0);
} catch {
  rejectedUnknownDraft = true;
}
assert(rejectedUnknownDraft, 'unknown requests cannot write persisted drafts');

const removalStorage = new MemoryStorage();
const removalStore = new InteractionStore(removalStorage);
removalStore.replaceRequests('conversation-1', [lowSecond]);
removalStore.setDraft('low-second', { scope: ['App'] }, 0);
const removalStoreAfterRestart = new InteractionStore(removalStorage);
removalStoreAfterRestart.replaceRequests('conversation-1', []);
assert(
  !removalStoreAfterRestart.getState().draftsById['low-second'],
  'a terminal-filtered request clears its persisted draft after restart',
);
