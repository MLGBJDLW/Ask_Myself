import type {
  VoiceAudioSpoolDescriptor,
  VoiceAudioSpoolStarted,
} from '../../lib/api';
import { BoundedAudioUploadQueue, type AudioQueueTelemetry } from './boundedAudioQueue';

export interface VoiceSpoolTransport {
  append: (sessionId: string, sequence: number, chunk: Uint8Array) => Promise<unknown>;
  finish: (sessionId: string) => Promise<VoiceAudioSpoolDescriptor>;
  cancel: (sessionId: string) => Promise<void>;
}

export interface NativeVoiceSpoolOptions {
  maxChunks?: number;
  maxBufferedBytes?: number;
  maxBufferedDurationMs?: number;
  onBackpressure?: (telemetry: AudioQueueTelemetry) => void;
  onError?: (error: Error, telemetry: AudioQueueTelemetry) => void;
}

/**
 * Ordered, bounded renderer adapter for one native voice spool session.
 *
 * Native Rust owns the recording file and integrity metadata. This adapter
 * retains at most the configured queue bound and assigns sequence numbers only
 * when a chunk is dispatched, so IPC failures cannot create an unbounded
 * Promise chain or silently reorder audio.
 */
export class NativeVoiceSpoolUpload {
  readonly sessionId: string;
  private readonly queue: BoundedAudioUploadQueue;
  private accepting = true;
  private rejected = false;
  private nextSequence = 0;
  private finishPromise: Promise<VoiceAudioSpoolDescriptor> | null = null;
  private recoveryPromise: Promise<VoiceAudioSpoolDescriptor> | null = null;
  private cancelPromise: Promise<void> | null = null;

  constructor(
    started: VoiceAudioSpoolStarted,
    private readonly transport: VoiceSpoolTransport,
    options: NativeVoiceSpoolOptions = {},
  ) {
    this.sessionId = started.sessionId;
    this.queue = new BoundedAudioUploadQueue(
      async (chunk) => {
        const sequence = this.nextSequence;
        await this.transport.append(this.sessionId, sequence, chunk);
        this.nextSequence += 1;
      },
      {
        maxChunks: options.maxChunks,
        maxBytes: options.maxBufferedBytes,
        maxChunkBytes: started.maxChunkBytes,
        bytesPerSecond: started.sampleRate * 2,
        maxBufferedDurationMs: options.maxBufferedDurationMs ?? 2_000,
        onRejected: (telemetry) => {
          this.rejected = true;
          options.onBackpressure?.(telemetry);
        },
        onError: options.onError,
      },
    );
  }

  enqueue(chunk: Uint8Array): boolean {
    if (!this.accepting) return false;
    return this.queue.enqueue(chunk);
  }

  finish(): Promise<VoiceAudioSpoolDescriptor> {
    if (this.finishPromise) return this.finishPromise;
    this.accepting = false;
    this.finishPromise = (async () => {
      await this.queue.flush();
      if (this.rejected) {
        throw new Error('Voice spool renderer queue exceeded its hard bound');
      }
      return this.transport.finish(this.sessionId);
    })();
    return this.finishPromise;
  }

  /** Finalize chunks already acknowledged by native storage after the
   * renderer queue itself has failed or rejected later audio. */
  finishAcceptedAudio(): Promise<VoiceAudioSpoolDescriptor> {
    if (this.recoveryPromise) return this.recoveryPromise;
    this.accepting = false;
    this.queue.cancel('Finalizing acknowledged voice audio after upload failure');
    this.recoveryPromise = this.transport.finish(this.sessionId);
    return this.recoveryPromise;
  }

  /** Preserve acknowledged audio when its React owner unmounts. This is not
   * a privacy cancellation: native state is finalized when possible and its
   * crash journal remains authoritative if application shutdown wins the race. */
  preserveAcceptedAudio(): Promise<VoiceAudioSpoolDescriptor> {
    return this.finish().catch(() => this.finishAcceptedAudio());
  }

  cancel(): Promise<void> {
    if (this.cancelPromise) return this.cancelPromise;
    this.accepting = false;
    this.queue.cancel('Voice spool upload cancelled');
    this.cancelPromise = this.transport.cancel(this.sessionId);
    return this.cancelPromise;
  }

  snapshot(): AudioQueueTelemetry {
    return this.queue.snapshot();
  }
}
