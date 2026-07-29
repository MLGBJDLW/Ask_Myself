/** Convert normalized Web Audio samples to signed, little-endian PCM16. */
export function float32ToPcm16(samples: Float32Array): Uint8Array {
  const output = new Uint8Array(samples.length * 2);
  const view = new DataView(output.buffer);
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index]));
    const pcm = sample <= -1
      ? -0x8000
      : sample >= 1
        ? 0x7fff
        : Math.round(sample * 0x8000);
    view.setInt16(index * 2, pcm, true);
  }
  return output;
}

/**
 * Stateful linear resampler for microphone chunks.
 *
 * The final sample from each input chunk is retained so interpolation and the
 * fractional source position continue seamlessly in the next chunk.
 */
export class StreamingPcm16Encoder {
  private readonly sourceStep: number;
  private nextSourcePosition = 0;
  private previousSample: number | null = null;

  constructor(sourceSampleRate: number, targetSampleRate = 24_000) {
    if (!Number.isFinite(sourceSampleRate) || sourceSampleRate <= 0) {
      throw new Error('sourceSampleRate must be positive');
    }
    if (!Number.isFinite(targetSampleRate) || targetSampleRate <= 0) {
      throw new Error('targetSampleRate must be positive');
    }
    this.sourceStep = sourceSampleRate / targetSampleRate;
  }

  encode(chunk: Float32Array): Uint8Array {
    if (chunk.length === 0) return new Uint8Array();

    const input = new Float32Array(chunk.length + (this.previousSample === null ? 0 : 1));
    let inputOffset = 0;
    if (this.previousSample !== null) {
      input[0] = this.previousSample;
      inputOffset = 1;
    }
    input.set(chunk, inputOffset);

    const output: number[] = [];
    while (this.nextSourcePosition < input.length - 1) {
      const leftIndex = Math.floor(this.nextSourcePosition);
      const fraction = this.nextSourcePosition - leftIndex;
      const left = input[leftIndex];
      const right = input[leftIndex + 1];
      output.push(left + (right - left) * fraction);
      this.nextSourcePosition += this.sourceStep;
    }

    this.nextSourcePosition -= input.length - 1;
    this.previousSample = input[input.length - 1];
    return float32ToPcm16(Float32Array.from(output));
  }
}
