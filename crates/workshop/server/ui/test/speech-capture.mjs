import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import path from "node:path";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { SpeechCaptureService } from "./src/services/speech-capture.ts";
    `,
    resolveDir: uiDir,
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { lifecycle, SpeechCaptureService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

function installBrowser(options = {}) {
  const resources = {
    constraints: [],
    contexts: [],
    sources: [],
    nodes: [],
    tracks: [{ stops: 0 }, { stops: 0 }],
    stream: null,
  };
  const stream = {
    getTracks: () =>
      resources.tracks.map((track) => ({
        stop() {
          track.stops += 1;
        },
      })),
  };
  resources.stream = stream;

  class FakeAudioContext {
    constructor(contextOptions) {
      if (options.contextError) {
        throw options.contextError;
      }
      this.options = contextOptions;
      this.sampleRate = options.contextSampleRate ?? 24_000;
      this.destination = { kind: "destination" };
      this.resumeCalls = 0;
      this.closeCalls = 0;
      this.audioWorklet = {
        addModule: async (url) => {
          this.moduleUrl = url;
          if (options.moduleError) {
            throw options.moduleError;
          }
        },
      };
      resources.contexts.push(this);
    }

    createMediaStreamSource(receivedStream) {
      assert.equal(receivedStream, stream);
      const source = {
        connects: [],
        disconnects: 0,
        connect(target) {
          this.connects.push(target);
        },
        disconnect() {
          this.disconnects += 1;
        },
      };
      resources.sources.push(source);
      return source;
    }

    async resume() {
      this.resumeCalls += 1;
      if (options.resumeError) {
        throw options.resumeError;
      }
    }

    async close() {
      this.closeCalls += 1;
      if (options.closeError) {
        throw options.closeError;
      }
    }
  }

  class FakeAudioWorkletNode {
    constructor(context, name) {
      if (options.nodeError) {
        throw options.nodeError;
      }
      this.context = context;
      this.name = name;
      this.connects = [];
      this.disconnects = 0;
      this.messages = [];
      this.port = {
        onmessage: null,
        postMessage: (message) => {
          this.messages.push(message);
          if (message?.type === "flush" && options.autoFlush !== false) {
            queueMicrotask(() => this.port.onmessage?.({ data: { type: "flushed" } }));
          }
          if (
            options.postMessageError &&
            (!options.postMessageType || options.postMessageType === message?.type)
          ) {
            throw options.postMessageError;
          }
        },
      };
      resources.nodes.push(this);
    }

    connect(target) {
      this.connects.push(target);
    }

    disconnect() {
      this.disconnects += 1;
    }

    emit(bytes) {
      this.port.onmessage?.({ data: Uint8Array.from(bytes).buffer });
    }
  }

  const previous = {
    navigator: Object.getOwnPropertyDescriptor(globalThis, "navigator"),
    AudioContext: Object.getOwnPropertyDescriptor(globalThis, "AudioContext"),
    AudioWorkletNode: Object.getOwnPropertyDescriptor(globalThis, "AudioWorkletNode"),
  };
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      mediaDevices: {
        getUserMedia: async (constraints) => {
          resources.constraints.push(constraints);
          if (options.mediaError) {
            throw options.mediaError;
          }
          if (options.mediaPromise) {
            return options.mediaPromise;
          }
          return stream;
        },
      },
    },
  });
  Object.defineProperty(globalThis, "AudioContext", {
    configurable: true,
    value: FakeAudioContext,
  });
  Object.defineProperty(globalThis, "AudioWorkletNode", {
    configurable: true,
    value: FakeAudioWorkletNode,
  });

  return {
    resources,
    restore() {
      for (const [name, descriptor] of Object.entries(previous)) {
        if (descriptor) {
          Object.defineProperty(globalThis, name, descriptor);
        } else {
          delete globalThis[name];
        }
      }
    },
  };
}

async function withBrowser(options, run) {
  const browser = installBrowser(options);
  try {
    await run(browser.resources);
  } finally {
    browser.restore();
  }
}

test("the default backend owns the complete 24 kHz capture lifecycle", async () => {
  await assertNoLeaks(lifecycle, async () => {
    await withBrowser({}, async (resources) => {
      const service = new SpeechCaptureService();
      const audio = [];
      service.onAudio((chunk) => audio.push(...new Uint8Array(chunk)));

      assert.deepEqual(await service.start(), { ok: true, kind: "started" });
      assert.equal(service.recording, true);
      assert.deepEqual(resources.constraints, [
        {
          audio: {
            channelCount: 1,
            sampleRate: 24_000,
            echoCancellation: true,
            noiseSuppression: true,
          },
        },
      ]);
      assert.equal(resources.contexts[0].options.sampleRate, 24_000);
      assert.equal(resources.contexts[0].sampleRate, 24_000);
      assert.equal(resources.contexts[0].moduleUrl, "/pcm-worklet.js");
      assert.equal(resources.contexts[0].resumeCalls, 1);
      assert.equal(resources.nodes[0].name, "pcm16-capture");
      assert.deepEqual(resources.sources[0].connects, [resources.nodes[0]]);
      assert.deepEqual(resources.nodes[0].connects, [resources.contexts[0].destination]);

      resources.nodes[0].emit([1, 2, 255]);
      assert.deepEqual(audio, [1, 2, 255]);
      assert.deepEqual(service.clear(), { ok: true, kind: "cleared" });
      assert.deepEqual(resources.nodes[0].messages, [{ type: "clear" }]);
      assert.deepEqual(await service.start(), {
        ok: false,
        kind: "start-failed",
        message: "speech capture is already active",
        recoverable: true,
      });

      assert.deepEqual(await service.stop(), { ok: true, kind: "stopped" });
      assert.equal(service.recording, false);
      assert.deepEqual(resources.nodes[0].messages, [{ type: "clear" }, { type: "flush" }]);
      assert.equal(resources.sources[0].disconnects, 1);
      assert.equal(resources.nodes[0].disconnects, 1);
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
      assert.equal(resources.nodes[0].port.onmessage, null);
      assert.deepEqual(await service.stop(), { ok: true, kind: "stopped" });
      service.dispose();
    });
  });
});

test("the default backend classifies permission, device, and graph start failures", async () => {
  await assertNoLeaks(lifecycle, async () => {
    for (const [mediaError, kind] of [
      [new DOMException("microphone denied", "NotAllowedError"), "permission-denied"],
      [new DOMException("no microphone", "NotFoundError"), "device-unavailable"],
    ]) {
      await withBrowser({ mediaError }, async (resources) => {
        const service = new SpeechCaptureService();
        assert.deepEqual(await service.start(), {
          ok: false,
          kind,
          message: mediaError.message,
          recoverable: true,
        });
        assert.equal(service.recording, false);
        assert.equal(resources.contexts.length, 0);
        assert.deepEqual(
          resources.tracks.map((track) => track.stops),
          [0, 0],
        );
        service.dispose();
      });
    }

    await withBrowser(
      { moduleError: new Error("worklet load failed") },
      async (resources) => {
        const service = new SpeechCaptureService();
        assert.deepEqual(await service.start(), {
          ok: false,
          kind: "start-failed",
          message: "worklet load failed",
          recoverable: true,
        });
        assert.equal(service.recording, false);
        assert.deepEqual(
          resources.tracks.map((track) => track.stops),
          [1, 1],
        );
        assert.equal(resources.contexts[0].closeCalls, 1);
        assert.equal(resources.sources.length, 0);
        assert.equal(resources.nodes.length, 0);
        service.dispose();
      },
    );

    await withBrowser({ contextSampleRate: 48_000 }, async (resources) => {
      const service = new SpeechCaptureService();
      assert.deepEqual(await service.start(), {
        ok: false,
        kind: "start-failed",
        message: "browser opened audio at 48000 Hz instead of 24000 Hz",
        recoverable: true,
      });
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
      service.dispose();
    });
  });
});

test("stop and disposal release every production graph resource", async () => {
  await assertNoLeaks(lifecycle, async () => {
    await withBrowser({ closeError: new Error("context close failed") }, async (resources) => {
      const service = new SpeechCaptureService();
      assert.deepEqual(await service.start(), { ok: true, kind: "started" });
      assert.deepEqual(await service.stop(), {
        ok: false,
        kind: "stop-failed",
        message: "context close failed",
        recoverable: true,
      });
      assert.equal(service.recording, false);
      assert.equal(resources.sources[0].disconnects, 1);
      assert.equal(resources.nodes[0].disconnects, 1);
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
      assert.equal(resources.nodes[0].port.onmessage, null);
      service.dispose();
    });

    await withBrowser(
      { postMessageError: new Error("worklet port failed") },
      async (resources) => {
        const service = new SpeechCaptureService();
        assert.deepEqual(await service.start(), { ok: true, kind: "started" });
        assert.deepEqual(await service.stop(), {
          ok: false,
          kind: "stop-failed",
          message: "worklet port failed",
          recoverable: true,
        });
        assert.equal(resources.sources[0].disconnects, 1);
        assert.equal(resources.nodes[0].disconnects, 1);
        assert.deepEqual(
          resources.tracks.map((track) => track.stops),
          [1, 1],
        );
        assert.equal(resources.contexts[0].closeCalls, 1);
        service.dispose();
      },
    );

    await withBrowser(
      {
        postMessageError: new Error("clear failed"),
        postMessageType: "clear",
      },
      async () => {
        const service = new SpeechCaptureService();
        assert.deepEqual(await service.start(), { ok: true, kind: "started" });
        assert.deepEqual(service.clear(), {
          ok: false,
          kind: "clear-failed",
          message: "clear failed",
          recoverable: true,
        });
        assert.equal(service.recording, true);
        assert.deepEqual(await service.stop(), { ok: true, kind: "stopped" });
        service.dispose();
      },
    );

    await withBrowser({ autoFlush: false }, async (resources) => {
      const service = new SpeechCaptureService();
      assert.deepEqual(await service.start(), { ok: true, kind: "started" });
      const stopping = service.stop();
      service.dispose();
      assert.deepEqual(await stopping, {
        ok: false,
        kind: "stop-failed",
        message: "speech capture was disposed while flushing",
        recoverable: true,
      });
      assert.equal(resources.sources[0].disconnects, 1);
      assert.equal(resources.nodes[0].disconnects, 1);
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
    });

    await withBrowser({}, async (resources) => {
      const service = new SpeechCaptureService();
      assert.deepEqual(await service.start(), { ok: true, kind: "started" });
      service.dispose();
      service.dispose();
      assert.equal(service.recording, false);
      assert.equal(resources.sources[0].disconnects, 1);
      assert.equal(resources.nodes[0].disconnects, 1);
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
      assert.equal(resources.nodes[0].port.onmessage, null);
    });
  });
});

test("disposal during production start rejects the take and leaks no graph", async () => {
  let resolveMedia;
  const mediaPromise = new Promise((resolve) => {
    resolveMedia = resolve;
  });

  await assertNoLeaks(lifecycle, async () => {
    await withBrowser({ mediaPromise }, async (resources) => {
      const service = new SpeechCaptureService();
      const starting = service.start();
      await Promise.resolve();
      service.dispose();
      resolveMedia(resources.stream);

      assert.deepEqual(await starting, {
        ok: false,
        kind: "start-failed",
        message: "speech capture was disposed while starting",
        recoverable: true,
      });
      assert.equal(service.recording, false);
      assert.equal(resources.sources[0].disconnects, 1);
      assert.equal(resources.nodes[0].disconnects, 1);
      assert.deepEqual(
        resources.tracks.map((track) => track.stops),
        [1, 1],
      );
      assert.equal(resources.contexts[0].closeCalls, 1);
      assert.equal(resources.nodes[0].port.onmessage, null);
    });
  });
});
