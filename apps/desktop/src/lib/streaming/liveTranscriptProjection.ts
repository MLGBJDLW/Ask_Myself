export interface LiveTranscriptProjection {
  answer: string;
  thinking: string;
  revision: number;
}

export interface LiveTranscriptSource {
  answer: string;
  thinking: string;
}

export const EMPTY_LIVE_TRANSCRIPT: LiveTranscriptProjection = {
  answer: '',
  thinking: '',
  revision: 0,
};

/**
 * Authoritative projection for one paint. Both visible channels advance in a
 * single revision so React never renders answer and thinking from different
 * stream clocks.
 */
export function projectLiveTranscript(
  current: LiveTranscriptProjection,
  source: LiveTranscriptSource,
): LiveTranscriptProjection {
  if (current.answer === source.answer && current.thinking === source.thinking) {
    return current;
  }
  return {
    answer: source.answer,
    thinking: source.thinking,
    revision: current.revision + 1,
  };
}
