const PROCESSOR_NAME = 'nexa-voice-pcm-processor';

class VoicePcmProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const processorOptions = options?.processorOptions ?? {};
    this.targetSampleRate = processorOptions.targetSampleRate;
    this.chunkFrames = processorOptions.chunkFrames;
    this.maxCredits = processorOptions.maxCredits ?? 4;
    this.maxPendingChunks = processorOptions.maxPendingChunks ?? 8;
    if (!Number.isFinite(this.targetSampleRate) || this.targetSampleRate <= 0) {
      throw new Error('targetSampleRate must be positive');
    }
    if (!Number.isInteger(this.chunkFrames) || this.chunkFrames <= 0) {
      throw new Error('chunkFrames must be a positive integer');
    }
    if (!Number.isInteger(this.maxCredits) || this.maxCredits <= 0) {
      throw new Error('maxCredits must be a positive integer');
    }
    if (!Number.isInteger(this.maxPendingChunks) || this.maxPendingChunks <= 0) {
      throw new Error('maxPendingChunks must be a positive integer');
    }

    this.sourceStep = sampleRate / this.targetSampleRate;
    this.nextSourcePosition = 0;
    this.previousSample = null;
    this.currentChunk = new Int16Array(this.chunkFrames);
    this.currentChunkOffset = 0;
    this.credits = this.maxCredits;
    this.pendingChunks = [];
    this.paused = false;
    this.stopped = false;
    this.overflowed = false;
    this.flushRequestId = null;

    this.port.onmessage = (event) => {
      const message = event.data ?? {};
      if (message.type === 'ack') {
        this.credits = Math.min(this.maxCredits, this.credits + 1);
        this.flushPendingChunks();
        this.maybeCompleteFlush();
      } else if (message.type === 'pause') {
        this.paused = true;
      } else if (message.type === 'resume') {
        this.paused = false;
      } else if (message.type === 'flush') {
        if (message.pauseAfter) this.paused = true;
        if (message.stopAfter) this.stopped = true;
        this.flushRequestId = message.requestId;
        this.flushPartialChunk();
        this.flushPendingChunks();
        this.maybeCompleteFlush();
      } else if (message.type === 'stop') {
        this.stopped = true;
      }
    };
  }

  process(inputs) {
    if (this.paused || this.stopped || this.overflowed) return true;
    const samples = inputs[0]?.[0];
    if (!samples || samples.length === 0) return true;

    const hasPreviousSample = this.previousSample !== null;
    const inputLength = samples.length + (hasPreviousSample ? 1 : 0);
    const sampleAt = (index) => {
      if (hasPreviousSample) {
        return index === 0 ? this.previousSample : samples[index - 1];
      }
      return samples[index];
    };

    while (this.nextSourcePosition < inputLength - 1) {
      const leftIndex = Math.floor(this.nextSourcePosition);
      const fraction = this.nextSourcePosition - leftIndex;
      const left = sampleAt(leftIndex);
      const right = sampleAt(leftIndex + 1);
      this.appendSample(left + (right - left) * fraction);
      if (this.overflowed) break;
      this.nextSourcePosition += this.sourceStep;
    }

    this.nextSourcePosition -= inputLength - 1;
    this.previousSample = samples[samples.length - 1];
    return true;
  }

  appendSample(sample) {
    const clipped = Math.max(-1, Math.min(1, sample));
    this.currentChunk[this.currentChunkOffset] = clipped <= -1
      ? -0x8000
      : clipped >= 1
        ? 0x7fff
        : Math.round(clipped * 0x8000);
    this.currentChunkOffset += 1;
    if (this.currentChunkOffset === this.currentChunk.length) {
      const buffer = this.currentChunk.buffer;
      this.currentChunk = new Int16Array(this.chunkFrames);
      this.currentChunkOffset = 0;
      this.emitChunk(buffer);
    }
  }

  flushPartialChunk() {
    if (this.currentChunkOffset === 0) return;
    const partial = this.currentChunk.slice(0, this.currentChunkOffset);
    this.currentChunk = new Int16Array(this.chunkFrames);
    this.currentChunkOffset = 0;
    this.emitChunk(partial.buffer);
  }

  emitChunk(buffer) {
    if (this.credits > 0) {
      this.credits -= 1;
      this.port.postMessage({ type: 'pcm', buffer }, [buffer]);
      return;
    }
    if (this.pendingChunks.length < this.maxPendingChunks) {
      this.pendingChunks.push(buffer);
      return;
    }
    this.overflowed = true;
    this.port.postMessage({
      type: 'overflow',
      pendingChunks: this.pendingChunks.length,
      maxPendingChunks: this.maxPendingChunks,
    });
  }

  flushPendingChunks() {
    while (this.credits > 0 && this.pendingChunks.length > 0) {
      const buffer = this.pendingChunks.shift();
      this.credits -= 1;
      this.port.postMessage({ type: 'pcm', buffer }, [buffer]);
    }
  }

  maybeCompleteFlush() {
    if (
      this.flushRequestId === null
      || this.pendingChunks.length > 0
      || this.credits !== this.maxCredits
    ) {
      return;
    }
    const requestId = this.flushRequestId;
    this.flushRequestId = null;
    this.port.postMessage({ type: 'flushed', requestId });
  }
}

registerProcessor(PROCESSOR_NAME, VoicePcmProcessor);
