import { DisposableStore, toDisposable } from "../base/lifecycle";
import { RealtimeTranscriptionService } from "../services/realtime-transcription";
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
import {
  createTakeRegistry,
  reduceTakeRegistry,
  type TakeRegistry,
  type TakeRegistryEffect,
  type TakeRegistryInput,
} from "./take-registry";

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
 * relay. The registry exclusively owns take state; this layer interprets its
 * typed editor, capture, status, and wire effects.
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
  let registry: TakeRegistry = createTakeRegistry();
  if (realtime.state === "ready") {
    registry = reduceTakeRegistry(registry, {
      type: "connection.ready",
      generation: realtime.generation,
    }).state;
  }
  let pendingCaptureStop: Promise<SpeechCaptureOutcome> | null = null;
  let captureGeneration = realtime.generation;
  let disposed = false;

  function setRecording(recording: boolean): void {
    mic.classList.toggle("stt-mic--recording", recording);
    mic.setAttribute("aria-pressed", String(recording));
    mic.title = recording ? "Stop recording" : "Push to talk";
    status.setRecording(recording);
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

  function interpretEffect(effect: TakeRegistryEffect): void {
    switch (effect.domain) {
      case "editor":
        switch (effect.command) {
          case "replace":
            input.replaceRange(effect.from, effect.to, effect.text);
            return;
          case "read-only":
            input.setReadOnly(effect.readOnly);
            return;
          case "focus":
            input.focus();
            return;
          default: {
            const exhaustive: never = effect;
            return exhaustive;
          }
        }
      case "capture":
        switch (effect.command) {
          case "clear":
            capture.clear();
            return;
          case "stop": {
            const stoppingCapture = releaseCapture();
            void stoppingCapture.then((outcome) => {
              if (!disposed) {
                dispatch({
                  type: "capture.stopped",
                  generation: effect.generation,
                  takeId: effect.takeId,
                  ok: outcome.ok,
                });
              }
            });
            return;
          }
          default: {
            const exhaustive: never = effect;
            return exhaustive;
          }
        }
      case "status":
        switch (effect.command) {
          case "recording":
            setRecording(effect.recording);
            return;
          case "local":
            status.showLocal(effect.label, effect.severity);
            return;
          default: {
            const exhaustive: never = effect;
            return exhaustive;
          }
        }
      case "wire":
        switch (effect.command) {
          case "append":
            {
              const result = realtime.append(effect.chunk);
            dispatch({
              type: "wire.result",
              generation: result.generation,
              requestId: effect.requestId,
              eventId: result.eventId,
            });
            return;
            }
          case "commit":
            {
              const result = realtime.commit();
            dispatch({
              type: "wire.result",
              generation: result.generation,
              requestId: effect.requestId,
              eventId: result.eventId,
            });
            return;
            }
          case "clear":
            realtime.clear();
            return;
          default: {
            const exhaustive: never = effect;
            return exhaustive;
          }
        }
      default: {
        const exhaustive: never = effect;
        return exhaustive;
      }
    }
  }

  function dispatch(inputEvent: TakeRegistryInput): void {
    const transition = reduceTakeRegistry(registry, inputEvent);
    registry = transition.state;
    for (const effect of transition.effects) {
      interpretEffect(effect);
    }
  }

  store.add(
    realtime.onEvent(({ event, generation }) => {
      dispatch({ type: "server.event", event, generation });
    }),
  );
  store.add(
    realtime.onState(({ state, generation }) => {
      if (state === "ready") {
        dispatch({ type: "connection.ready", generation });
      }
    }),
  );
  store.add(
    realtime.onError((error) => {
      if (error.scope === "connection") {
        dispatch({
          type: "connection.lost",
          generation: error.generation,
        });
      } else if (error.scope === "session") {
        dispatch({
          type: "service.error",
          generation: error.generation,
          eventId: error.eventId,
        });
      }
    }),
  );
  store.add(
    capture.onAudio((chunk) => {
      dispatch({
        type: "capture.audio",
        generation: captureGeneration,
        chunk,
      });
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
      if (
        disposed ||
        registry.activeTakeId !== null ||
        registry.capture !== "idle"
      ) {
        return;
      }
    }
    if (realtime.state !== "ready") {
      realtime.connect();
      status.showLocal("Dictation is connecting. Try again in a moment.", "info");
      return;
    }
    const generation = realtime.generation;
    captureGeneration = generation;
    const outcome = await capture.start();
    if (!outcome.ok) {
      status.showLocal(captureFailureLabel(outcome), "error");
      return;
    }
    if (disposed) {
      void releaseCapture();
      return;
    }
    dispatch({
      type: "user.start",
      generation,
      context: input.insertionContext(),
    });
    if (
      registry.activeTakeId === null ||
      registry.capture !== "recording"
    ) {
      void releaseCapture();
    }
  }

  function stop(): void {
    dispatch({ type: "user.stop", generation: realtime.generation });
  }

  function discardIfRecording(): void {
    dispatch({ type: "user.discard", generation: realtime.generation });
  }

  const onMicClick = (): void => {
    if (registry.activeTakeId !== null) {
      stop();
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
