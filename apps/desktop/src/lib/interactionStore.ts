import type { InteractionAnswers, InteractionDraft, InteractionRequest } from '../types/conversation';

const DRAFT_STORAGE_KEY = 'nexa.interaction-drafts.v1';

type Listener = () => void;

export interface InteractionStoreState {
  requestsById: Readonly<Record<string, InteractionRequest>>;
  draftsById: Readonly<Record<string, InteractionDraft>>;
  hydratedConversationIds: Readonly<Record<string, true>>;
  hydratedAll: boolean;
}

export interface InteractionDraftStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

const ACTIVE_STATUSES = new Set(['pending', 'presented', 'partially_answered']);

function defaultStorage(): InteractionDraftStorage | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function isStringArrayRecord(value: unknown): value is InteractionAnswers {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  return Object.values(value).every((answers) => (
    Array.isArray(answers) && answers.every((answer) => typeof answer === 'string')
  ));
}

function parseDrafts(storage: InteractionDraftStorage | null): Record<string, InteractionDraft> {
  if (!storage) return {};
  try {
    const parsed = JSON.parse(storage.getItem(DRAFT_STORAGE_KEY) ?? '{}') as unknown;
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    const drafts: Record<string, InteractionDraft> = {};
    for (const [interactionId, value] of Object.entries(parsed)) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) continue;
      const draft = value as Partial<InteractionDraft>;
      if (
        draft.schemaVersion !== 1
        || draft.interactionId !== interactionId
        || !isStringArrayRecord(draft.answers)
        || typeof draft.currentQuestionIndex !== 'number'
        || !Number.isInteger(draft.currentQuestionIndex)
        || draft.currentQuestionIndex < 0
        || typeof draft.updatedAt !== 'string'
      ) {
        continue;
      }
      drafts[interactionId] = {
        schemaVersion: 1,
        interactionId,
        answers: draft.answers,
        currentQuestionIndex: draft.currentQuestionIndex,
        updatedAt: draft.updatedAt,
      };
    }
    return drafts;
  } catch {
    return {};
  }
}

export class InteractionStore {
  private readonly storage: InteractionDraftStorage | null;
  private readonly listeners = new Set<Listener>();
  private state: InteractionStoreState;

  constructor(storage: InteractionDraftStorage | null = defaultStorage()) {
    this.storage = storage;
    this.state = {
      requestsById: {},
      draftsById: parseDrafts(storage),
      hydratedConversationIds: {},
      hydratedAll: false,
    };
  }

  getState = (): InteractionStoreState => this.state;

  subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  replaceRequests(conversationId: string | null, requests: InteractionRequest[]): void {
    const requestsById = { ...this.state.requestsById };
    if (conversationId) {
      for (const [id, request] of Object.entries(requestsById)) {
        if (request.conversationId === conversationId) delete requestsById[id];
      }
    } else {
      for (const id of Object.keys(requestsById)) delete requestsById[id];
    }
    for (const request of requests) requestsById[request.interactionId] = request;
    this.state = {
      ...this.state,
      requestsById,
      hydratedConversationIds: conversationId
        ? { ...this.state.hydratedConversationIds, [conversationId]: true }
        : this.state.hydratedConversationIds,
      hydratedAll: conversationId ? this.state.hydratedAll : true,
    };
    this.removeNonPersistableDrafts(requests);
    this.notify();
  }

  upsertRequest(request: InteractionRequest): void {
    this.state = {
      ...this.state,
      requestsById: {
        ...this.state.requestsById,
        [request.interactionId]: request,
      },
    };
    this.removeNonPersistableDrafts([request]);
    this.notify();
  }

  setDraft(
    interactionId: string,
    answers: InteractionAnswers,
    currentQuestionIndex: number,
  ): InteractionDraft {
    const request = this.state.requestsById[interactionId];
    if (request?.kind === 'credential_request') {
      throw new Error('Credential interactions cannot persist inline answers');
    }
    if (request && !ACTIVE_STATUSES.has(request.status)) {
      throw new Error(`Cannot edit interaction ${interactionId} in status ${request.status}`);
    }
    const draft: InteractionDraft = {
      schemaVersion: 1,
      interactionId,
      answers: Object.fromEntries(
        Object.entries(answers).map(([id, values]) => [id, [...values]]),
      ),
      currentQuestionIndex: Math.max(0, Math.trunc(currentQuestionIndex)),
      updatedAt: new Date().toISOString(),
    };
    this.state = {
      ...this.state,
      draftsById: { ...this.state.draftsById, [interactionId]: draft },
    };
    this.persistDrafts();
    this.notify();
    return draft;
  }

  clearDraft(interactionId: string): void {
    if (!this.state.draftsById[interactionId]) return;
    const draftsById = { ...this.state.draftsById };
    delete draftsById[interactionId];
    this.state = { ...this.state, draftsById };
    this.persistDrafts();
    this.notify();
  }

  queue(conversationId?: string): InteractionRequest[] {
    return Object.values(this.state.requestsById)
      .filter((request) => (
        ACTIVE_STATUSES.has(request.status)
        && (!conversationId || request.conversationId === conversationId)
      ))
      .sort((left, right) => (
        right.riskPriority - left.riskPriority
        || left.queueSequence - right.queueSequence
        || left.interactionId.localeCompare(right.interactionId)
      ));
  }

  private removeNonPersistableDrafts(requests: InteractionRequest[]): void {
    const resolvedIds = requests
      .filter((request) => (
        !ACTIVE_STATUSES.has(request.status) || request.kind === 'credential_request'
      ))
      .map((request) => request.interactionId)
      .filter((id) => Boolean(this.state.draftsById[id]));
    if (resolvedIds.length === 0) return;
    const draftsById = { ...this.state.draftsById };
    for (const id of resolvedIds) delete draftsById[id];
    this.state = { ...this.state, draftsById };
    this.persistDrafts();
  }

  private persistDrafts(): void {
    if (!this.storage) return;
    try {
      if (Object.keys(this.state.draftsById).length === 0) {
        this.storage.removeItem(DRAFT_STORAGE_KEY);
      } else {
        this.storage.setItem(DRAFT_STORAGE_KEY, JSON.stringify(this.state.draftsById));
      }
    } catch {
      // Draft persistence is best-effort; the in-memory draft remains usable.
    }
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

export const interactionStore = new InteractionStore();
