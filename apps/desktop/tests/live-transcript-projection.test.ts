// @ts-expect-error The contract runner intentionally omits Node ambient types.
import * as assert from 'node:assert/strict';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { test } from 'node:test';

import {
  EMPTY_LIVE_TRANSCRIPT,
  projectLiveTranscript,
} from '../src/lib/streaming/liveTranscriptProjection';

test('answer and thinking advance under one projection revision', () => {
  const projected = projectLiveTranscript(EMPTY_LIVE_TRANSCRIPT, {
    answer: 'answer',
    thinking: 'plan',
  });

  assert.deepEqual(projected, {
    answer: 'answer',
    thinking: 'plan',
    revision: 1,
  });
  assert.equal(
    projectLiveTranscript(projected, { answer: 'answer', thinking: 'plan' }),
    projected,
  );
});

test('an authoritative phase reset replaces both channels without stale text', () => {
  const projected = projectLiveTranscript(
    { answer: 'old answer', thinking: 'old thinking', revision: 4 },
    { answer: '', thinking: '' },
  );

  assert.deepEqual(projected, { answer: '', thinking: '', revision: 5 });
});
