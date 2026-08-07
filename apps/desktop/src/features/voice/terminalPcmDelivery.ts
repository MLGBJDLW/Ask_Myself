export type PcmDeliveryResult = 'accepted' | 'rejected' | 'discarded';

/**
 * Preserves the contiguous accepted PCM prefix for one recording. Once a
 * chunk is rejected or delivery throws, later worklet messages are discarded
 * even if the downstream queue subsequently has capacity again.
 */
export class TerminalPcmDelivery {
  private terminal = false;

  constructor(
    private readonly deliverChunk: (chunk: Uint8Array) => boolean | void,
  ) {}

  deliver(chunk: Uint8Array): PcmDeliveryResult {
    if (this.terminal) return 'discarded';
    try {
      if (this.deliverChunk(chunk) === false) {
        this.terminal = true;
        return 'rejected';
      }
      return 'accepted';
    } catch {
      this.terminal = true;
      return 'rejected';
    }
  }

  terminate(): boolean {
    if (this.terminal) return false;
    this.terminal = true;
    return true;
  }

  get isTerminal(): boolean {
    return this.terminal;
  }
}
