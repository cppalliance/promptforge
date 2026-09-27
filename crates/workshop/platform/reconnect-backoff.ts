// Exponential reconnect backoff, shared by the persistent workshop socket
// (workshop-socket.ts), the agent-session socket (agent-socket.ts), and
// the Realtime transcription socket (realtime-transcription.ts). The
// first retry waits `initialMs`, each failure doubles the wait, and `maxMs`
// keeps a down server from pushing the wait out of bounds. One timer at a
// time: scheduling while an attempt is already waiting stacks nothing. A
// successful open resets the delay to `initialMs`; disposal cancels any
// pending attempt.

export interface ReconnectBackoffOptions {
  readonly initialMs?: number;
  readonly maxMs?: number;
}

const DEFAULT_INITIAL_MS = 1000;
const DEFAULT_MAX_MS = 30_000;

export class ReconnectBackoff {
  private readonly initialMs: number;
  private readonly maxMs: number;
  private delayMs: number;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: ReconnectBackoffOptions = {}) {
    this.initialMs = options.initialMs ?? DEFAULT_INITIAL_MS;
    this.maxMs = options.maxMs ?? DEFAULT_MAX_MS;
    this.delayMs = this.initialMs;
  }

  /**
   * Cancels any pending attempt and restores the initial delay - the
   * successful-open path, so the next dropout starts over at `initialMs`.
   */
  reset(): void {
    this.cancel();
    this.delayMs = this.initialMs;
  }

  /** Cancels any pending attempt; the disposal teardown path. */
  cancel(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  /**
   * Schedules the next attempt with exponential backoff: `retry` runs
   * after the current delay, which doubles per failed attempt up to
   * `maxMs`. One timer at a time - a call while an attempt is already
   * waiting stacks nothing.
   */
  schedule(retry: () => void): void {
    if (this.timer !== null) {
      return;
    }
    const delay = this.delayMs;
    this.delayMs = Math.min(delay * 2, this.maxMs);
    this.timer = setTimeout(() => {
      this.timer = null;
      retry();
    }, delay);
  }
}
