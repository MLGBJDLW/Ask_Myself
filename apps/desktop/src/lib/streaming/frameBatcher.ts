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
  private scheduled: object | null = null;

  constructor(
    private readonly flush: (conversationId: string) => void,
    private readonly requestFrame: FrameRequest = requestBrowserFrame,
  ) {}

  schedule(conversationId: string): void {
    this.pending.add(conversationId);
    if (this.scheduled) return;

    const batch = {};
    this.scheduled = batch;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const deliver = () => {
      if (this.scheduled !== batch) return;
      this.scheduled = null;
      if (deadline !== undefined) clearTimeout(deadline);
      const conversations = [...this.pending];
      this.pending.clear();
      conversations.forEach(this.flush);
    };
    // Occluded native WebViews can suspend requestAnimationFrame. A bounded
    // timer keeps task state responsive, and the batch token rejects a late
    // callback when the WebView starts painting again.
    deadline = setTimeout(deliver, 100);
    this.requestFrame(deliver);
  }

  /** Urgent state must be visible immediately and must not be replayed next frame. */
  flushNow(conversationId: string): void {
    this.pending.delete(conversationId);
    this.flush(conversationId);
  }
}
