import {
  LONG_STREAM_PRESENTATION_INTERVAL_MS,
  LONG_STREAM_THRESHOLD_CHARS,
  SHORT_STREAM_PRESENTATION_INTERVAL_MS,
  markdownPresentationInterval,
} from '../src/lib/streaming/markdownPresentation';

function assertEqual(actual: number, expected: number, message: string): void {
  if (actual !== expected) throw new Error(`${message}: expected ${expected}, received ${actual}`);
}

assertEqual(markdownPresentationInterval(0), SHORT_STREAM_PRESENTATION_INTERVAL_MS, 'empty stream');
assertEqual(
  markdownPresentationInterval(LONG_STREAM_THRESHOLD_CHARS - 1),
  SHORT_STREAM_PRESENTATION_INTERVAL_MS,
  'short stream boundary',
);
assertEqual(
  markdownPresentationInterval(LONG_STREAM_THRESHOLD_CHARS),
  LONG_STREAM_PRESENTATION_INTERVAL_MS,
  'long stream boundary',
);
assertEqual(
  markdownPresentationInterval(4 * LONG_STREAM_THRESHOLD_CHARS),
  LONG_STREAM_PRESENTATION_INTERVAL_MS,
  'large stream',
);

console.log('markdown presentation contracts passed');
