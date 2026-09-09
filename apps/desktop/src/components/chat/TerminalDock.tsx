import { useCallback, useEffect, useRef, useState } from 'react';
import { NexaSelect } from '../ui/overlay';
import { listen } from '@tauri-apps/api/event';
import { Terminal as XTerm, type FontWeight } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import {
  ChevronDown,
  Copy,
  Link2,
  Loader2,
  Maximize2,
  MessageSquareText,
  Minimize2,
  PanelBottomOpen,
  Plus,
  Power,
  RotateCcw,
  TerminalSquare,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import type { TerminalEvent, TerminalSessionInfo, TerminalShell } from '../../lib/api';
import { useTheme } from '../../lib/ThemeProvider';
import { useTranslation } from '../../i18n';
import { Button } from '../ui/Button';

type TerminalStatus = 'idle' | 'starting' | 'running' | 'exited' | 'error';

const TERMINAL_EVENT = 'terminal:event';
export const TERMINAL_TOGGLE_EVENT = 'nexa:terminal-toggle';
export const TERMINAL_OPEN_EVENT = 'nexa:terminal-open';
const MAX_BUFFER_CHARS = 180_000;

const SHELL_OPTIONS: Array<{ value: TerminalShell; label: string }> = [
  { value: 'default', label: 'Default' },
  { value: 'powershell', label: 'PowerShell' },
  { value: 'cmd', label: 'Cmd' },
  { value: 'bash', label: 'Bash' },
];

const FALLBACK_TERMINAL_THEME = {
  background: '#0a0a0f',
  foreground: '#f0f0f5',
  cursor: '#2dd4bf',
  selectionBackground: '#14B8A620',
  black: '#12121a',
  red: '#ef4444',
  green: '#22c55e',
  yellow: '#f59e0b',
  blue: '#3b82f6',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#f0f0f5',
  brightBlack: '#606070',
  brightRed: '#f87171',
  brightGreen: '#22c55e',
  brightYellow: '#f59e0b',
  brightBlue: '#3b82f6',
  brightMagenta: '#d670d6',
  brightCyan: '#29b8db',
  brightWhite: '#ffffff',
};

function cssColorVar(styles: CSSStyleDeclaration, name: string, fallback: string): string {
  const value = styles.getPropertyValue(name).trim();
  return value || fallback;
}

function readTerminalTheme() {
  if (typeof window === 'undefined') {
    return FALLBACK_TERMINAL_THEME;
  }
  const styles = window.getComputedStyle(document.documentElement);
  const surface0 = cssColorVar(styles, '--color-surface-0', FALLBACK_TERMINAL_THEME.background);
  const textPrimary = cssColorVar(styles, '--color-text-primary', FALLBACK_TERMINAL_THEME.foreground);
  const accentHover = cssColorVar(styles, '--color-accent-hover', FALLBACK_TERMINAL_THEME.cyan);
  const accentSubtle = cssColorVar(styles, '--color-accent-subtle', FALLBACK_TERMINAL_THEME.selectionBackground);

  return {
    ...FALLBACK_TERMINAL_THEME,
    background: surface0,
    foreground: textPrimary,
    cursor: accentHover,
    selectionBackground: accentSubtle,
  };
}

const TERMINAL_FONT_FALLBACK = '"CaskaydiaMono Nerd Font", "Cascadia Mono", "JetBrainsMono Nerd Font", "JetBrains Mono", "Symbols Nerd Font Mono", Consolas, monospace';

function readTerminalFont() {
  const styles = getComputedStyle(document.documentElement);
  return styles.getPropertyValue('--user-font-mono').trim()
    || styles.getPropertyValue('--theme-font-mono').trim()
    || TERMINAL_FONT_FALLBACK;
}

function clampOutputBuffer(value: string): string {
  if (value.length <= MAX_BUFFER_CHARS) return value;
  return value.slice(value.length - MAX_BUFFER_CHARS);
}

async function writeClipboardText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  const fallback = document.createElement('textarea');
  fallback.value = value;
  fallback.style.position = 'fixed';
  fallback.style.opacity = '0';
  document.body.appendChild(fallback);
  fallback.select();
  const copied = document.execCommand('copy');
  fallback.remove();
  if (!copied) {
    throw new Error('Clipboard is unavailable');
  }
}

function terminalStatusLabel(status: TerminalStatus): string {
  switch (status) {
    case 'starting':
      return 'Starting';
    case 'running':
      return 'Running';
    case 'exited':
      return 'Exited';
    case 'error':
      return 'Error';
    default:
      return 'Ready';
  }
}

function terminalStatusClass(status: TerminalStatus): string {
  switch (status) {
    case 'running':
      return 'bg-success';
    case 'starting':
      return 'bg-accent';
    case 'error':
      return 'bg-danger';
    case 'exited':
      return 'bg-warning';
    default:
      return 'bg-text-tertiary';
  }
}

export interface TerminalAgentSelection {
  text: string;
  outputTail: string;
  session: TerminalSessionInfo;
}

interface TerminalDockProps {
  conversationId?: string;
  agentLabel?: string;
  onSendSelectionToAgent?: (selection: TerminalAgentSelection) => void;
  onRenderedChange?: (rendered: boolean) => void;
}

export function TerminalDock({
  conversationId,
  agentLabel,
  onSendSelectionToAgent,
  onRenderedChange,
}: TerminalDockProps) {
  const { theme } = useTheme();
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const [isTall, setIsTall] = useState(false);
  const [selectedShell, setSelectedShell] = useState<TerminalShell>('default');
  const [session, setSession] = useState<TerminalSessionInfo | null>(null);
  const [availableSessions, setAvailableSessions] = useState<TerminalSessionInfo[]>([]);
  const [isRestoring, setIsRestoring] = useState(false);
  const [status, setStatus] = useState<TerminalStatus>('idle');
  const [error, setError] = useState<string | null>(null);
  const [selection, setSelection] = useState('');
  const hostRef = useRef<HTMLDivElement | null>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const statusRef = useRef<TerminalStatus>('idle');
  const outputBufferRef = useRef('');
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startingRef = useRef(false);
  const restoringRef = useRef(false);

  const toggleTerminalPanel = useCallback(() => {
    setIsOpen((value) => !value);
  }, []);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  const appendTerminalData = useCallback((data: string) => {
    outputBufferRef.current = clampOutputBuffer(outputBufferRef.current + data);
    xtermRef.current?.write(data);
  }, []);

  const appendSystemLine = useCallback((message: string) => {
    appendTerminalData(`\r\n\x1b[2m${message}\x1b[0m\r\n`);
  }, [appendTerminalData]);

  const attachSession = useCallback(async (
    info: TerminalSessionInfo,
    isCancelled: () => boolean = () => false,
  ) => {
    const snapshot = await api.snapshotTerminalSession(info.id, MAX_BUFFER_CHARS);
    if (isCancelled()) return;
    sessionIdRef.current = info.id;
    outputBufferRef.current = clampOutputBuffer(snapshot.output);
    setSession(snapshot.session);
    setSelection('');
    setError(null);
    setStatus('running');
    xtermRef.current?.reset();
    if (outputBufferRef.current) {
      xtermRef.current?.write(outputBufferRef.current);
    }
  }, []);

  const resizeActiveTerminal = useCallback(() => {
    const term = xtermRef.current;
    const fitAddon = fitAddonRef.current;
    if (!term || !fitAddon) return;
    try {
      fitAddon.fit();
    } catch {
      return;
    }
    const sessionId = sessionIdRef.current;
    if (!sessionId) return;
    void api.resizeTerminalSession(sessionId, term.rows, term.cols).catch((err) => {
      console.warn('[TerminalDock] resize failed:', err);
    });
  }, []);

  const closeActiveSession = useCallback(async (nextStatus: TerminalStatus = 'exited') => {
    const sessionId = sessionIdRef.current;
    if (!sessionId) {
      setSession(null);
      setSelection('');
      setStatus(nextStatus);
      return;
    }
    sessionIdRef.current = null;
    setSession(null);
    setAvailableSessions((sessions) => sessions.filter((item) => item.id !== sessionId));
    setSelection('');
    setStatus(nextStatus);
    try {
      await api.closeTerminalSession(sessionId);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setStatus('error');
      appendSystemLine(message);
    }
  }, [appendSystemLine]);

  const closeTerminalDock = useCallback(() => {
    setIsOpen(false);
    setIsTall(false);
    setError(null);
    if (!sessionIdRef.current) {
      setStatus('idle');
      setSession(null);
      outputBufferRef.current = '';
      xtermRef.current?.reset();
    }
  }, []);

  const startSession = useCallback(async (shell: TerminalShell = selectedShell) => {
    if (startingRef.current) return;
    startingRef.current = true;

    setIsOpen(true);
    setError(null);
    setStatus('starting');
    setSession(null);
    setSelection('');
    outputBufferRef.current = '';
    xtermRef.current?.reset();

    try {
      const term = xtermRef.current;
      const started = await api.startTerminalSession({
        shell,
        rows: term?.rows ?? 24,
        cols: term?.cols ?? 80,
        conversationId: conversationId ?? null,
      });
      const info = conversationId
        ? await api.bindTerminalSession(started.id, conversationId)
        : started;
      sessionIdRef.current = info.id;
      setSession(info);
      setAvailableSessions((sessions) => [
        ...sessions.filter((item) => item.id !== info.id),
        info,
      ]);
      setStatus('running');
      appendSystemLine(`${info.shell} · ${info.cwd}`);
      resizeActiveTerminal();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setStatus('error');
      appendSystemLine(message);
    } finally {
      startingRef.current = false;
    }
  }, [appendSystemLine, conversationId, resizeActiveTerminal, selectedShell]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    listen<TerminalEvent>(TERMINAL_EVENT, (event) => {
      if (cancelled) return;
      const payload = event.payload;
      if (!payload || payload.sessionId !== sessionIdRef.current) return;
      if (payload.kind === 'data') {
        appendTerminalData(payload.data ?? '');
        return;
      }
      if (payload.kind === 'exit') {
        const exitText = payload.signal
          ? `process exited by ${payload.signal}`
          : `process exited with code ${payload.exitCode ?? 'unknown'}`;
        appendSystemLine(exitText);
        sessionIdRef.current = null;
        setSession(null);
        setAvailableSessions((sessions) => sessions.filter((item) => item.id !== payload.sessionId));
        setSelection('');
        setStatus('exited');
        return;
      }
      const message = payload.data || 'terminal session failed';
      appendSystemLine(message);
      sessionIdRef.current = null;
      setError(message);
      setSelection('');
      setStatus('error');
    }).then((dispose) => {
      if (cancelled) {
        dispose();
      } else {
        unlisten = dispose;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appendSystemLine, appendTerminalData]);

  useEffect(() => {
    if (!isOpen || !hostRef.current || xtermRef.current) return;

    const term = new XTerm({
      allowProposedApi: true,
      cursorBlink: true,
      convertEol: false,
      fontFamily: readTerminalFont(),
      fontSize: 14,
      lineHeight: 1.1,
      minimumContrastRatio: 1,
      customGlyphs: true,
      rescaleOverlappingGlyphs: true,
      scrollback: 5000,
      theme: readTerminalTheme(),
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = '11';
    term.open(hostRef.current);
    // Replay the saved buffer before publishing the renderer. New output is
    // written directly once xtermRef is set and must not be replayed next frame.
    if (outputBufferRef.current) term.write(outputBufferRef.current);
    let disposed = false;
    // WebGL draws box/powerline glyphs consistently. DOM remains usable when
    // the device has no working graphics context or loses it after opening.
    void (async () => {
      try {
        const { LigaturesAddon } = await import('@xterm/addon-ligatures');
        if (disposed) return;
        term.loadAddon(new LigaturesAddon());
      } catch { /* Font ligatures are optional. */ }
      const { WebglAddon } = await import('@xterm/addon-webgl');
      if (disposed) return;
      const addon = new WebglAddon();
      try {
        addon.onContextLoss(() => { if (!disposed) addon.dispose(); });
        term.loadAddon(addon);
      } catch { addon.dispose(); }
    })().catch(() => { /* Keep the DOM renderer if optional graphics loading fails. */ });
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true;
      const key = event.key.toLowerCase();
      const copyShortcut = key === 'c' && (event.ctrlKey || event.metaKey);
      if (copyShortcut && term.hasSelection()) {
        const selectedText = term.getSelection();
        void writeClipboardText(selectedText)
          .then(() => toast.success(t('chat.terminalSelectionCopied')))
          .catch((copyError) => {
            const message = copyError instanceof Error ? copyError.message : String(copyError);
            appendSystemLine(message);
          });
        return false;
      }
      const pasteShortcut = key === 'v' && (
        event.metaKey || (event.ctrlKey && event.shiftKey)
      );
      if (pasteShortcut) {
        if (!navigator.clipboard?.readText) return false;
        void navigator.clipboard.readText()
          .then((text) => {
            const sessionId = sessionIdRef.current;
            if (!text || !sessionId || statusRef.current !== 'running') return;
            return api.writeTerminalSession(sessionId, text);
          })
          .catch((pasteError) => {
            const message = pasteError instanceof Error ? pasteError.message : String(pasteError);
            appendSystemLine(message);
          });
        return false;
      }
      return true;
    });
    term.onData((data) => {
      const sessionId = sessionIdRef.current;
      if (!sessionId || statusRef.current !== 'running') return;
      void api.writeTerminalSession(sessionId, data).catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        appendSystemLine(message);
      });
    });
    term.onSelectionChange(() => {
      setSelection(term.getSelection());
    });
    xtermRef.current = term;
    fitAddonRef.current = fitAddon;

    requestAnimationFrame(() => {
      resizeActiveTerminal();
    });

    const observer = new ResizeObserver(() => {
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = setTimeout(resizeActiveTerminal, 80);
    });
    observer.observe(hostRef.current);

    return () => {
      disposed = true;
      observer.disconnect();
      if (resizeTimerRef.current) {
        clearTimeout(resizeTimerRef.current);
        resizeTimerRef.current = null;
      }
      fitAddonRef.current = null;
      xtermRef.current = null;
      term.dispose();
    };
  }, [appendSystemLine, isOpen, resizeActiveTerminal, t]);

  useEffect(() => {
    if (!isOpen) return;
    let disposed = false;
    let revision = 0;
    const update = async () => {
      const requestedRevision = ++revision;
      const light = document.documentElement.dataset.themeMode === 'light' || theme === 'light' || theme === 'bloom';
      const appearance = await api.getTerminalAppearance(session?.shell ?? selectedShell, light).catch(() => null);
      const term = xtermRef.current;
      if (disposed || !term || requestedRevision !== revision) return;
      const colors = { ...readTerminalTheme(), ...appearance?.theme };
      const fontFamily = appearance?.fontFamily
        ? `${JSON.stringify(appearance.fontFamily)}, ${TERMINAL_FONT_FALLBACK}` : readTerminalFont();
      term.options.theme = colors;
      term.options.fontFamily = fontFamily;
      term.options.fontSize = appearance?.fontSize ?? 14;
      term.options.fontWeight = Math.min(900, Math.max(100, Math.round((appearance?.fontWeight ?? 400) / 100) * 100)) as FontWeight;
      term.options.cursorStyle = appearance?.cursorStyle ?? 'bar';
      if (hostRef.current) {
        hostRef.current.style.backgroundColor = colors.background;
        hostRef.current.style.fontFamily = fontFamily;
        hostRef.current.style.fontSize = `${term.options.fontSize}px`;
        hostRef.current.dataset.appearanceSource = appearance?.source ?? 'Nexa';
      }
      await document.fonts.load(`${term.options.fontSize}px ${fontFamily}`).catch(() => {});
      if (!disposed) { resizeActiveTerminal(); term.refresh(0, term.rows - 1); }
    };
    void update();
    window.addEventListener('focus', update);
    const observer = new MutationObserver(() => void update());
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'class', 'data-theme-mode'] });
    return () => { disposed = true; observer.disconnect(); window.removeEventListener('focus', update); };
  }, [isOpen, selectedShell, session?.shell, theme, resizeActiveTerminal]);

  useEffect(() => {
    let cancelled = false;
    restoringRef.current = true;
    setIsRestoring(true);
    sessionIdRef.current = null;
    setSession(null);
    setSelection('');
    setStatus('idle');
    outputBufferRef.current = '';
    xtermRef.current?.reset();

    const restore = async () => {
      try {
        const sessions = await api.listTerminalSessions();
        if (cancelled) return;
        const matching = conversationId
          ? sessions.filter((item) => item.conversationId === conversationId)
          : sessions.filter((item) => !item.conversationId);
        setAvailableSessions(matching);
        if (!conversationId) return;
        const active = await api.activeTerminalSession(conversationId);
        if (!cancelled && active) {
          await attachSession(active, () => cancelled);
        }
      } catch (restoreError) {
        if (!cancelled) {
          console.warn('[TerminalDock] session restore failed:', restoreError);
        }
      } finally {
        restoringRef.current = false;
        if (!cancelled) {
          setIsRestoring(false);
        }
      }
    };
    void restore();
    return () => {
      cancelled = true;
    };
  }, [attachSession, conversationId]);

  useEffect(() => {
    if (!isOpen || isRestoring || restoringRef.current) return;
    if (sessionIdRef.current || status !== 'idle') return;
    void startSession(selectedShell);
  }, [isOpen, isRestoring, selectedShell, startSession, status]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.repeat || event.altKey || event.shiftKey || event.isComposing) {
        return;
      }
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 'j') {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      toggleTerminalPanel();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleTerminalPanel]);

  useEffect(() => {
    window.addEventListener(TERMINAL_TOGGLE_EVENT, toggleTerminalPanel);
    return () => window.removeEventListener(TERMINAL_TOGGLE_EVENT, toggleTerminalPanel);
  }, [toggleTerminalPanel]);

  useEffect(() => {
    const handler = () => setIsOpen(true);
    window.addEventListener(TERMINAL_OPEN_EVENT, handler);
    return () => window.removeEventListener(TERMINAL_OPEN_EVENT, handler);
  }, []);

  const handleShellChange = (value: TerminalShell) => {
    setSelectedShell(value);
    if (isOpen) {
      void startSession(value);
    }
  };

  const handleSessionChange = useCallback((sessionId: string) => {
    const next = availableSessions.find((item) => item.id === sessionId);
    if (!next) return;
    void (async () => {
      const active = conversationId
        ? await api.bindTerminalSession(next.id, conversationId)
        : next;
      await attachSession(active);
    })().catch((switchError) => {
      const message = switchError instanceof Error ? switchError.message : String(switchError);
      setError(message);
      setStatus('error');
    });
  }, [attachSession, availableSessions, conversationId]);

  const restartActiveSession = useCallback(async () => {
    await closeActiveSession('idle');
    await startSession(selectedShell);
  }, [closeActiveSession, selectedShell, startSession]);

  const handleOpen = () => {
    setIsOpen(true);
  };

  const handleCopySelection = useCallback(() => {
    if (!selection) return;
    void writeClipboardText(selection)
      .then(() => toast.success(t('chat.terminalSelectionCopied')))
      .catch((copyError) => {
        const message = copyError instanceof Error ? copyError.message : String(copyError);
        appendSystemLine(message);
      });
  }, [appendSystemLine, selection, t]);

  const handleSendSelectionToAgent = useCallback(() => {
    if (!selection || !session || !onSendSelectionToAgent) return;
    onSendSelectionToAgent({
      text: selection,
      outputTail: outputBufferRef.current.slice(-12_000),
      session,
    });
    xtermRef.current?.clearSelection();
    setSelection('');
  }, [onSendSelectionToAgent, selection, session]);

  const statusText = terminalStatusLabel(status);
  const panelHeight = isTall ? 'h-[42vh] min-h-72' : 'h-64 min-h-48';
  const shouldRenderDock = isOpen || status !== 'idle' || Boolean(session) || Boolean(error);

  useEffect(() => {
    onRenderedChange?.(shouldRenderDock);
  }, [onRenderedChange, shouldRenderDock]);

  useEffect(() => () => {
    onRenderedChange?.(false);
  }, [onRenderedChange]);

  if (!shouldRenderDock) {
    return null;
  }

  return (
    <div
      className="shrink-0 border-t border-border/60 bg-surface-1/90"
      data-testid="terminal-dock"
      data-theme-surface="transparent"
    >
      <div
        className="flex min-h-10 flex-wrap items-center gap-2 px-3 py-1.5"
        data-testid="terminal-dock-header"
        data-theme-surface="chrome"
      >
        <button
          type="button"
          onClick={isOpen ? toggleTerminalPanel : handleOpen}
          className="inline-flex h-8 min-w-0 items-center gap-2 rounded-md border border-border/55 bg-surface-2/70 px-2.5 text-xs font-medium text-text-primary transition-colors hover:border-border-hover hover:bg-surface-3"
          aria-expanded={isOpen}
          aria-keyshortcuts="Control+J Meta+J"
          title="Terminal (Ctrl+J / Cmd+J)"
        >
          {isOpen ? <ChevronDown className="h-3.5 w-3.5 shrink-0" /> : <PanelBottomOpen className="h-3.5 w-3.5 shrink-0" />}
          <TerminalSquare className="h-3.5 w-3.5 shrink-0 text-accent" />
          <span className="truncate">Terminal</span>
        </button>
        <span className="inline-flex h-8 items-center gap-1.5 rounded-md border border-border/45 bg-surface-0/55 px-2 text-[11px] text-text-secondary">
          {status === 'starting' ? (
            <Loader2 className="h-3 w-3 animate-spin text-accent" />
          ) : (
            <span className={`h-2 w-2 rounded-full ${terminalStatusClass(status)}`} />
          )}
          {statusText}
        </span>
        <NexaSelect
          value={selectedShell}
          onChange={(event) => handleShellChange(event.target.value as TerminalShell)}
          className="h-8 rounded-md border border-border/55 bg-surface-0 px-2 text-xs text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
          aria-label="Terminal shell"
        >
          {SHELL_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </NexaSelect>
        {availableSessions.length > 1 && (
          <NexaSelect
            value={session?.id ?? ''}
            onChange={(event) => handleSessionChange(event.target.value)}
            className="h-8 max-w-44 rounded-md border border-border/55 bg-surface-0 px-2 text-xs text-text-primary outline-none transition-colors hover:border-border-hover focus:border-accent"
            aria-label="Active terminal session"
            title="Active terminal session"
          >
            {availableSessions.map((item, index) => (
              <option key={item.id} value={item.id}>
                {`${index + 1}: ${item.shell}${item.processId ? ` #${item.processId}` : ''}`}
              </option>
            ))}
          </NexaSelect>
        )}
        {session && (
          <div className="min-w-0 flex-1 truncate text-[11px] text-text-tertiary">
            {session.shell}
            {session.processId ? ` #${session.processId}` : ''} · {session.cwd}
          </div>
        )}
        {!session && error && (
          <div className="min-w-0 flex-1 truncate text-[11px] text-danger">
            {error}
          </div>
        )}
        {session && conversationId && (
          <span
            data-testid="terminal-agent-link"
            className="inline-flex h-7 max-w-48 items-center gap-1 rounded-md border border-accent/20 bg-accent/10 px-2 text-[10px] text-accent"
            title={t('chat.terminalLinkedToAgent', { agent: agentLabel || conversationId })}
          >
            <Link2 className="h-3 w-3 shrink-0" />
            <span className="truncate">
              {t('chat.terminalLinkedToAgent', { agent: agentLabel || conversationId })}
            </span>
          </span>
        )}
        {selection && (
          <div className="inline-flex h-8 items-center gap-1 rounded-md border border-info/25 bg-info/10 px-1.5">
            <span className="px-1 text-[10px] text-info">
              {t('chat.terminalSelectionCount', { count: selection.length })}
            </span>
            <Button
              variant="ghost"
              size="sm"
              iconOnly
              icon={<Copy size={13} />}
              aria-label={t('chat.terminalCopySelection')}
              title={t('chat.terminalCopySelection')}
              onClick={handleCopySelection}
            />
            <Button
              variant="ghost"
              size="sm"
              iconOnly
              icon={<MessageSquareText size={13} />}
              data-testid="terminal-send-selection"
              aria-label={t('chat.terminalSendSelection')}
              title={t('chat.terminalSendSelection')}
              onClick={handleSendSelectionToAgent}
              disabled={!onSendSelectionToAgent}
            />
          </div>
        )}
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            icon={<Plus size={14} />}
            aria-label="Start terminal"
            title="Start terminal"
            onClick={() => void startSession(selectedShell)}
            disabled={status === 'starting'}
          />
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            icon={<RotateCcw size={14} />}
            aria-label="Restart terminal"
            title="Restart terminal"
            onClick={() => void restartActiveSession()}
            disabled={status === 'starting'}
          />
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            icon={isTall ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            aria-label={isTall ? 'Shrink terminal' : 'Grow terminal'}
            title={isTall ? 'Shrink terminal' : 'Grow terminal'}
            onClick={() => setIsTall((value) => !value)}
          />
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            icon={<Power size={14} />}
            aria-label="Stop terminal"
            title="Stop terminal"
            onClick={() => void closeActiveSession('exited')}
            disabled={!sessionIdRef.current}
          />
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            icon={<X size={14} />}
            aria-label="Close terminal"
            title="Collapse terminal without stopping it"
            onClick={closeTerminalDock}
          />
        </div>
      </div>
      {isOpen && (
        <div
          className={`border-t border-border/50 bg-surface-0 ${panelHeight}`}
          data-theme-surface="overlay"
          data-theme-overlay-tone="canvas"
        >
          <div ref={hostRef} data-testid="terminal-screen" className="h-full w-full overflow-hidden px-2 py-2" />
        </div>
      )}
    </div>
  );
}
