export type FrameRequest = (callback: () => void) => unknown;

function requestBrowserFrame(callback: () => void): unknown {
  if (typeof globalThis.requestAnimationFrame === 'function') {
    return globalThis.requestAnimationFrame(callback);
  }
  return setTimeout(callback, 16);
}

/** Coalesce ordinary stream mutations into one subscriber notification per frame. */
export class ConversationFrameBatcher {
  private readonly pending = new Set<string>();
  private framePending = false;

  constructor(
    private readonly flush: (conversationId: string) => void,
    private readonly requestFrame: FrameRequest = requestBrowserFrame,
  ) {}

  schedule(conversationId: string): void {
    this.pending.add(conversationId);
    if (this.framePending) return;

    this.framePending = true;
    this.requestFrame(() => {
      this.framePending = false;
      const conversations = [...this.pending];
      this.pending.clear();
      conversations.forEach(this.flush);
    });
  }

  /** Urgent state must be visible immediately and must not be replayed next frame. */
  flushNow(conversationId: string): void {
    this.pending.delete(conversationId);
    this.flush(conversationId);
  }
}
