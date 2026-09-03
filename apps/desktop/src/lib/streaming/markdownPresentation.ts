export const SHORT_STREAM_PRESENTATION_INTERVAL_MS = 50;
export const LONG_STREAM_PRESENTATION_INTERVAL_MS = 250;
export const LONG_STREAM_THRESHOLD_CHARS = 16 * 1024;

/**
 * Markdown parsing is substantially more expensive than applying text deltas.
 * Keep short answers responsive while preventing long answers from scheduling
 * a full parser/highlighter pass on every animation frame.
 */
export function markdownPresentationInterval(contentLength: number): number {
  return contentLength >= LONG_STREAM_THRESHOLD_CHARS
    ? LONG_STREAM_PRESENTATION_INTERVAL_MS
    : SHORT_STREAM_PRESENTATION_INTERVAL_MS;
}
