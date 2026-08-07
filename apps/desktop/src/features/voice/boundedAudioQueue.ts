export interface AudioQueueTelemetry {
  queuedBytes: number;
  inFlightChunks: number;
  maxQueueDepth: number;
  maxBufferedBytes: number;
  acceptedChunks: number;
  sentChunks: number;
  rejectedChunks: number;
}

export interface BoundedAudioQueueOptions {
  maxChunks?: number;
  maxBytes?: number;
  maxChunkBytes?: number;
  onRejected?: (telemetry: AudioQueueTelemetry) => void;
}

type FlushWaiter = {
  resolve: () => void;
  reject: (error: Error) => void;
};

const DEFAULT_MAX_CHUNKS = 8;
const DEFAULT_MAX_BYTES = 2 * 1024 * 1024;
const DEFAULT_MAX_CHUNK_BYTES = 256 * 1024;

/**
 * Sequential, byte-bounded renderer-to-native audio upload queue.
 *
 * The audio callback only performs a bounded enqueue. Native IPC is awaited by
 * the drain loop, so a slow provider cannot create an unbounded Promise chain.
 */
export class BoundedAudioUploadQueue {
  private readonly chunks: Uint8Array[] = [];
  private readonly maxChunks: number;
  private readonly maxBytes: number;
  private readonly maxChunkBytes: number;
  private readonly onRejected?: (telemetry: AudioQueueTelemetry) => void;
  private readonly waiters: FlushWaiter[] = [];
  private queuedBytes = 0;
  private inFlightBytes = 0;
  private draining = false;
  private terminalError: Error | null = null;
  private telemetry: AudioQueueTelemetry = {
    queuedBytes: 0,
    inFlightChunks: 0,
    maxQueueDepth: 0,
    maxBufferedBytes: 0,
    acceptedChunks: 0,
    sentChunks: 0,
    rejectedChunks: 0,
  };

  constructor(
    private readonly sendChunk: (chunk: Uint8Array) => Promise<void>,
    options: BoundedAudioQueueOptions = {},
  ) {
    this.maxChunks = options.maxChunks ?? DEFAULT_MAX_CHUNKS;
    this.maxBytes = options.maxBytes ?? DEFAULT_MAX_BYTES;
    this.maxChunkBytes = options.maxChunkBytes ?? DEFAULT_MAX_CHUNK_BYTES;
    this.onRejected = options.onRejected;
    if (this.maxChunks < 1 || this.maxBytes < 1 || this.maxChunkBytes < 1) {
      throw new Error('Audio queue limits must be positive');
    }
  }

  enqueue(chunk: Uint8Array): boolean {
    if (chunk.byteLength === 0) return true;
    const bufferedBytes = this.queuedBytes + this.inFlightBytes;
    const bufferedChunks = this.chunks.length + (this.inFlightBytes > 0 ? 1 : 0);
    if (
      this.terminalError
      || chunk.byteLength > this.maxChunkBytes
      || bufferedChunks >= this.maxChunks
      || bufferedBytes + chunk.byteLength > this.maxBytes
    ) {
      this.telemetry.rejectedChunks += 1;
      this.onRejected?.(this.snapshot());
      return false;
    }

    this.chunks.push(chunk);
    this.queuedBytes += chunk.byteLength;
    this.telemetry.acceptedChunks += 1;
    this.updateTelemetry();
    void this.drain();
    return true;
  }

  async flush(): Promise<void> {
    if (this.terminalError) throw this.terminalError;
    if (!this.draining && this.chunks.length === 0) return;
    return new Promise<void>((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
  }

  cancel(reason = 'Realtime audio upload cancelled'): void {
    this.terminalError = new Error(reason);
    this.chunks.length = 0;
    this.queuedBytes = 0;
    this.updateTelemetry();
    this.settleWaiters();
  }

  snapshot(): AudioQueueTelemetry {
    return { ...this.telemetry };
  }

  private updateTelemetry(): void {
    this.telemetry.queuedBytes = this.queuedBytes;
    this.telemetry.inFlightChunks = this.inFlightBytes > 0 ? 1 : 0;
    this.telemetry.maxQueueDepth = Math.max(
      this.telemetry.maxQueueDepth,
      this.chunks.length + this.telemetry.inFlightChunks,
    );
    this.telemetry.maxBufferedBytes = Math.max(
      this.telemetry.maxBufferedBytes,
      this.queuedBytes + this.inFlightBytes,
    );
  }

  private async drain(): Promise<void> {
    if (this.draining || this.terminalError) return;
    this.draining = true;
    try {
      while (this.chunks.length > 0 && !this.terminalError) {
        const chunk = this.chunks.shift()!;
        this.queuedBytes -= chunk.byteLength;
        this.inFlightBytes = chunk.byteLength;
        this.updateTelemetry();
        await this.sendChunk(chunk);
        this.telemetry.sentChunks += 1;
        this.inFlightBytes = 0;
        this.updateTelemetry();
      }
    } catch (error) {
      this.terminalError = error instanceof Error ? error : new Error(String(error));
      this.chunks.length = 0;
      this.queuedBytes = 0;
      this.inFlightBytes = 0;
      this.updateTelemetry();
    } finally {
      this.draining = false;
      this.settleWaiters();
    }
  }

  private settleWaiters(): void {
    if (this.draining || this.chunks.length > 0) return;
    const waiters = this.waiters.splice(0);
    for (const waiter of waiters) {
      if (this.terminalError) waiter.reject(this.terminalError);
      else waiter.resolve();
    }
  }
}
