import type { TaskResumeCheckpoint } from '../types/workflows';

export function resumableCheckpointForTask(
  runId: string,
  status: string,
  checkpoints: readonly TaskResumeCheckpoint[],
): TaskResumeCheckpoint | null {
  if (status !== 'paused') return null;

  const latestCheckpoint = checkpoints[0] ?? null;
  return latestCheckpoint?.runId === runId ? latestCheckpoint : null;
}

export function invalidateTaskCheckpointLoadState<
  T extends {
    loaded: Set<string>;
    resumeCheckpoints?: TaskResumeCheckpoint[];
  },
>(
  cache: Map<string, T>,
  autoLoadedRuns: Set<string>,
  runId: string,
): void {
  const cached = cache.get(runId);
  if (cached) {
    const loaded = new Set(cached.loaded);
    loaded.delete('checkpoint');
    cache.set(runId, {
      ...cached,
      loaded,
      resumeCheckpoints: undefined,
    });
  }
  autoLoadedRuns.delete(runId);
}
