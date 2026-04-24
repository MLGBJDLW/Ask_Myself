import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { useState, useEffect, useCallback, useRef } from 'react';

interface UpdateState {
  status: 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error' | 'up-to-date';
  version?: string;
  notes?: string;
  progress?: number;
  error?: string;
  errorCode?: string | number | null;
  errorDetail?: { stack?: string };
  errorStage?: 'check' | 'download' | 'install';
}

const UPDATE_CHECK_TIMEOUT_MS = 90_000;
const UPDATE_DOWNLOAD_TIMEOUT_MS = 600_000;

function extractError(e: unknown): { error: string; errorCode: string | number | null; errorDetail: { stack?: string } } {
  const errMsg = e instanceof Error ? e.message : String(e);
  const errCode = (e as { code?: string | number; status?: string | number } | null)?.code
    ?? (e as { code?: string | number; status?: string | number } | null)?.status
    ?? null;
  const errStack = e instanceof Error ? e.stack : undefined;
  return { error: errMsg, errorCode: errCode, errorDetail: { stack: errStack?.slice(0, 500) } };
}

export function useUpdater(checkOnMount = true) {
  const [state, setState] = useState<UpdateState>({ status: 'idle' });
  const updateRef = useRef<Update | null>(null);

  const checkForUpdate = useCallback(async () => {
    setState({ status: 'checking' });
    try {
      const update = await check({ timeout: UPDATE_CHECK_TIMEOUT_MS });
      if (update) {
        updateRef.current = update;
        setState({
          status: 'available',
          version: update.version,
          notes: update.body ?? undefined,
        });
        return update;
      } else {
        updateRef.current = null;
        setState({ status: 'up-to-date' });
        return null;
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      // Graceful fallback: missing release manifest (404) → treat as up-to-date
      if (/\b404\b|Not Found/i.test(msg)) {
        updateRef.current = null;
        setState({ status: 'up-to-date' });
        return null;
      }
      setState({ status: 'error', errorStage: 'check', ...extractError(e) });
      return null;
    }
  }, []);

  const downloadAndInstall = useCallback(async () => {
    let update = updateRef.current;
    if (!update) {
      try {
        update = await check({ timeout: UPDATE_CHECK_TIMEOUT_MS });
        if (update) updateRef.current = update;
      } catch (e) {
        setState({ status: 'error', errorStage: 'check', ...extractError(e) });
        return;
      }
      if (!update) return;
    }

    setState(prev => ({
      ...prev,
      status: 'downloading',
      progress: 0,
      error: undefined,
      errorCode: undefined,
      errorDetail: undefined,
      errorStage: undefined,
    }));

    let downloaded = 0;
    let contentLength = 0;

    try {
      await update.downloadAndInstall(
        (event) => {
          switch (event.event) {
            case 'Started':
              contentLength = event.data.contentLength ?? 0;
              break;
            case 'Progress':
              downloaded += event.data.chunkLength;
              if (contentLength > 0) {
                setState(prev => ({
                  ...prev,
                  progress: Math.round((downloaded / contentLength) * 100),
                }));
              }
              break;
            case 'Finished':
              setState(prev => ({ ...prev, status: 'ready', progress: 100 }));
              break;
          }
        },
        { timeout: UPDATE_DOWNLOAD_TIMEOUT_MS },
      );
    } catch (e) {
      setState(prev => ({
        ...prev,
        status: 'error',
        progress: undefined,
        errorStage: 'download',
        ...extractError(e),
      }));
      return;
    }

    try {
      await relaunch();
    } catch (e) {
      setState(prev => ({
        ...prev,
        status: 'error',
        progress: undefined,
        errorStage: 'install',
        ...extractError(e),
      }));
    }
  }, []);

  useEffect(() => {
    if (checkOnMount) {
      const timer = setTimeout(checkForUpdate, 5000);
      return () => clearTimeout(timer);
    }
  }, [checkOnMount, checkForUpdate]);

  return { ...state, checkForUpdate, downloadAndInstall };
}
