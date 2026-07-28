/**
 * Pure helpers for turning raw microphone samples into a bar waveform.
 *
 * Kept free of DOM and Web Audio types so the shaping rules can be tested
 * without a browser; the analyser plumbing lives in `useMicrophoneAnalyser`.
 */

/** Below this RMS the input is treated as silence rather than noise floor. */
const SILENCE_FLOOR = 0.004;

/** Perceptual boost so quiet speech still moves the bars. */
const LOUDNESS_EXPONENT = 0.55;

/** Smallest visible height so an active mic never looks disconnected. */
const MIN_BAR = 0.06;

/**
 * Reduce time-domain samples (-1..1) into `barCount` normalized 0..1 heights.
 *
 * Buckets are contiguous so the bars read left-to-right as a short slice of
 * recent audio rather than a static level meter.
 */
export function computeWaveformBars(samples: ArrayLike<number>, barCount: number): number[] {
  const bars = Math.max(1, Math.floor(barCount));
  if (samples.length === 0) return new Array<number>(bars).fill(0);

  const result = new Array<number>(bars);
  for (let bar = 0; bar < bars; bar += 1) {
    const start = Math.floor((bar * samples.length) / bars);
    const end = Math.max(start + 1, Math.floor(((bar + 1) * samples.length) / bars));
    let sum = 0;
    for (let index = start; index < end && index < samples.length; index += 1) {
      const sample = samples[index];
      sum += sample * sample;
    }
    const rms = Math.sqrt(sum / (end - start));
    result[bar] = rms <= SILENCE_FLOOR ? 0 : Math.min(1, Math.pow(rms, LOUDNESS_EXPONENT));
  }
  return result;
}

/**
 * Ease a new frame toward the previous one so the bars glide instead of
 * flickering, while still dropping quickly when the speaker stops.
 */
export function smoothWaveformBars(previous: number[], next: number[], attack = 0.55): number[] {
  if (previous.length !== next.length) return next.slice();
  const release = attack * 0.5;
  return next.map((value, index) => {
    const prior = previous[index];
    const factor = value > prior ? attack : release;
    return prior + (value - prior) * factor;
  });
}

/** Clamp bar heights into the drawable range used by the waveform component. */
export function toBarHeights(bars: number[], minimum = MIN_BAR): number[] {
  return bars.map((value) => Math.min(1, Math.max(minimum, value)));
}
