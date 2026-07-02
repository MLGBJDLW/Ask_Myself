import { invoke } from '@tauri-apps/api/core';
import { Update as TauriUpdate, type Update as TauriUpdateInstance } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { useState, useEffect, useCallback } from 'react';

export const UPDATE_SOURCES = ['github'] as const;
export type UpdateSource = typeof UPDATE_SOURCES[number];

interface UpdateState {
  status: 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error' | 'up-to-date';
  source: UpdateSource;
  version?: string;
  notes?: string;
  progress?: number;
  error?: string;
  errorCode?: string | number | null;
  errorDetail?: { stack?: string };
  errorStage?: 'check' | 'download' | 'install';
  lastCheckedAt?: string;
}

interface TauriUpdateMetadata {
  rid: number;
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
}

const UPDATE_SOURCE_STORAGE_KEY = 'nexa-update-source';
const DEFAULT_UPDATE_SOURCE: UpdateSource = 'github';
const UPDATE_CHECK_TIMEOUT_MS = 90_000;
const UPDATE_DOWNLOAD_TIMEOUT_MS = 600_000;
const GITHUB_RELEASES_API_URL = 'https://api.github.com/repos/MLGBJDLW/Nexa/releases?per_page=50';

interface GitHubRelease {
  tag_name?: string;
  name?: string | null;
  body?: string | null;
  draft?: boolean;
  prerelease?: boolean;
  published_at?: string | null;
}

function isUpdateSource(value: string | null): value is UpdateSource {
  return value === 'github';
}

function readStoredUpdateSource(): UpdateSource {
  if (typeof window === 'undefined') return DEFAULT_UPDATE_SOURCE;
  try {
    const value = window.localStorage.getItem(UPDATE_SOURCE_STORAGE_KEY);
    return isUpdateSource(value) ? value : DEFAULT_UPDATE_SOURCE;
  } catch {
    return DEFAULT_UPDATE_SOURCE;
  }
}

function persistUpdateSource(source: UpdateSource) {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(UPDATE_SOURCE_STORAGE_KEY, source);
  } catch {
    // Ignore storage failures; the current session still uses the selected source.
  }
}

async function checkUpdateFromSource(source: UpdateSource): Promise<TauriUpdateInstance | null> {
  const metadata = await invoke<TauriUpdateMetadata | null>('check_update_from_source_cmd', {
    source,
    timeout: UPDATE_CHECK_TIMEOUT_MS,
  });
  return metadata ? new TauriUpdate(metadata) : null;
}

function normalizeReleaseVersion(value: string | null | undefined): string {
  const match = (value ?? '').trim().match(/(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)/);
  return match?.[1] ?? '';
}

function compareReleaseVersions(a: string, b: string): number {
  const [aCore, aPre = ''] = a.split('-', 2);
  const [bCore, bPre = ''] = b.split('-', 2);
  const aParts = aCore.split('.').map(part => Number.parseInt(part, 10) || 0);
  const bParts = bCore.split('.').map(part => Number.parseInt(part, 10) || 0);
  for (let index = 0; index < 3; index += 1) {
    const diff = (aParts[index] ?? 0) - (bParts[index] ?? 0);
    if (diff !== 0) return diff;
  }
  if (!aPre && bPre) return 1;
  if (aPre && !bPre) return -1;
  return aPre.localeCompare(bPre);
}

function releaseDisplayName(release: GitHubRelease, version: string): string {
  const name = release.name?.trim();
  if (name) return name;
  const tag = release.tag_name?.trim();
  if (tag) return tag;
  return `v${version}`;
}

async function fetchGithubReleaseNotesBetween(
  currentVersion: string,
  latestVersion: string,
  fallbackBody: string | undefined,
): Promise<string | undefined> {
  const current = normalizeReleaseVersion(currentVersion);
  const latest = normalizeReleaseVersion(latestVersion);
  if (!current || !latest || compareReleaseVersions(current, latest) >= 0) {
    return fallbackBody;
  }

  const response = await fetch(GITHUB_RELEASES_API_URL, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!response.ok) {
    throw new Error(`GitHub releases request failed (${response.status})`);
  }

  const releases = await response.json() as GitHubRelease[];
  const selected = releases
    .map((release) => ({
      release,
      version: normalizeReleaseVersion(release.tag_name ?? release.name ?? ''),
    }))
    .filter(({ release, version }) => (
      !release.draft &&
      version &&
      compareReleaseVersions(version, current) > 0 &&
      compareReleaseVersions(version, latest) <= 0
    ))
    .sort((a, b) => compareReleaseVersions(a.version, b.version));

  if (selected.length === 0) return fallbackBody;

  return selected
    .map(({ release, version }) => {
      const heading = `## ${releaseDisplayName(release, version)}`;
      const body = (release.body ?? '').trim() || '_No release notes provided._';
      return `${heading}\n\n${body}`;
    })
    .join('\n\n---\n\n');
}

async function resolveUpdateNotes(
  source: UpdateSource,
  update: TauriUpdateInstance,
): Promise<string | undefined> {
  if (source !== 'github') return update.body ?? undefined;
  try {
    return await fetchGithubReleaseNotesBetween(
      update.currentVersion,
      update.version,
      update.body ?? undefined,
    );
  } catch (error) {
    console.warn('[useUpdater] failed to load ranged release notes:', error);
    return update.body ?? undefined;
  }
}

let sharedSource = readStoredUpdateSource();
let sharedState: UpdateState = { status: 'idle', source: sharedSource };
let sharedUpdate: TauriUpdateInstance | null = null;
let autoCheckStarted = false;
const listeners = new Set<(state: UpdateState) => void>();

function setSharedState(next: UpdateState | ((prev: UpdateState) => UpdateState)) {
  sharedState = typeof next === 'function'
    ? (next as (prev: UpdateState) => UpdateState)(sharedState)
    : next;
  for (const listener of listeners) {
    listener(sharedState);
  }
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
  const [state, setState] = useState<UpdateState>(sharedState);

  const setUpdateSource = useCallback((source: UpdateSource) => {
    if (source === sharedSource) return;
    sharedSource = source;
    persistUpdateSource(source);
    sharedUpdate = null;
    setSharedState({ status: 'idle', source });
  }, []);

  const checkForUpdate = useCallback(async (sourceOverride?: UpdateSource) => {
    const source = sourceOverride ?? sharedSource;
    setSharedState({ status: 'checking', source });
    try {
      const update = await checkUpdateFromSource(source);
      const lastCheckedAt = new Date().toISOString();
      if (source !== sharedSource) {
        return update;
      }
      if (update) {
        sharedUpdate = update;
        const notes = await resolveUpdateNotes(source, update);
        if (source !== sharedSource) {
          return update;
        }
        setSharedState({
          status: 'available',
          source,
          version: update.version,
          notes,
          lastCheckedAt,
        });
        return update;
      } else {
        sharedUpdate = null;
        setSharedState({ status: 'up-to-date', source, lastCheckedAt });
        return null;
      }
    } catch (e) {
      if (source !== sharedSource) {
        return null;
      }
      const msg = e instanceof Error ? e.message : String(e);
      // Graceful fallback: missing release manifest (404) → treat as up-to-date
      if (/\b404\b|Not Found/i.test(msg)) {
        sharedUpdate = null;
        setSharedState({ status: 'up-to-date', source, lastCheckedAt: new Date().toISOString() });
        return null;
      }
      setSharedState({ status: 'error', source, errorStage: 'check', lastCheckedAt: new Date().toISOString(), ...extractError(e) });
      return null;
    }
  }, []);

  const downloadAndInstall = useCallback(async () => {
    let update = sharedUpdate;
    const source = sharedSource;
    if (!update) {
      try {
        update = await checkForUpdate(source);
        if (update) sharedUpdate = update;
      } catch (e) {
        setSharedState({ status: 'error', source, errorStage: 'check', lastCheckedAt: new Date().toISOString(), ...extractError(e) });
        return;
      }
      if (!update) return;
    }

    setSharedState(prev => ({
      ...prev,
      status: 'downloading',
      source,
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
                setSharedState(prev => ({
                  ...prev,
                  progress: Math.round((downloaded / contentLength) * 100),
                }));
              }
              break;
            case 'Finished':
              setSharedState(prev => ({ ...prev, status: 'ready', progress: 100 }));
              break;
          }
        },
        { timeout: UPDATE_DOWNLOAD_TIMEOUT_MS },
      );
      setSharedState(prev => ({ ...prev, status: 'ready', progress: 100 }));
    } catch (e) {
      setSharedState(prev => ({
        ...prev,
        status: 'error',
        progress: undefined,
        errorStage: 'download',
        ...extractError(e),
      }));
      return;
    }
  }, [checkForUpdate]);

  const restart = useCallback(async () => {
    try {
      await relaunch();
    } catch (e) {
      setSharedState(prev => ({
        ...prev,
        status: 'error',
        progress: undefined,
        source: sharedSource,
        errorStage: 'install',
        ...extractError(e),
      }));
    }
  }, []);

  useEffect(() => {
    listeners.add(setState);
    setState(sharedState);
    return () => {
      listeners.delete(setState);
    };
  }, []);

  useEffect(() => {
    if (!checkOnMount || autoCheckStarted) return;
    autoCheckStarted = true;
    let fired = false;
    const timer = setTimeout(() => {
      fired = true;
      void checkForUpdate();
    }, 5000);
    return () => {
      clearTimeout(timer);
      if (!fired) {
        autoCheckStarted = false;
      }
    };
  }, [checkOnMount, checkForUpdate]);

  return { ...state, setUpdateSource, checkForUpdate, downloadAndInstall, restart };
}
