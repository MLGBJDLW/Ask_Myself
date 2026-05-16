export type StreamTimeoutHandle = ReturnType<typeof setTimeout>;

export interface StreamWatchdogState {
  _timeoutId: StreamTimeoutHandle | null;
}

export function resolveStreamTimeoutMs(): number {
  if (typeof window === 'undefined') return 120_000;
  const override = (window as Window & { __ASK_STREAM_TIMEOUT_MS__?: unknown }).__ASK_STREAM_TIMEOUT_MS__;
  return typeof override === 'number' && Number.isFinite(override) && override > 0
    ? override
    : 120_000;
}

export const STREAM_TIMEOUT_MS = resolveStreamTimeoutMs();

export function clearStreamWatchdog(state: StreamWatchdogState): void {
  if (state._timeoutId) clearTimeout(state._timeoutId);
  state._timeoutId = null;
}

export function armStreamWatchdog(
  state: StreamWatchdogState,
  onTimeout: () => void,
  timeoutMs = STREAM_TIMEOUT_MS,
): void {
  clearStreamWatchdog(state);
  state._timeoutId = setTimeout(() => {
    state._timeoutId = null;
    onTimeout();
  }, timeoutMs);
}
