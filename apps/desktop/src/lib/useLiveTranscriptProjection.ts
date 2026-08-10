import { useMemo, useRef } from 'react';

import {
  EMPTY_LIVE_TRANSCRIPT,
  projectLiveTranscript,
  type LiveTranscriptProjection,
  type LiveTranscriptSource,
} from './streaming/liveTranscriptProjection';

/**
 * Atomically projects answer and thinking on the render already scheduled by
 * StreamStore. This deliberately owns no timer or animation frame of its own:
 * the streaming store remains the single presentation clock.
 */
export function useLiveTranscriptProjection(
  source: LiveTranscriptSource,
): LiveTranscriptProjection {
  const currentRef = useRef(EMPTY_LIVE_TRANSCRIPT);
  return useMemo(
    () => {
      currentRef.current = projectLiveTranscript(currentRef.current, source);
      return currentRef.current;
    },
    [source.answer, source.thinking],
  );
}
