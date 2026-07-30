import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react';
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

export const OPEN_BROWSER_WORKSPACE_EVENT = 'nexa:open-browser-workspace';

export interface BrowserDockStatus {
  tabCount: number;
  state: 'empty' | 'idle' | 'loading' | 'agent' | 'user' | 'error';
}

export type BrowserAgentArtifact = api.BrowserPickArtifact | {
  kind: 'text';
  url: string;
  title: string;
  text: string;
};

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
  const sessionPromiseRef = useRef<Promise<api.BrowserSessionInfo | null> | null>(null);
  const conversationIdRef = useRef(conversationId);
  conversationIdRef.current = conversationId;
  const session = storedSession?.conversationId === conversationId ? storedSession : null;
  const currentTab = useMemo(() => activeTab(session), [session]);
  const effectiveFullScreen = fullScreen || narrowViewport;

  useEffect(() => {
    const query = window.matchMedia('(max-width: 959px)');
    const update = (event: MediaQueryListEvent | MediaQueryList) => setNarrowViewport(event.matches);
    update(query);
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);

  const refresh = useCallback(async () => {
    if (!conversationId) {
      setSession(null);
      return null;
    }
    const next = await api.activeBrowserSession(conversationId);
    if (conversationIdRef.current === conversationId) setSession(next);
    return next;
  }, [conversationId]);

  const reportError = useCallback((message: string, error: unknown) => {
    const formatted = formatUserError(message, error);
    setLastError(formatted);
    toast.error(formatted);
  }, []);

  const bounds = useCallback((): api.BrowserBounds | null => {
    const rect = contentRef.current?.getBoundingClientRect();
    if (!rect || rect.width < 1 || rect.height < 1) return null;
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  }, []);

  const syncBounds = useCallback(async (visible = open) => {
    if (!session) return;
    const nextBounds = bounds() ?? latestBoundsRef.current;
    if (!nextBounds) return;
    latestBoundsRef.current = nextBounds;
    await api.setBrowserBounds(session.id, nextBounds, visible);
  }, [bounds, open, session]);

  const ensureSession = useCallback(async (url?: string) => {
    if (!conversationId) return null;
    if (sessionPromiseRef.current) {
      const current = await sessionPromiseRef.current;
      if (current?.conversationId === conversationId && url) {
        await api.openBrowserTab(current.id, url, open ? bounds() : null);
        const refreshed = await api.activeBrowserSession(conversationId);
        if (conversationIdRef.current === conversationId) setSession(refreshed);
        return refreshed;
      }
      if (current?.conversationId === conversationId) return current;
      sessionPromiseRef.current = null;
    }
    const pending = (async () => {
    let current = session?.conversationId === conversationId
      ? session
      : await api.activeBrowserSession(conversationId);
    if (current?.conversationId !== conversationId) current = null;
    const nextBounds = bounds();
    if (!current) {
      current = await api.createBrowserSession({
        conversationId,
        url: url || 'https://www.google.com',
        openInitialUrlOnReuse: Boolean(url),
        bounds: open ? nextBounds : null,
      });
    } else if (url) {
      await api.openBrowserTab(current.id, url, open ? nextBounds : null);
      current = await api.activeBrowserSession(conversationId);
    }
    if (conversationIdRef.current === conversationId) setSession(current);
    return current;
    })();
    sessionPromiseRef.current = pending;
    try {
      return await pending;
    } finally {
      if (sessionPromiseRef.current === pending) sessionPromiseRef.current = null;
    }
  }, [bounds, conversationId, open, session]);

  useEffect(() => {
    void refresh().catch(() => setSession(null));
  }, [refresh]);

  useEffect(() => {
    if (!open || !conversationId) return;
    void ensureSession()
      .then(() => window.requestAnimationFrame(() => void syncBounds(true)))
      .catch((error) => reportError(t('browser.openFailed'), error));
  }, [conversationId, ensureSession, open, reportError, syncBounds, t]);

  useEffect(() => {
    if (!session) return;
    if (!open) {
      void syncBounds(false).catch(() => undefined);
      return;
    }
    const element = contentRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      window.requestAnimationFrame(() => void syncBounds(true).catch(() => undefined));
    });
    const handleResize = () => void syncBounds(true).catch(() => undefined);
    observer.observe(element);
    window.addEventListener('resize', handleResize);
    void syncBounds(true);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', handleResize);
      void api.setBrowserBounds(session.id, latestBoundsRef.current ?? { x: 0, y: 0, width: 1, height: 1 }, false).catch(() => undefined);
    };
  }, [effectiveFullScreen, open, session, syncBounds, width]);

  useEffect(() => {
    const handler = (event: Event) => {
      if (!conversationId) return;
      const detail = (event as CustomEvent<{ url?: string }>).detail;
      event.preventDefault();
      onOpenChange(true);
      if (detail?.url) {
        void ensureSession(detail.url).catch((error) => reportError(t('browser.openFailed'), error));
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
      if (event.payload.kind === 'downloadRequested') {
        toast.warning(t('browser.downloadBlocked'));
      }
      if (event.payload.kind === 'newWindowRequested') {
        const url = String(event.payload.payload.url ?? '');
        const sessionId = String(event.payload.payload.sessionId ?? '');
        const sourceTabId = String(event.payload.payload.tabId ?? '');
        if (url && sessionId && sourceTabId && session?.id === sessionId) {
          void api.openBrowserPopup(sessionId, sourceTabId, url, open ? bounds() : null)
            .then(() => refresh())
            .catch((error) => reportError(t('browser.popupBlocked'), error));
          return;
        }
      }
      void refresh().catch(() => undefined);
    }).then((dispose) => {
      if (disposed) dispose(); else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bounds, open, refresh, reportError, session?.id, t]);

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
  }, [currentTab?.loading, lastError, onStatusChange, session]);

  useEffect(() => () => {
    if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
  }, []);

  const withCurrentTab = useCallback(async (
    operation: (sessionId: string, tabId: string) => Promise<unknown>,
  ) => {
    if (!session || !currentTab) return;
    setBusy(true);
    setLastError(null);
    try {
      await operation(session.id, currentTab.id);
      await refresh();
    } catch (error) {
      reportError(t('browser.actionFailed'), error);
    } finally {
      setBusy(false);
    }
  }, [currentTab, refresh, reportError, session, t]);

  const navigate = useCallback(async (event: FormEvent) => {
    event.preventDefault();
    if (!address.trim()) return;
    await withCurrentTab((sessionId, tabId) => api.navigateBrowserTab(sessionId, tabId, address));
  }, [address, withCurrentTab]);

  const addTab = useCallback(async () => {
    try {
      const current = await ensureSession();
      if (!current) return;
      await api.openBrowserTab(current.id, 'https://www.google.com', open ? bounds() : null);
      await refresh();
    } catch (error) {
      reportError(t('browser.openFailed'), error);
    }
  }, [bounds, ensureSession, open, refresh, reportError, t]);

  const closeTab = useCallback(async (tabId: string) => {
    if (!session) return;
    try {
      const next = await api.closeBrowserTab(session.id, tabId);
      if (next.tabs.length === 0) {
        await api.closeBrowserSession(session.id);
        setSession(null);
      } else {
        setSession(next);
      }
    } catch (error) {
      reportError(t('browser.actionFailed'), error);
    }
  }, [reportError, session, t]);

  const startPick = useCallback(async (mode: 'element' | 'region') => {
    if (!session || !currentTab) return;
    setPickMode(mode);
    try {
      if (mode === 'element') await api.beginBrowserElementPick(session.id, currentTab.id);
      else await api.beginBrowserRegionPick(session.id, currentTab.id);
      if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
      let attempts = 0;
      pickTimerRef.current = window.setInterval(() => {
        attempts += 1;
        void api.takeBrowserPick(session.id, currentTab.id).then((artifact) => {
          if (artifact) {
            if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
            pickTimerRef.current = null;
            setPickMode(null);
            onSendArtifactToAgent?.(artifact);
            toast.success(t('browser.artifactAttached'));
          } else if (attempts >= 120) {
            if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
            pickTimerRef.current = null;
            setPickMode(null);
          }
        }).catch(() => {
          if (pickTimerRef.current !== null) window.clearInterval(pickTimerRef.current);
          pickTimerRef.current = null;
          setPickMode(null);
        });
      }, 250);
    } catch (error) {
      setPickMode(null);
      reportError(t('browser.actionFailed'), error);
    }
  }, [currentTab, onSendArtifactToAgent, reportError, session, t]);

  const sendSelectedText = useCallback(async () => {
    if (!session || !currentTab) return;
    try {
      const text = await api.selectedBrowserText(session.id, currentTab.id);
      if (!text.trim()) {
        toast.info(t('browser.noSelectedText'));
        return;
      }
      onSendArtifactToAgent?.({ kind: 'text', url: currentTab.url, title: currentTab.title, text });
      toast.success(t('browser.artifactAttached'));
    } catch (error) {
      reportError(t('browser.actionFailed'), error);
    }
  }, [currentTab, onSendArtifactToAgent, reportError, session, t]);

  const takeControl = useCallback(async () => {
    if (!session) return;
    try {
      setSession(await api.acquireBrowserControl(session.id, 'user'));
    } catch (error) {
      reportError(t('browser.actionFailed'), error);
    }
  }, [reportError, session, t]);

  const handBackControl = useCallback(async () => {
    if (!session) return;
    try {
      setSession(await api.acquireBrowserControl(session.id, 'none'));
    } catch (error) {
      reportError(t('browser.actionFailed'), error);
    }
  }, [reportError, session, t]);

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
  return (
    <aside
      data-testid="browser-dock"
      className={`${effectiveFullScreen ? 'fixed inset-0 z-40' : 'relative h-full shrink-0'} flex min-h-0 flex-col overflow-hidden border-l border-border/70 bg-surface-1 shadow-[-24px_0_60px_rgba(0,0,0,.2)]`}
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
      <header className="shrink-0 border-b border-border/70 bg-[linear-gradient(110deg,rgba(8,47,73,.3),transparent_55%)] px-2.5 pb-2 pt-2">
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
          <button type="button" onClick={() => onOpenChange(false)} className="grid h-8 w-8 place-items-center rounded-md text-text-tertiary hover:bg-surface-3 hover:text-text-primary" aria-label={t('browser.close')}>
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
              <button type="button" onClick={() => session && void api.activateBrowserTab(session.id, tab.id).then(setSession)} className="min-w-0 flex-1 truncate text-left" title={shortTitle(tab)}>{shortTitle(tab)}</button>
              <button type="button" onClick={() => void closeTab(tab.id)} className="opacity-0 transition-opacity group-hover:opacity-100" aria-label={t('browser.closeTab')}><X size={11} /></button>
            </div>
          ))}
          <button type="button" onClick={() => void addTab()} className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-text-tertiary hover:bg-surface-3 hover:text-text-primary" aria-label={t('browser.newTab')}><Plus size={13} /></button>
        </div>

        <div className="mt-1.5 flex items-center gap-1">
          <button type="button" disabled={!currentTab || pickMode !== null} onClick={() => void startPick('element')} className={`browser-action-chip ${pickMode === 'element' ? 'border-cyan-400/40 bg-cyan-400/15 text-cyan-200' : ''}`}><MousePointer2 size={12} /> {t('browser.pointOut')}</button>
          <button type="button" disabled={!currentTab || pickMode !== null} onClick={() => void startPick('region')} className={`browser-action-chip ${pickMode === 'region' ? 'border-cyan-400/40 bg-cyan-400/15 text-cyan-200' : ''}`}><Crosshair size={12} /> {t('browser.selectRegion')}</button>
          <button type="button" disabled={!currentTab} onClick={() => void sendSelectedText()} className="browser-action-chip"><TextCursorInput size={12} /> {t('browser.sendText')}</button>
          <span className="ml-auto flex items-center gap-1 text-[9px] uppercase tracking-[.14em] text-text-tertiary"><Send size={10} /> {t('browser.sharedSession')}</span>
        </div>
      </header>

      <div ref={contentRef} className="relative min-h-0 flex-1 bg-[#071018]" data-testid="browser-native-surface">
        {!currentTab && (
          <div className="absolute inset-0 grid place-items-center px-8 text-center">
            <div>
              <Globe2 className="mx-auto h-8 w-8 text-cyan-300/60" />
              <p className="mt-3 text-xs font-medium text-text-secondary">{t('browser.empty')}</p>
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
}
