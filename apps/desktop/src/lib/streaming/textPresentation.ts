import { markdownPresentationInterval, LONG_STREAM_THRESHOLD_CHARS } from './markdownPresentation';

export type StreamingMode = 'chunked' | 'balanced' | 'smooth';
interface PresentationClock {
  now: () => number;
  schedule: (callback: () => void, delay: number) => unknown;
  cancel: (handle: unknown) => void;
}
const clock: PresentationClock = {
  now: () => performance.now(),
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: handle => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export function presentationInterval(mode: StreamingMode, length: number): number {
  if (mode === 'chunked') return length >= LONG_STREAM_THRESHOLD_CHARS ? 400 : 180;
  if (mode === 'smooth') return length >= LONG_STREAM_THRESHOLD_CHARS ? 80 : 32;
  return markdownPresentationInterval(length);
}

/** A disposable projection of canonical text. A fixed reveal deadline prevents
 * continuous token arrivals from postponing painting or leaving a growing tail. */
export class TextPresentation {
  private target: string;
  private presented: string;
  private timer: unknown = null;
  private lastPaint: number;
  private deadline = 0;
  private disposed = false;
  constructor(initial: string, private readonly mode: StreamingMode, private readonly reduceMotion: boolean,
    private readonly paint: (text: string) => void, private readonly timerClock = clock) {
    this.target = this.presented = initial;
    this.lastPaint = timerClock.now();
  }

  update(text: string, streaming: boolean) {
    if (this.disposed) return;
    this.target = text;
    if (!streaming || !text.startsWith(this.presented)) {
      this.clearTimer();
      this.deadline = 0;
      this.commit(text);
      return;
    }
    if (text === this.presented || this.timer !== null) return;
    if (!this.deadline) this.deadline = this.timerClock.now() + 240;
    this.schedule();
  }

  private schedule() {
    const interval = presentationInterval(this.mode, this.target.length);
    this.timer = this.timerClock.schedule(() => {
      this.timer = null;
      if (this.disposed) return;
      const now = this.timerClock.now();
      let end = this.target.length;
      if (this.mode === 'smooth' && !this.reduceMotion && now < this.deadline) {
        const ticksLeft = Math.max(1, Math.ceil((this.deadline - now) / interval));
        end = Math.min(end, this.presented.length + Math.max(1, Math.ceil((end - this.presented.length) / ticksLeft)));
        // Never reveal half of a UTF-16 surrogate pair.
        const previous = this.target.charCodeAt(end - 1);
        if (previous >= 0xd800 && previous <= 0xdbff && end < this.target.length) end++;
      }
      this.commit(this.target.slice(0, end));
      if (this.presented !== this.target) this.schedule();
      else this.deadline = 0;
    }, Math.max(0, interval - (this.timerClock.now() - this.lastPaint)));
  }

  private commit(text: string) {
    this.lastPaint = this.timerClock.now();
    if (text === this.presented) return;
    this.presented = text;
    this.paint(text);
  }
  private clearTimer() {
    if (this.timer !== null) this.timerClock.cancel(this.timer);
    this.timer = null;
  }
  dispose() { this.disposed = true; this.clearTimer(); }
}
