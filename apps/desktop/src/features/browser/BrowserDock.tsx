import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import { createPortal } from 'react-dom';
import { listen } from '@tauri-apps/api/event';
import { open as openExternal } from '@tauri-apps/plugin-shell';
import {
  ArrowLeft,
  ArrowRight,
  Crosshair,
  ExternalLink,
  Globe2,
  Hand,
  Loader2,
  Maximize2,
  Minimize2,
  MousePointer2,
  Plus,
  RefreshCw,
  Send,
  Square,
  TextCursorInput,
  X,
} from 'lucide-react';
import { toast } from 'sonner';

import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { formatUserError } from '../../lib/userError';
import { OPEN_BROWSER_WORKSPACE_EVENT, type OpenNexaBrowserDetail } from './openNexaBrowser';

export interface BrowserDockStatus {
  tabCount: number;
  state: 'empty' | 'idle' | 'loading' | 'agent' | 'user' | 'error';
}

export type BrowserAgentSelection = api.BrowserPickArtifact | {
  kind: 'text';
  url: string;
  title: string;
  text: string;
};

export interface BrowserAgentArtifact {
  conversationId: string;
  sessionId: string;
  tabId: string;
  selection: BrowserAgentSelection;
}

interface BrowserDockProps {
  open: boolean;
  conversationId?: string;
  agentLabel?: string;
  onOpenChange: (open: boolean) => void;
  onStatusChange?: (status: BrowserDockStatus) => void;
  onSendArtifactToAgent?: (artifact: BrowserAgentArtifact) => void;
}

const MIN_WIDTH = 440;
const MAX_WIDTH = 920;
const DEFAULT_WIDTH = 620;
const WIDTH_STORAGE_KEY = 'nexa-browser-dock-width';
const MAX_BROWSER_TABS_PER_SESSION = 16;

interface BrowserSessionRequestScope {
  conversationId: string;
  conversationGeneration: number;
  requestGeneration: number;
  expectedSessionId?: string;
}

function storedWidth(): number {
  const parsed = Number(localStorage.getItem(WIDTH_STORAGE_KEY));
  return Number.isFinite(parsed) ? Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, parsed)) : DEFAULT_WIDTH;
}

function activeTab(session: api.BrowserSessionInfo | null): api.BrowserTabInfo | null {
  if (!session) return null;
  return session.tabs.find((tab) => tab.id === session.activeTabId) ?? session.tabs[0] ?? null;
}

function ownerType(owner: api.BrowserControlOwner | undefined): 'none' | 'user' | 'agent' {
  return owner?.type ?? 'none';
}

function shortTitle(tab: api.BrowserTabInfo): string {
  if (tab.title.trim()) return tab.title.trim();
  try {
    return new URL(tab.url).hostname;
  } catch {
    return tab.url || 'New tab';
  }
}

export function BrowserDock({
  open,
  conversationId,
  agentLabel,
  onOpenChange,
  onStatusChange,
  onSendArtifactToAgent,
}: BrowserDockProps) {
  const { t } = useTranslation();
  const [storedSession, setSession] = useState<api.BrowserSessionInfo | null>(null);
  const [address, setAddress] = useState('');
  const [busy, setBusy] = useState(false);
  const [fullScreen, setFullScreen] = useState(false);
  const [narrowViewport, setNarrowViewport] = useState(false);
  const [width, setWidth] = useState(storedWidth);
  const [pickMode, setPickMode] = useState<'element' | 'region' | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const latestBoundsRef = useRef<api.BrowserBounds | null>(null);
  const pickTimerRef = useRef<number | null>(null);
  const sessionPromisesRef = useRef(new Map<string, Promise<api.BrowserSessionInfo | null>>());
  const sessionRequestGenerationRef = useRef(0);
  const refreshesRef = useRef(new Map<string, { dirty: boolean; promise: Promise<api.BrowserSessionInfo | null> }>());
  const conversationLifecycleRef = useRef({ conversationId, generation: 0 });
  if (conversationLifecycleRef.current.conversationId !== conversationId) {
    conversationLifecycleRef.current = {
      conversationId,
      generation: conversationLifecycleRef.current.generation + 1,
    };
    sessionRequestGenerationRef.current += 1;
  }
  const conversationIdRef = useRef(conversationId);
  conversationIdRef.current = conversationId;
  const session = storedSession?.conversationId === conversationId ? storedSession : null;
  const currentTab = useMemo(() => activeTab(session), [session]);
  const openRef = useRef(open);
  const sessionRef = useRef(session);
  const sessionIdRef = useRef(session?.id);
  const activeTabIdRef = useRef(currentTab?.id);
  const onOpenChangeRef = useRef(onOpenChange);
  const translateRef = useRef(t);
  openRef.current = open;
  sessionRef.current = session;
  sessionIdRef.current = session?.id;
  activeTabIdRef.current = currentTab?.id;
  onOpenChangeRef.current = onOpenChange;
  translateRef.current = t;
  const artifactScopeGenerationRef = useRef(0);
  const busyGenerationRef = useRef(0);
  const visibilityGenerationRef = useRef(0);
  const visibilityRevisionBySessionRef = useRef(new Map<string, number>());
  const pendingPopupCountBySessionRef = useRef(new Map<string, number>());
  const popupLimitWarnedSessionsRef = useRef(new Set<string>());
  const effectiveFullScreen = fullScreen || narrowViewport;

  useEffect(() => {
    const query = window.matchMedia('(max-width: 959px)');
    const update = (event: MediaQueryListEvent | MediaQueryList) => setNarrowViewport(event.matches);
    update(query);
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);

  const reportError = useCallback((message: string, error: unknown) => {
    const formatted = formatUserError(message, error);
    setLastError(formatted);
    toast.error(formatted);
  }, []);

  const beginSessionRequest = useCallback((
    targetConversationId: string,
    expectedSessionId?: string,
  ): BrowserSessionRequestScope | null => {
    const lifecycle = conversationLifecycleRef.current;
    if (
      conversationIdRef.current !== targetConversationId
      || lifecycle.conversationId !== targetConversationId
    ) return null;
    const requestGeneration = sessionRequestGenerationRef.current + 1;
    sessionRequestGenerationRef.current = requestGeneration;
    return {
      conversationId: targetConversationId,
      conversationGeneration: lifecycle.generation,
      requestGeneration,
      expectedSessionId,
    };
  }, []);

  const sessionScopeOwnsCurrent = useCallback((scope: BrowserSessionRequestScope) => {
    const lifecycle = conversationLifecycleRef.current;
    return conversationIdRef.current === scope.conversationId
      && lifecycle.conversationId === scope.conversationId
      && lifecycle.generation === scope.conversationGeneration
      && (
        scope.expectedSessionId === undefined
        || sessionIdRef.current === scope.expectedSessionId
      );
  }, []);

  const sessionScopeCanCommit = useCallback((scope: BrowserSessionRequestScope) => (
    sessionScopeOwnsCurrent(scope)
    && sessionRequestGenerationRef.current === scope.requestGeneration
  ), [sessionScopeOwnsCurrent]);

  const commitSession = useCallback((
    scope: BrowserSessionRequestScope,
    next: api.BrowserSessionInfo | null,
  ) => {
    if (!sessionScopeCanCommit(scope)) return false;
    if (next?.conversationId !== undefined && next?.conversationId !== scope.conversationId) {
      return false;
    }
    if (scope.expectedSessionId !== undefined && next && next.id !== scope.expectedSessionId) {
      return false;
    }
    if (next) {
      const currentVisibilityRevision = visibilityRevisionBySessionRef.current.get(next.id) ?? 0;
      visibilityRevisionBySessionRef.current.set(next.id, Math.max(
        currentVisibilityRevision,
        next.visibilityRevision,
        next.visibilityRequestRevision ?? 0,
      ));
    }
    sessionRef.current = next;
    sessionIdRef.current = next?.id;
    setSession(next);
    return true;
  }, [sessionScopeCanCommit]);

  const reportScopeError = useCallback((
    scope: BrowserSessionRequestScope,
    message: string,
    error: unknown,
  ) => {
    if (sessionScopeCanCommit(scope)) reportError(message, error);
  }, [reportError, sessionScopeCanCommit]);

  const bounds = useCallback((): api.BrowserBounds | null => {
    const rect = contentRef.current?.getBoundingClientRect();
    if (!rect || rect.width < 1 || rect.height < 1) return null;
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  }, []);

  const nextVisibilityRevision = useCallback((sessionId: string) => {
    const next = (visibilityRevisionBySessionRef.current.get(sessionId) ?? 0) + 1;
    visibilityRevisionBySessionRef.current.set(sessionId, next);
    return next;
  }, []);

  const recordMinimumVisibilityRevision = useCallback((sessionId: string, minimum: unknown) => {
    const parsed = Number(minimum);
    if (!Number.isSafeInteger(parsed) || parsed < 1) return;
    const current = visibilityRevisionBySessionRef.current.get(sessionId) ?? 0;
    visibilityRevisionBySessionRef.current.set(sessionId, Math.max(current, parsed));
  }, []);

  const recoverRequestedVisibility = useCallback((
    scope: BrowserSessionRequestScope,
    next: api.BrowserSessionInfo | null,
  ) => {
    if (
      !sessionScopeCanCommit(scope)
      || !next
      || next.conversationId !== scope.conversationId
      || !next.visibilityRequested
    ) return;
    recordMinimumVisibilityRevision(next.id, next.visibilityRequestRevision);
    onOpenChangeRef.current(true);
  }, [recordMinimumVisibilityRevision, sessionScopeCanCommit]);

  const refresh = useCallback(async () => {
    const targetConversationId = conversationIdRef.current;
    if (!targetConversationId) {
      sessionRef.current = null;
      sessionIdRef.current = undefined;
      setSession(null);
      return null;
    }
    const lifecycleGeneration = conversationLifecycleRef.current.generation;
    const key = `${lifecycleGeneration}:${targetConversationId}`;
    const pending = refreshesRef.current.get(key);
    if (pending) {
      pending.dirty = true;
      return pending.promise;
    }
    const entry = { dirty: false, promise: Promise.resolve<api.BrowserSessionInfo | null>(null) };
    refreshesRef.current.set(key, entry);
    entry.promise = (async () => {
      let latest: api.BrowserSessionInfo | null = null;
      try {
        do {
          entry.dirty = false;
          if (conversationLifecycleRef.current.generation !== lifecycleGeneration) return null;
          const scope = beginSessionRequest(targetConversationId);
          if (!scope) return null;
          const next = await api.activeBrowserSession(targetConversationId);
          if (commitSession(scope, next)) {
            recoverRequestedVisibility(scope, next);
            latest = next;
          }
        } while (entry.dirty);
        return latest;
      } finally {
        refreshesRef.current.delete(key);
      }
    })();
    return entry.promise;
  }, [beginSessionRequest, commitSession, recoverRequestedVisibility]);

  const syncBounds = useCallback(async (
    visible = openRef.current,
    expectedGeneration?: number,
    targetSessionId = sessionIdRef.current,
    targetConversationId = conversationIdRef.current,
  ) => {
    if (!targetSessionId) return;
    if (
      visible
      && (
        (expectedGeneration !== undefined
          && visibilityGenerationRef.current !== expectedGeneration)
        || conversationIdRef.current !== targetConversationId
        || sessionIdRef.current !== targetSessionId
      )
    ) return;
    const nextBounds = bounds() ?? latestBoundsRef.current;
    if (!nextBounds) return;
    latestBoundsRef.current = nextBounds;
    await api.setBrowserBounds(
      targetSessionId,
      nextBounds,
      visible,
      nextVisibilityRevision(targetSessionId),
    );
  }, [bounds, nextVisibilityRevision]);

  const ensureSession = useCallback(async (url?: string) => {
    if (!conversationId) return null;
    const scope = beginSessionRequest(conversationId);
    if (!scope) return null;
    const existingPromise = sessionPromisesRef.current.get(conversationId);
    if (existingPromise) {
      const current = await existingPromise;
      if (!sessionScopeOwnsCurrent(scope)) return null;
      if (current?.conversationId === conversationId && url) {
        await api.openBrowserTab(current.id, url, openRef.current ? bounds() : null);
        if (!sessionScopeOwnsCurrent(scope)) return null;
        const refreshed = await api.activeBrowserSession(conversationId);
        if (commitSession(scope, refreshed)) recoverRequestedVisibility(scope, refreshed);
        return refreshed;
      }
      if (current?.conversationId === conversationId) {
        if (commitSession(scope, current)) recoverRequestedVisibility(scope, current);
        return current;
      }
      sessionPromisesRef.current.delete(conversationId);
    }
    const pending = (async () => {
      let current = sessionRef.current?.conversationId === conversationId
        ? sessionRef.current
        : await api.activeBrowserSession(conversationId);
      if (current?.conversationId !== conversationId) current = null;
      const nextBounds = bounds();
      if (!current) {
        current = await api.createBrowserSession({
          conversationId,
          url: url || 'https://www.google.com',
          openInitialUrlOnReuse: Boolean(url),
          bounds: openRef.current ? nextBounds : null,
        });
      } else if (url) {
        await api.openBrowserTab(current.id, url, openRef.current ? nextBounds : null);
        current = await api.activeBrowserSession(conversationId);
      }
      return current;
    })();
    sessionPromisesRef.current.set(conversationId, pending);
    try {
      const current = await pending;
      if (!sessionScopeOwnsCurrent(scope)) return null;
      if (commitSession(scope, current)) recoverRequestedVisibility(scope, current);
      return current;
    } finally {
      if (sessionPromisesRef.current.get(conversationId) === pending) {
        sessionPromisesRef.current.delete(conversationId);
      }
    }
  }, [
    beginSessionRequest,
    bounds,
    commitSession,
    conversationId,
    recoverRequestedVisibility,
    sessionScopeOwnsCurrent,
  ]);

  useEffect(() => {
    void refresh().catch(() => undefined);
  }, [conversationId, refresh]);

  useEffect(() => {
    if (!open || !conversationId) return;
    const targetConversationId = conversationId;
    void ensureSession()
      .catch((error) => {
        if (conversationIdRef.current === targetConversationId) {
          reportError(t('browser.openFailed'), error);
        }
      });
  }, [conversationId, ensureSession, open, reportError, t]);

  useEffect(() => {
    const scopedSessionId = session?.id;
    const scopedConversationId = session?.conversationId;
    if (!scopedSessionId || !scopedConversationId) return;
    const generation = visibilityGenerationRef.current + 1;
    visibilityGenerationRef.current = generation;
    let pendingFrame: number | null = null;
    const scheduleVisibleBounds = () => {
      if (pendingFrame !== null) window.cancelAnimationFrame(pendingFrame);
      pendingFrame = window.requestAnimationFrame(() => {
        pendingFrame = null;
        if (
          visibilityGenerationRef.current !== generation
          || !open
          || conversationIdRef.current !== session.conversationId
          || sessionIdRef.current !== session.id
        ) return;
        void syncBounds(
          true,
          generation,
          scopedSessionId,
          scopedConversationId,
        ).catch(() => undefined);
      });
    };
    if (!open) {
      void syncBounds(
        false,
        undefined,
        scopedSessionId,
        scopedConversationId,
      ).catch(() => undefined);
      return () => {
        if (visibilityGenerationRef.current === generation) {
          visibilityGenerationRef.current += 1;
        }
      };
    }
    const element = contentRef.current;
    if (!element) return;
    const observer = new ResizeObserver(scheduleVisibleBounds);
    const handleResize = scheduleVisibleBounds;
    observer.observe(element);
    window.addEventListener('resize', handleResize);
    scheduleVisibleBounds();
    return () => {
      if (visibilityGenerationRef.current === generation) {
        visibilityGenerationRef.current += 1;
      }
      if (pendingFrame !== null) window.cancelAnimationFrame(pendingFrame);
      observer.disconnect();
      window.removeEventListener('resize', handleResize);
      void api.setBrowserBounds(
        scopedSessionId,
        latestBoundsRef.current ?? { x: 0, y: 0, width: 1, height: 1 },
        false,
        nextVisibilityRevision(scopedSessionId),
      ).catch(() => undefined);
    };
  }, [effectiveFullScreen, nextVisibilityRevision, open, session?.conversationId, session?.id, syncBounds]);

  useEffect(() => {
    const handler = (event: Event) => {
      if (!conversationId) return;
      const detail = (event as CustomEvent<OpenNexaBrowserDetail>).detail;
      event.preventDefault();
      onOpenChange(true);
      if (detail?.url) {
        const targetConversationId = conversationId;
        void ensureSession(detail.url).catch((error) => {
          if (conversationIdRef.current === targetConversationId) {
            reportError(t('browser.openFailed'), error);
          }
        });
      }
    };
    window.addEventListener(OPEN_BROWSER_WORKSPACE_EVENT, handler);
    return () => window.removeEventListener(OPEN_BROWSER_WORKSPACE_EVENT, handler);
  }, [conversationId, ensureSession, onOpenChange, reportError, t]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<api.BrowserEvent>('browser:event', (event) => {
      if (disposed) return;
      const payload = event.payload.payload;
      const eventConversationId = typeof payload.conversationId === 'string'
        ? payload.conversationId
        : null;
      const eventSessionId = typeof payload.sessionId === 'string'
        ? payload.sessionId
        : '';
      const currentConversationId = conversationIdRef.current;

      if (event.payload.kind === 'sessionCreated' || event.payload.kind === 'workspaceVisibilityRequested') {
        if (!currentConversationId || eventConversationId !== currentConversationId) return;
        if (eventSessionId) {
          recordMinimumVisibilityRevision(eventSessionId, payload.minimumVisibilityRevision);
        }
        if (Boolean(payload.requestVisible)) onOpenChangeRef.current(true);
        void refresh().catch(() => undefined);
        return;
      }

      const currentSession = sessionRef.current;
      if (
        !currentConversationId
        || !currentSession
        || currentSession.conversationId !== currentConversationId
        || !eventSessionId
        || currentSession.id !== eventSessionId
      ) return;

      if (event.payload.kind === 'downloadRequested') {
        toast.warning(translateRef.current('browser.downloadBlocked'));
        return;
      }
      if (event.payload.kind === 'newWindowRequested') {
        const url = typeof payload.url === 'string' ? payload.url : '';
        const sourceTabId = typeof payload.tabId === 'string' ? payload.tabId : '';
        if (
          !openRef.current
          || !url
          || !sourceTabId
          || !currentSession.tabs.some(tab => tab.id === sourceTabId)
        ) return;

        const pendingCount = pendingPopupCountBySessionRef.current.get(eventSessionId) ?? 0;
        if (currentSession.tabs.length + pendingCount >= MAX_BROWSER_TABS_PER_SESSION) {
          if (!popupLimitWarnedSessionsRef.current.has(eventSessionId)) {
            popupLimitWarnedSessionsRef.current.add(eventSessionId);
            toast.warning(translateRef.current('browser.popupBlocked'));
          }
          return;
        }

        const scope = beginSessionRequest(currentConversationId, eventSessionId);
        if (!scope) return;
        pendingPopupCountBySessionRef.current.set(eventSessionId, pendingCount + 1);
        void api.openBrowserPopup(
          eventSessionId,
          sourceTabId,
          url,
          openRef.current ? bounds() : null,
        ).then(async () => {
          if (!sessionScopeOwnsCurrent(scope)) return;
          try {
            await refresh();
          } catch (error) {
            if (sessionScopeOwnsCurrent(scope)) {
              reportError(translateRef.current('browser.popupBlocked'), error);
            }
          }
        }).catch((error) => {
          reportScopeError(scope, translateRef.current('browser.popupBlocked'), error);
        }).finally(() => {
          const remaining = Math.max(
            0,
            (pendingPopupCountBySessionRef.current.get(eventSessionId) ?? 1) - 1,
          );
          if (remaining === 0) pendingPopupCountBySessionRef.current.delete(eventSessionId);
          else pendingPopupCountBySessionRef.current.set(eventSessionId, remaining);
        });
        return;
      }
      void refresh().catch(() => undefined);
    }).then((dispose) => {
      if (disposed) dispose(); else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [
    beginSessionRequest,
    bounds,
    recordMinimumVisibilityRevision,
    refresh,
    reportError,
    reportScopeError,
    sessionScopeOwnsCurrent,
  ]);

  useEffect(() => {
    setAddress(currentTab?.url ?? '');
  }, [currentTab?.id, currentTab?.url]);

  useEffect(() => {
    const control = ownerType(session?.controlOwner);
    const state: BrowserDockStatus['state'] = lastError
      ? 'error'
      : currentTab?.loading
        ? 'loading'
        : control === 'agent'
          ? 'agent'
          : control === 'user'
            ? 'user'
            : session
              ? 'idle'
              : 'empty';
    onStatusChange?.({ tabCount: session?.tabs.length ?? 0, state });
  }, [currentTab?.loading, lastError, onStatusChange, session?.id, session?.tabs.length, session?.controlOwner?.type]);

  useEffect(() => {
    busyGenerationRef.current += 1;
    popupLimitWarnedSessionsRef.current.clear();
    setBusy(false);
    setLastError(null);
  }, [conversationId, session?.id]);

  useEffect(() => {
    artifactScopeGenerationRef.current += 1;
    if (pickTimerRef.current !== null) {
      window.clearInterval(pickTimerRef.current);
      pickTimerRef.current = null;
    }
    setPickMode(null);
    return () => {
      artifactScopeGenerationRef.current += 1;
      if (pickTimerRef.current !== null) {
        window.clearInterval(pickTimerRef.current);
        pickTimerRef.current = null;
      }
    };
  }, [conversationId, currentTab?.id, session?.id]);

  const artifactScopeIsCurrent = useCallback((scope: {
    conversationId: string;
    sessionId: string;
    tabId: string;
    generation: number;
  }) => (
    artifactScopeGenerationRef.current === scope.generation
    && conversationIdRef.current === scope.conversationId
    && sessionIdRef.current === scope.sessionId
    && activeTabIdRef.current === scope.tabId
  ), []);

  const withCurrentTab = useCallback(async (
    operation: (sessionId: string, tabId: string) => Promise<unknown>,
  ) => {
    if (!conversationId || !session || !currentTab) return;
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    const busyGeneration = busyGenerationRef.current + 1;
    busyGenerationRef.current = busyGeneration;
    setBusy(true);
    setLastError(null);
    try {
      await operation(session.id, currentTab.id);
      if (sessionScopeOwnsCurrent(scope)) {
        try {
          await refresh();
        } catch (error) {
          if (sessionScopeOwnsCurrent(scope)) {
            reportError(t('browser.actionFailed'), error);
          }
        }
      }
    } catch (error) {
      reportScopeError(scope, t('browser.actionFailed'), error);
    } finally {
      if (
        busyGenerationRef.current === busyGeneration
        && sessionScopeOwnsCurrent(scope)
      ) setBusy(false);
    }
  }, [
    beginSessionRequest,
    conversationId,
    currentTab,
    refresh,
    reportError,
    reportScopeError,
    session,
    sessionScopeOwnsCurrent,
    t,
  ]);

  const navigate = useCallback(async (event: FormEvent) => {
    event.preventDefault();
    if (!address.trim()) return;
    await withCurrentTab((sessionId, tabId) => api.navigateBrowserTab(sessionId, tabId, address));
  }, [address, withCurrentTab]);

  const addTab = useCallback(async () => {
    const targetConversationId = conversationId;
    if (!targetConversationId || session?.cleanupPending) return;
    try {
      const current = await ensureSession();
      if (!current || conversationIdRef.current !== targetConversationId) return;
      const scope = beginSessionRequest(targetConversationId, current.id);
      if (!scope) return;
      await api.openBrowserTab(
        current.id,
        'https://www.google.com',
        openRef.current ? bounds() : null,
      );
      if (sessionScopeOwnsCurrent(scope)) await refresh();
    } catch (error) {
      if (conversationIdRef.current === targetConversationId) {
        reportError(t('browser.openFailed'), error);
      }
    }
  }, [
    beginSessionRequest,
    bounds,
    conversationId,
    ensureSession,
    refresh,
    reportError,
    sessionScopeOwnsCurrent,
    session?.cleanupPending,
    t,
  ]);

  const closeTab = useCallback(async (tabId: string) => {
    if (!conversationId || !session) return;
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    let operationScope = scope;
    try {
      const next = await api.closeBrowserTab(session.id, tabId);
      if (!sessionScopeOwnsCurrent(scope)) return;
      if (next.tabs.length === 0) {
        const verificationScope = beginSessionRequest(conversationId, session.id);
        if (!verificationScope) return;
        operationScope = verificationScope;
        const latest = await api.activeBrowserSession(conversationId);
        if (!sessionScopeCanCommit(verificationScope)) return;
        if (latest && latest.id === session.id && latest.tabs.length > 0) {
          commitSession(verificationScope, latest);
          return;
        }
        if (!latest) {
          commitSession(verificationScope, null);
          return;
        }
        if (latest.id !== session.id) return;
        if (!commitSession(verificationScope, latest)) return;
        try {
          await api.closeBrowserSession(session.id);
          commitSession(verificationScope, null);
        } catch (error) {
          try {
            const retained = await api.activeBrowserSession(conversationId);
            if (retained?.id === session.id) {
              commitSession(verificationScope, retained);
            }
          } catch {
            // Preserve the close failure as the primary user-facing error. A
            // later browser event/refresh can still reconcile the session.
          }
          throw error;
        }
      } else {
        commitSession(scope, next);
      }
    } catch (error) {
      reportScopeError(operationScope, t('browser.actionFailed'), error);
    }
  }, [
    beginSessionRequest,
    commitSession,
    conversationId,
    reportScopeError,
    session,
    sessionScopeCanCommit,
    sessionScopeOwnsCurrent,
    t,
  ]);

  const closeSession = useCallback(async (hideWorkspace: boolean) => {
    if (!conversationId || !session) {
      if (hideWorkspace) onOpenChange(false);
      return;
    }
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    const busyGeneration = busyGenerationRef.current + 1;
    busyGenerationRef.current = busyGeneration;
    setBusy(true);
    setLastError(null);
    try {
      await api.closeBrowserSession(session.id);
      // sessionClosed may already have triggered a refresh. Finish the exact
      // closed session without letting an older snapshot revive it, and never
      // hide a different conversation or a replacement session.
      if (conversationIdRef.current === conversationId
        && conversationLifecycleRef.current.generation === scope.conversationGeneration
        && (!sessionIdRef.current || sessionIdRef.current === session.id)) {
        const closedScope = beginSessionRequest(conversationId);
        if (closedScope && commitSession(closedScope, null)) {
          if (hideWorkspace) onOpenChange(false);
          else toast.success(t('browser.cleanupCompleted'));
        }
      }
    } catch (error) {
      reportScopeError(scope, t('browser.actionFailed'), error);
      void refresh();
    } finally {
      if (busyGenerationRef.current === busyGeneration) setBusy(false);
    }
  }, [
    beginSessionRequest,
    commitSession,
    conversationId,
    onOpenChange,
    refresh,
    reportScopeError,
    session,
    t,
  ]);

  const retryCloseSession = useCallback(() => closeSession(false), [closeSession]);

  const startPick = useCallback(async (mode: 'element' | 'region') => {
    if (!conversationId || !session || !currentTab || !onSendArtifactToAgent) return;
    const scope = {
      conversationId,
      sessionId: session.id,
      tabId: currentTab.id,
      generation: artifactScopeGenerationRef.current,
    };
    setPickMode(mode);
    try {
      if (mode === 'element') await api.beginBrowserElementPick(session.id, currentTab.id);
      else await api.beginBrowserRegionPick(session.id, currentTab.id);
      if (!artifactScopeIsCurrent(scope)) return;
      if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
      let attempts = 0;
      let pollInFlight = false;
      pickTimerRef.current = window.setInterval(() => {
        if (pollInFlight || !artifactScopeIsCurrent(scope)) return;
        pollInFlight = true;
        attempts += 1;
        void api.takeBrowserPick(session.id, currentTab.id).then((artifact) => {
          if (!artifactScopeIsCurrent(scope)) return;
          if (artifact) {
            if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
            pickTimerRef.current = null;
            setPickMode(null);
            onSendArtifactToAgent({
              conversationId: scope.conversationId,
              sessionId: scope.sessionId,
              tabId: scope.tabId,
              selection: artifact,
            });
            toast.success(t('browser.artifactAttached'));
          } else if (attempts >= 120) {
            if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
            pickTimerRef.current = null;
            setPickMode(null);
          }
        }).catch(() => {
          if (!artifactScopeIsCurrent(scope)) return;
          if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
          pickTimerRef.current = null;
          setPickMode(null);
        }).finally(() => {
          pollInFlight = false;
        });
      }, 250);
    } catch (error) {
      if (artifactScopeIsCurrent(scope)) {
        setPickMode(null);
        reportError(t('browser.actionFailed'), error);
      }
    }
  }, [artifactScopeIsCurrent, conversationId, currentTab, onSendArtifactToAgent, reportError, session, t]);

  const sendSelectedText = useCallback(async () => {
    if (!conversationId || !session || !currentTab || !onSendArtifactToAgent) return;
    const scope = {
      conversationId,
      sessionId: session.id,
      tabId: currentTab.id,
      generation: artifactScopeGenerationRef.current,
    };
    try {
      const text = await api.selectedBrowserText(session.id, currentTab.id);
      if (!artifactScopeIsCurrent(scope)) return;
      if (!text.trim()) {
        toast.info(t('browser.noSelectedText'));
        return;
      }
      onSendArtifactToAgent({
        conversationId: scope.conversationId,
        sessionId: scope.sessionId,
        tabId: scope.tabId,
        selection: { kind: 'text', url: currentTab.url, title: currentTab.title, text },
      });
      toast.success(t('browser.artifactAttached'));
    } catch (error) {
      if (artifactScopeIsCurrent(scope)) reportError(t('browser.actionFailed'), error);
    }
  }, [artifactScopeIsCurrent, conversationId, currentTab, onSendArtifactToAgent, reportError, session, t]);

  const takeControl = useCallback(async () => {
    if (!conversationId || !session) return;
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    try {
      commitSession(scope, await api.acquireBrowserControl(session.id, 'user'));
    } catch (error) {
      reportScopeError(scope, t('browser.actionFailed'), error);
    }
  }, [beginSessionRequest, commitSession, conversationId, reportScopeError, session, t]);

  const handBackControl = useCallback(async () => {
    if (!conversationId || !session) return;
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    try {
      commitSession(scope, await api.acquireBrowserControl(session.id, 'none'));
    } catch (error) {
      reportScopeError(scope, t('browser.actionFailed'), error);
    }
  }, [beginSessionRequest, commitSession, conversationId, reportScopeError, session, t]);

  const activateTab = useCallback(async (tabId: string) => {
    if (!conversationId || !session) return;
    const scope = beginSessionRequest(conversationId, session.id);
    if (!scope) return;
    try {
      commitSession(scope, await api.activateBrowserTab(session.id, tabId));
    } catch (error) {
      reportScopeError(scope, t('browser.actionFailed'), error);
    }
  }, [beginSessionRequest, commitSession, conversationId, reportScopeError, session, t]);

  const resize = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (effectiveFullScreen) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = width;
    let finalWidth = startWidth;
    const onMove = (moveEvent: PointerEvent) => {
      const next = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startWidth + startX - moveEvent.clientX));
      finalWidth = next;
      setWidth(next);
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      localStorage.setItem(WIDTH_STORAGE_KEY, String(finalWidth));
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp, { once: true });
  }, [effectiveFullScreen, width]);

  if (!open) return null;

  const control = ownerType(session?.controlOwner);
  const dock = (
    <aside
      data-testid="browser-dock"
      className={`${effectiveFullScreen ? 'absolute inset-0 z-40' : 'relative h-full shrink-0'} flex min-h-0 flex-col overflow-hidden border-l border-border/70 bg-surface-1 shadow-[-24px_0_60px_rgba(0,0,0,.2)]`}
      style={effectiveFullScreen ? undefined : { width }}
      aria-label={t('browser.title')}
    >
      {!effectiveFullScreen && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label={t('browser.resize')}
          className="absolute bottom-0 left-0 top-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize bg-transparent hover:bg-cyan-400/35"
          onPointerDown={resize}
        />
      )}
      <header className="shrink-0 border-b border-border/70 bg-[linear-gradient(110deg,rgba(8,47,73,.3),transparent_55%)] px-2.5 pb-2 pt-2" data-theme-density-surface="browser-header">
        <div className="flex items-center gap-2">
          <div className="grid h-8 w-8 place-items-center rounded-md border border-cyan-400/20 bg-cyan-400/10 text-cyan-300">
            <Globe2 size={16} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 text-xs font-semibold tracking-wide text-text-primary">
              {t('browser.title')}
              <span className={`h-1.5 w-1.5 rounded-full ${currentTab?.loading ? 'animate-pulse bg-amber-400' : control === 'agent' ? 'animate-pulse bg-cyan-300' : control === 'user' ? 'bg-emerald-400' : 'bg-text-tertiary'}`} />
            </div>
            <div className="truncate text-[10px] text-text-tertiary">
              {control === 'agent'
                ? t('browser.agentControlling', { agent: agentLabel || 'Agent' })
                : control === 'user'
                  ? t('browser.userControlling')
                  : t('browser.sessionIdle')}
            </div>
          </div>
          {control === 'agent' && (
            <button type="button" onClick={() => void takeControl()} className="inline-flex h-8 items-center gap-1.5 rounded-md border border-amber-400/25 bg-amber-400/10 px-2 text-[11px] font-medium text-amber-300 hover:bg-amber-400/15">
              <Hand size={13} /> {t('browser.takeControl')}
            </button>
          )}
          {control === 'user' && (
            <button type="button" onClick={() => void handBackControl()} className="inline-flex h-8 items-center gap-1.5 rounded-md border border-emerald-400/25 bg-emerald-400/10 px-2 text-[11px] font-medium text-emerald-300 hover:bg-emerald-400/15">
              <Hand size={13} /> {t('browser.handBackControl')}
            </button>
          )}
          {!narrowViewport && (
            <button type="button" onClick={() => setFullScreen((value) => !value)} className="grid h-8 w-8 place-items-center rounded-md text-text-tertiary hover:bg-surface-3 hover:text-text-primary" aria-label={fullScreen ? t('browser.exitFullScreen') : t('browser.fullScreen')}>
              {fullScreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
          )}
          <button type="button" onClick={() => void closeSession(true)} className="grid h-8 w-8 place-items-center rounded-md text-text-tertiary hover:bg-surface-3 hover:text-text-primary" aria-label={t('browser.close')}>
            <X size={15} />
          </button>
        </div>

        <div className="mt-2 flex items-center gap-1.5">
          <button type="button" disabled={!currentTab || busy} onClick={() => void withCurrentTab(api.goBackBrowserTab)} className="browser-tool-button" aria-label={t('browser.back')}><ArrowLeft size={14} /></button>
          <button type="button" disabled={!currentTab || busy} onClick={() => void withCurrentTab(api.goForwardBrowserTab)} className="browser-tool-button" aria-label={t('browser.forward')}><ArrowRight size={14} /></button>
          <button type="button" disabled={!currentTab || busy} onClick={() => void withCurrentTab(currentTab?.loading ? api.stopBrowserTab : api.reloadBrowserTab)} className="browser-tool-button" aria-label={currentTab?.loading ? t('browser.stop') : t('browser.reload')}>
            {busy ? <Loader2 size={14} className="animate-spin" /> : currentTab?.loading ? <Square size={11} /> : <RefreshCw size={13} />}
          </button>
          <form onSubmit={navigate} className="min-w-0 flex-1">
            <div className="flex h-8 items-center rounded-md border border-border/70 bg-surface-2/80 px-2 focus-within:border-cyan-400/40 focus-within:ring-1 focus-within:ring-cyan-400/15">
              <span className={`mr-1.5 h-1.5 w-1.5 shrink-0 rounded-full ${currentTab?.url.startsWith('https://') ? 'bg-emerald-400' : 'bg-amber-400'}`} />
              <input value={address} onChange={(event) => setAddress(event.target.value)} className="min-w-0 flex-1 bg-transparent text-[11px] text-text-primary outline-none" placeholder={t('browser.addressPlaceholder')} aria-label={t('browser.address')} />
            </div>
          </form>
          <button type="button" disabled={!currentTab} onClick={() => currentTab && void openExternal(currentTab.url).catch((error) => reportError(t('browser.openFailed'), error))} className="browser-tool-button" aria-label={t('browser.openExternal')}><ExternalLink size={13} /></button>
        </div>

        <div className="mt-2 flex items-center gap-1.5 overflow-x-auto pb-0.5">
          {session?.tabs.map((tab) => (
            <div key={tab.id} className={`group flex h-7 min-w-24 max-w-40 items-center gap-1 rounded-md border px-2 text-[10px] ${tab.active ? 'border-cyan-400/25 bg-cyan-400/10 text-text-primary' : 'border-transparent bg-surface-2/60 text-text-tertiary hover:bg-surface-3'}`}>
              <button type="button" onClick={() => void activateTab(tab.id)} className="min-w-0 flex-1 truncate text-left" title={shortTitle(tab)}>{shortTitle(tab)}</button>
              <button type="button" onClick={() => void closeTab(tab.id)} className="opacity-0 transition-opacity group-hover:opacity-100" aria-label={t('browser.closeTab')}><X size={11} /></button>
            </div>
          ))}
          <button type="button" disabled={busy || session?.cleanupPending} onClick={() => void addTab()} className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-text-tertiary hover:bg-surface-3 hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40" aria-label={t('browser.newTab')}><Plus size={13} /></button>
        </div>

        {onSendArtifactToAgent && (
          <div className="mt-1.5 flex items-center gap-1">
            <button type="button" disabled={!currentTab || pickMode !== null} onClick={() => void startPick('element')} className={`browser-action-chip ${pickMode === 'element' ? 'border-cyan-400/40 bg-cyan-400/15 text-cyan-200' : ''}`}><MousePointer2 size={12} /> {t('browser.pointOut')}</button>
            <button type="button" disabled={!currentTab || pickMode !== null} onClick={() => void startPick('region')} className={`browser-action-chip ${pickMode === 'region' ? 'border-cyan-400/40 bg-cyan-400/15 text-cyan-200' : ''}`}><Crosshair size={12} /> {t('browser.selectRegion')}</button>
            <button type="button" disabled={!currentTab} onClick={() => void sendSelectedText()} className="browser-action-chip"><TextCursorInput size={12} /> {t('browser.sendText')}</button>
            <span className="ml-auto flex items-center gap-1 text-[9px] uppercase tracking-[.14em] text-text-tertiary"><Send size={10} /> {t('browser.sharedSession')}</span>
          </div>
        )}
      </header>

      <div ref={contentRef} className="relative min-h-0 flex-1 bg-[#071018]" data-testid="browser-native-surface">
        {!currentTab && (
          <div className="absolute inset-0 grid place-items-center px-8 text-center">
            <div>
              <Globe2 className="mx-auto h-8 w-8 text-cyan-300/60" />
              <p className="mt-3 text-xs font-medium text-text-secondary">
                {session?.cleanupPending ? t('browser.cleanupPending') : t('browser.empty')}
              </p>
              {session?.cleanupPending && (
                <button
                  type="button"
                  data-testid="browser-retry-close-session"
                  disabled={busy}
                  onClick={() => void retryCloseSession()}
                  className="browser-action-chip mx-auto mt-3"
                >
                  {busy ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
                  {t('browser.retryCleanup')}
                </button>
              )}
            </div>
          </div>
        )}
        {pickMode && (
          <div className="pointer-events-none absolute left-3 top-3 z-10 rounded-md border border-cyan-400/30 bg-[#04131d]/90 px-2.5 py-1.5 text-[10px] text-cyan-100 shadow-lg backdrop-blur">
            {pickMode === 'element' ? t('browser.pickElementHint') : t('browser.pickRegionHint')}
          </div>
        )}
      </div>
    </aside>
  );
  // Expand inside the application content area. The titlebar owns its own
  // native controls and must never share coordinates with browser controls.
  const workspace = document.getElementById('app-window-content');
  return effectiveFullScreen && workspace ? createPortal(dock, workspace) : dock;
}
