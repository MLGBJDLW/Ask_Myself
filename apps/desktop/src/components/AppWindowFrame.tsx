import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Copy, Minus, Square, X } from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from '../i18n';
import { Logo } from './Logo';

function hasNativeWindowRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } };
  }).__TAURI_INTERNALS__;
  return Boolean(internals?.metadata?.currentWindow?.label);
}

interface AppWindowFrameProps {
  children: ReactNode;
  area: 'home' | 'task';
}

export function AppWindowFrame({ children, area }: AppWindowFrameProps) {
  const { t } = useTranslation();
  const hasWindowRuntime = hasNativeWindowRuntime();
  const appWindow = useMemo(
    () => hasWindowRuntime ? getCurrentWindow() : null,
    [hasWindowRuntime],
  );
  const [isMaximized, setIsMaximized] = useState(false);

  const refreshMaximizedState = useCallback(async () => {
    if (!appWindow) return;
    try {
      setIsMaximized(await appWindow.isMaximized());
    } catch {
      // Browser previews do not expose native window state.
    }
  }, [appWindow]);

  useEffect(() => {
    if (!appWindow) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;
    const syncState = async () => {
      if (disposed) return;
      await refreshMaximizedState();
    };

    void syncState();
    void appWindow.onResized(() => {
      void syncState();
    }).then((stopListening) => {
      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
      }
    }).catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appWindow, refreshMaximizedState]);

  const minimize = useCallback(() => {
    if (!appWindow) return;
    void appWindow.minimize().catch(() => {});
  }, [appWindow]);

  const toggleMaximize = useCallback(async () => {
    if (!appWindow) return;
    try {
      await appWindow.toggleMaximize();
      await refreshMaximizedState();
    } catch {
      // Keep the web shell usable when no native window is attached.
    }
  }, [appWindow, refreshMaximizedState]);

  const close = useCallback(() => {
    if (!appWindow) return;
    void appWindow.close().catch(() => {});
  }, [appWindow]);

  return (
    <div
      className="app-window-frame relative isolate flex h-screen w-screen flex-col overflow-hidden bg-surface-0 text-text-primary"
      data-app-area={area}
    >
      <div className="app-theme-backdrop" aria-hidden="true" />
      <header
        className="app-titlebar relative z-[100] flex h-9 shrink-0 select-none items-stretch border-b border-border bg-surface-1/95"
        data-testid="app-titlebar"
      >
        <div
          className="flex min-w-0 flex-1 items-center gap-2 px-3"
          data-tauri-drag-region=""
          data-testid="app-titlebar-drag-region"
          onDoubleClick={() => void toggleMaximize()}
        >
          <Logo size={14} className="pointer-events-none shrink-0" />
          <span className="pointer-events-none truncate text-[11px] font-semibold tracking-[0.08em] text-text-secondary">
            {t('app.name')}
          </span>
          <span className="pointer-events-none h-1 w-1 rounded-full bg-accent/70 shadow-[0_0_8px_var(--color-accent)]" aria-hidden="true" />
        </div>

        <div className="flex shrink-0 items-stretch" data-testid="app-window-controls">
          <button
            type="button"
            aria-label={t('app.minimizeWindow')}
            title={t('app.minimizeWindow')}
            disabled={!hasWindowRuntime}
            onClick={minimize}
            className="app-window-control"
          >
            <Minus className="h-3.5 w-3.5" strokeWidth={1.6} />
          </button>
          <button
            type="button"
            aria-label={isMaximized ? t('app.restoreWindow') : t('app.maximizeWindow')}
            title={isMaximized ? t('app.restoreWindow') : t('app.maximizeWindow')}
            disabled={!hasWindowRuntime}
            onClick={() => void toggleMaximize()}
            className="app-window-control"
          >
            {isMaximized ? (
              <Copy className="h-3 w-3" strokeWidth={1.45} />
            ) : (
              <Square className="h-3 w-3" strokeWidth={1.45} />
            )}
          </button>
          <button
            type="button"
            aria-label={t('app.closeWindow')}
            title={t('app.closeWindow')}
            disabled={!hasWindowRuntime}
            onClick={close}
            className="app-window-control app-window-control-close"
          >
            <X className="h-3.5 w-3.5" strokeWidth={1.6} />
          </button>
        </div>
      </header>

      <div id="app-window-content" className="relative z-10 min-h-0 flex-1 overflow-hidden">{children}</div>
    </div>
  );
}
