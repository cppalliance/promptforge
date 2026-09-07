import { DisposableStore, toDisposable } from "../base/lifecycle";
import {
  RealtimeTranscriptionService,
  type RealtimeTranscriptCompletion,
  type RealtimeTranscriptSnapshot,
} from "../services/realtime-transcription";
import {
  SpeechCaptureService,
  type SpeechCaptureFailure,
  type SpeechCaptureOutcome,
} from "../services/speech-capture";
import type {
  SttBlocker,
  SttElements,
  SttHandle,
  SttStatus,
} from "./stt";

interface Take {
  from: number;
  length: number;
  readonly original: string;
  itemId: string | null;
}

function captureFailureLabel(failure: SpeechCaptureFailure): string {
  if (failure.kind === "permission-denied") {
    return "Microphone permission was denied.";
  }
  if (failure.kind === "device-unavailable") {
    return "No microphone is available.";
  }
  if (failure.kind === "stop-failed") {
    return "Dictation could not finish capturing audio. Try again.";
  }
  return "Dictation could not start. Try again.";
}

/**
 * Wires push-to-talk UI to production PCM16 capture and the additive Realtime
 * relay. Item-keyed take regions isolate overlapping authoritative results.
 */
export function setupStt(
  elements: SttElements,
  status: SttStatus,
  blocked: SttBlocker,
  capture: SpeechCaptureService,
  providedRealtime?: RealtimeTranscriptionService,
): SttHandle {
  const { mic, input } = elements;
  const store = new DisposableStore();
  const realtime = providedRealtime ?? store.add(new RealtimeTranscriptionService());
  const takes: Take[] = [];
  const awaitingCommit: Array<Take | null> = [];
  const byItem = new Map<string, Take>();
  const byClientEvent = new Map<string, Take>();
  const retiredItems = new Set<string>();
  let active: Take | null = null;
  let stopping = false;
  let pendingCaptureStop: Promise<SpeechCaptureOutcome> | null = null;
  let disposed = false;

  function setRecording(recording: boolean): void {
    mic.classList.toggle("stt-mic--recording", recording);
    mic.setAttribute("aria-pressed", String(recording));
    mic.title = recording ? "Stop recording" : "Push to talk";
    status.setRecording(recording || (active === null && capture.recording));
  }

  function releaseCapture(): Promise<SpeechCaptureOutcome> {
    if (pendingCaptureStop !== null) {
      return pendingCaptureStop;
    }
    const stoppingCapture = capture.stop();
    pendingCaptureStop = stoppingCapture;
    void stoppingCapture.finally(() => {
      if (pendingCaptureStop === stoppingCapture) {
        pendingCaptureStop = null;
      }
    });
    return stoppingCapture;
  }

  function syncInputLock(): void {
    input.setReadOnly(takes.length > 0);
  }

  function splice(take: Take, text: string): void {
    const oldEnd = take.from + take.length;
    const delta = text.length - take.length;
    input.replaceRange(take.from, oldEnd, text);
    take.length = text.length;
    if (delta === 0) {
      return;
    }
    for (const other of takes) {
      if (other !== take && other.from >= oldEnd) {
        other.from += delta;
      }
    }
  }

  function removeTake(take: Take): void {
    const index = takes.indexOf(take);
    if (index >= 0) {
      takes.splice(index, 1);
    }
    const waiting = awaitingCommit.indexOf(take);
    if (waiting >= 0) {
      awaitingCommit[waiting] = null;
    }
    if (take.itemId !== null) {
      byItem.delete(take.itemId);
      retiredItems.add(take.itemId);
    }
    if (active === take) {
      active = null;
    }
    for (const [eventId, owner] of byClientEvent) {
      if (owner === take) {
        byClientEvent.delete(eventId);
      }
    }
    syncInputLock();
  }

  function rollback(take: Take): void {
    splice(take, take.original);
    removeTake(take);
  }

  function rollbackAll(): void {
    for (const take of [...takes].reverse()) {
      rollback(take);
    }
  }

  function takeFor(itemId: string): Take | null {
    return byItem.get(itemId) ?? null;
  }

  function applySnapshot(snapshot: RealtimeTranscriptSnapshot): void {
    let take = takeFor(snapshot.itemId);
    if (take === null) {
      if (retiredItems.has(snapshot.itemId) || awaitingCommit.includes(null)) {
        return;
      }
      const unbound = takes.filter((candidate) => candidate.itemId === null);
      if (unbound.length !== 1) {
        return;
      }
      take = unbound[0];
      take.itemId = snapshot.itemId;
      byItem.set(snapshot.itemId, take);
    }
    splice(take, snapshot.text);
  }

  function applyCompletion(completion: RealtimeTranscriptCompletion): void {
    const take = takeFor(completion.itemId);
    if (take === null) {
      return;
    }
    if (active === take && capture.recording) {
      active = null;
      void releaseCapture();
      setRecording(false);
    }
    const current = input.readRange(take.from, take.from + take.length);
    const insertionWhitespace = current.match(/^\s+/)?.[0] ?? "";
    const authoritative = completion.transcript.trimEnd();
    const transcript =
      take.original === "" &&
      authoritative !== "" &&
      insertionWhitespace !== "" &&
      !/^\s/.test(authoritative)
        ? insertionWhitespace + authoritative
        : authoritative;
    splice(take, transcript);
    removeTake(take);
    if (transcript === "") {
      status.showLocal("No speech was detected.", "info");
    } else {
      input.focus();
      status.showLocal("Dictation ready.", "info");
    }
  }

  store.add(
    realtime.onCommitted((itemId) => {
      if (awaitingCommit.length === 0) {
        if (byItem.has(itemId)) {
          return;
        }
        if (active !== null && active.itemId === null) {
          active.itemId = itemId;
          byItem.set(itemId, active);
          return;
        }
        retiredItems.add(itemId);
        status.showLocal("Dictation is temporarily unavailable. Try again.", "error");
        return;
      }
      const take = awaitingCommit[0];
      if (take === null) {
        awaitingCommit.shift();
        retiredItems.add(itemId);
        return;
      }
      if (take.itemId === null) {
        awaitingCommit.shift();
        take.itemId = itemId;
        byItem.set(itemId, take);
        return;
      }
      if (take.itemId === itemId) {
        awaitingCommit.shift();
        return;
      }
      awaitingCommit.shift();
      retiredItems.add(itemId);
      rollback(take);
      status.showLocal("Dictation is temporarily unavailable. Try again.", "error");
    }),
  );
  store.add(realtime.onSnapshot(applySnapshot));
  store.add(realtime.onCompleted(applyCompletion));
  store.add(
    realtime.onFailed(({ itemId }) => {
      const take = takeFor(itemId);
      if (take !== null) {
        rollback(take);
      }
      status.showLocal("Dictation could not be transcribed. Try again.", "error");
    }),
  );
  store.add(
    realtime.onError((error) => {
      if (error.scope === "connection") {
        if (takes.length > 0 && active !== null && capture.recording) {
          capture.clear();
          void releaseCapture();
        }
        rollbackAll();
        awaitingCommit.length = 0;
        setRecording(false);
      } else {
        const affected =
          error.scope === "event" && error.eventId !== null
            ? byClientEvent.get(error.eventId) ?? null
            : active;
        if (affected !== null) {
          if (active === affected && capture.recording) {
            capture.clear();
            void releaseCapture();
          }
          rollback(affected);
          setRecording(false);
        }
      }
      status.showLocal("Dictation is temporarily unavailable. Try again.", "error");
    }),
  );
  store.add(
    capture.onAudio((chunk) => {
      const take = active;
      if (take === null) {
        return;
      }
      const eventId = realtime.append(chunk);
      if (eventId === null) {
        capture.clear();
        void releaseCapture();
        if (takes.includes(take)) {
          rollback(take);
        }
        setRecording(false);
      } else {
        byClientEvent.set(eventId, take);
      }
    }),
  );

  async function start(): Promise<void> {
    const reason = blocked();
    if (reason !== null) {
      status.showLocal(reason, "info");
      return;
    }
    if (pendingCaptureStop !== null) {
      await pendingCaptureStop;
      if (disposed || active !== null) {
        return;
      }
    }
    if (realtime.state !== "ready") {
      realtime.connect();
      status.showLocal("Dictation is connecting. Try again in a moment.", "info");
      return;
    }
    const selection = input.getSelection();
    const outcome = await capture.start();
    if (!outcome.ok) {
      status.showLocal(captureFailureLabel(outcome), "error");
      return;
    }
    if (disposed) {
      void releaseCapture();
      return;
    }
    const take: Take = {
      from: selection.start,
      length: selection.end - selection.start,
      original: input.readRange(selection.start, selection.end),
      itemId: null,
    };
    takes.push(take);
    active = take;
    syncInputLock();
    setRecording(true);
    status.showLocal("Listening...", "info");
  }

  async function stop(): Promise<void> {
    const take = active;
    if (take === null || stopping) {
      return;
    }
    stopping = true;
    const stoppingCapture = releaseCapture();
    setRecording(false);
    status.showLocal("Transcribing...", "info");
    const outcome = await stoppingCapture;
    if (active === take) {
      active = null;
    }
    stopping = false;
    if (disposed) {
      return;
    }
    if (!takes.includes(take)) {
      return;
    }
    if (!outcome.ok) {
      realtime.clear();
      rollback(take);
      status.showLocal(captureFailureLabel(outcome), "error");
      return;
    }
    const eventId = realtime.commit();
    if (eventId === null) {
      rollback(take);
      return;
    }
    awaitingCommit.push(take);
    byClientEvent.set(eventId, take);
  }

  function discardIfRecording(): void {
    if (takes.length === 0) {
      return;
    }
    if (active !== null && capture.recording) {
      capture.clear();
      realtime.clear();
      void releaseCapture();
    }
    active = null;
    stopping = false;
    rollbackAll();
    setRecording(false);
  }

  const onMicClick = (): void => {
    if (active !== null) {
      void stop();
    } else {
      void start();
    }
  };
  mic.addEventListener("click", onMicClick);
  store.add(toDisposable(() => mic.removeEventListener("click", onMicClick)));

  return {
    discardIfRecording,
    dispose(): void {
      if (disposed) {
        return;
      }
      disposed = true;
      discardIfRecording();
      store.dispose();
    },
  };
}
