import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { useState, useEffect, useCallback } from 'react';

interface UpdateState {
  status: 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error' | 'up-to-date';
  version?: string;
  notes?: string;
  progress?: number;
  error?: string;
  errorCode?: string | number | null;
  errorDetail?: { stack?: string };
}

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

  const checkForUpdate = useCallback(async () => {
    setState({ status: 'checking' });
    try {
      const update = await check();
      if (update) {
        setState({
          status: 'available',
          version: update.version,
          notes: update.body ?? undefined,
        });
        return update;
      } else {
        setState({ status: 'up-to-date' });
        return null;
      }
    } catch (e) {
      setState({ status: 'error', ...extractError(e) });
      return null;
    }
  }, []);

  const downloadAndInstall = useCallback(async () => {
    try {
      const update = await check();
      if (!update) return;

      setState(prev => ({ ...prev, status: 'downloading', progress: 0 }));

      let downloaded = 0;
      let contentLength = 0;

      await update.downloadAndInstall((event) => {
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
      });

      await relaunch();
    } catch (e) {
      setState(prev => ({ ...prev, status: 'error', ...extractError(e) }));
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
