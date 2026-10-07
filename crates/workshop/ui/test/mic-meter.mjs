// The status bar mic meter (parts/status/mic-meter.ts): a slot left of
// the record LED that draws the shared capture's last METER_BARS levels,
// newest on the right, on animation frames it requests only while capture
// has an owner. Idle, the bars rest as dots and no frame is scheduled; the
// tooltip and accessible label name the owner; a click reveals the owner
// and does nothing without one; the owner's release returns the meter to
// idle. Drives a real StatusBar and SpeechCaptureService (behind a
// scripted backend) against jsdom built from the real index.html, with a
// hand-cranked requestAnimationFrame.
// Run: node --test test/mic-meter.mjs
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const html = await readFile(path.join(uiDir, "index.html"), "utf8");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { StatusBar } from "./src/parts/status/status-bar.ts";
      export { MicMeter, METER_BARS } from "./src/parts/status/mic-meter.ts";
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
  loader: { ".css": "empty" },
});

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.Element = window.Element;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Node = window.Node;
globalThis.getComputedStyle = window.getComputedStyle.bind(window);

// A hand-cranked animation clock: frame requests queue here, and
// runFrames() runs the queued callbacks the way one display refresh would.
const frames = new Map();
let nextFrame = 1;
let requested = 0;
const requestFrame = (callback) => {
  requested += 1;
  const id = nextFrame++;
  frames.set(id, callback);
  return id;
};
const cancelFrame = (id) => {
  frames.delete(id);
};
window.requestAnimationFrame = requestFrame;
window.cancelAnimationFrame = cancelFrame;
globalThis.requestAnimationFrame = requestFrame;
globalThis.cancelAnimationFrame = cancelFrame;
function runFrames() {
  const due = [...frames.values()];
  frames.clear();
  for (const callback of due) callback(0);
}

const bundlePath = path.join(os.tmpdir(), "promptforge-mic-meter-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, StatusBar, MicMeter, METER_BARS, SpeechCaptureService } = await import(
  pathToFileURL(bundlePath).href
);

// A PCM16 square wave whose chunkLevel reads `level`: the level maps -60
// dBFS to 0 and full scale to 1, and a square wave's RMS is its amplitude.
function chunkAt(level) {
  const amplitude = Math.round(32_767 * 10 ** (-3 * (1 - level)));
  const view = new DataView(new ArrayBuffer(480 * 2));
  for (let index = 0; index < 480; index++) {
    view.setInt16(index * 2, index % 2 === 0 ? amplitude : -amplitude, true);
  }
  return view.buffer;
}

// One status bar holding the record LED, a capture service whose scripted
// backend hands its emit to the test (optionally emitting one chunk while
// the graph is still opening), and the meter under test.
function harness(options = {}) {
  frames.clear();
  requested = 0;
  const bar = new StatusBar();
  const record = bar.register({ id: "recording", name: "Recording indicator", order: 0 });
  const h = {
    bar,
    emit: null,
    owner: Symbol("agent"),
    name: "Session (Agent)",
    reveals: 0,
  };
  h.presence = {
    label: () => h.name,
    reveal: () => {
      h.reveals += 1;
    },
  };
  h.capture = new SpeechCaptureService({
    async open(emitAudio) {
      h.emit = emitAudio;
      if (options.chunkWhileOpening !== undefined) {
        emitAudio(options.chunkWhileOpening);
      }
      return { clear() {}, async stop() {}, dispose() {} };
    },
  });
  h.meter = new MicMeter(bar, h.capture);
  h.element = window.document.querySelector('.status-bar__indicators > [data-indicator="mic-meter"]');
  h.levels = () =>
    [...h.element.querySelectorAll(".status-bar__meter-bar")].map((bar) =>
      Number(bar.style.getPropertyValue("--level")),
    );
  h.idle = () => h.element.classList.contains("status-bar__meter--idle");
  h.start = async () => {
    const outcome = await h.capture.start(h.owner, h.presence);
    assert.equal(outcome.ok, true, "the scripted capture starts");
  };
  h.dispose = () => {
    h.meter.dispose();
    h.capture.dispose();
    record.dispose();
    bar.dispose();
  };
  return h;
}

function assertLevels(actual, expected, message) {
  assert.equal(actual.length, expected.length, `${message}: bar count`);
  actual.forEach((level, index) => {
    assert.ok(
      Math.abs(level - expected[index]) < 0.01,
      `${message}: bar ${index} reads ${level}, expected ${expected[index]} (all: ${actual.join(", ")})`,
    );
  });
}

const dots = () => Array.from({ length: METER_BARS }, () => 0);

test("the meter is a button of nine bars left of the record LED", async () => {
  await assertNoLeaks(lifecycle, () => {
    const h = harness();
    assert.equal(METER_BARS, 9);
    assert.equal(h.element?.tagName, "BUTTON", "the meter renders as a button");
    const order = [...window.document.querySelectorAll(".status-bar__indicators > *")].map(
      (el) => el.dataset.indicator,
    );
    assert.deepEqual(order, ["mic-meter", "recording"], "the meter stands left of the record LED");
    assert.equal(h.element.querySelectorAll(".status-bar__meter-bar").length, METER_BARS);
    h.dispose();
  });
});

test("idle shows dots and schedules no frames", async () => {
  await assertNoLeaks(lifecycle, () => {
    const h = harness();
    assert.equal(h.idle(), true, "a meter with no owner is idle");
    assertLevels(h.levels(), dots(), "idle bars rest as dots");
    assert.equal(requested, 0, "an idle meter requests no animation frame");
    h.dispose();
  });
});

test("a level that arrives before the owner is recorded draws nothing", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness({ chunkWhileOpening: chunkAt(0.9) });
    await h.start();
    assert.equal(requested, 0, "an unowned level requests no frame");
    runFrames();
    assertLevels(h.levels(), dots(), "the unowned level never reaches the bars");
    h.dispose();
  });
});

test("bars scroll right to left as levels arrive", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    await h.start();
    assert.equal(h.idle(), false, "an owned meter is live");
    h.emit(chunkAt(0.2));
    h.emit(chunkAt(0.5));
    assertLevels(h.levels(), dots(), "levels wait for the next frame");
    assert.equal(frames.size, 1, "two levels in one frame share one frame request");
    runFrames();
    assertLevels(h.levels(), [0, 0, 0, 0, 0, 0, 0, 0.2, 0.5], "the newest level is rightmost");
    h.emit(chunkAt(0.8));
    runFrames();
    assertLevels(h.levels(), [0, 0, 0, 0, 0, 0, 0.2, 0.5, 0.8], "earlier levels move left");
    const latest = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
    for (const level of latest) h.emit(chunkAt(level));
    runFrames();
    assertLevels(h.levels(), latest, "only the last METER_BARS levels stay");
    assert.equal(frames.size, 0, "no frame is requested without a new level");
    h.dispose();
  });
});

test("the tooltip and accessible label name the owner", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    assert.equal(h.element.title, "", "an idle meter has no tooltip");
    await h.start();
    assert.match(h.element.title, /Session \(Agent\)/, "the tooltip names the owner");
    assert.match(h.element.getAttribute("aria-label"), /Session \(Agent\)/, "the label names the owner");
    h.name = "Renamed (Agent)";
    h.emit(chunkAt(0.4));
    runFrames();
    assert.match(h.element.title, /Renamed \(Agent\)/, "a renamed owner shows on the next frame");
    assert.match(h.element.getAttribute("aria-label"), /Renamed \(Agent\)/);
    await h.capture.stop(h.owner);
    assert.equal(h.element.title, "", "the released meter drops the tooltip");
    assert.doesNotMatch(h.element.getAttribute("aria-label"), /Agent/, "the released label names nobody");
    h.dispose();
  });
});

test("a click calls the owner's reveal", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    await h.start();
    h.element.click();
    assert.equal(h.reveals, 1, "one click reveals the owner once");
    h.dispose();
  });
});

test("a click with no owner does nothing", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    h.element.click();
    assert.equal(h.reveals, 0, "a click before any owner reveals nothing");
    await h.start();
    await h.capture.stop(h.owner);
    h.element.click();
    assert.equal(h.reveals, 0, "a click after the owner released reveals nothing");
    h.dispose();
  });
});

test("the meter returns to idle when the owner releases the microphone", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    await h.start();
    h.emit(chunkAt(0.5));
    runFrames();
    h.emit(chunkAt(0.7));
    assert.equal(frames.size, 1, "a level is waiting for its frame");
    await h.capture.stop(h.owner);
    assert.equal(h.idle(), true, "the released meter is idle");
    assertLevels(h.levels(), dots(), "the released meter rests as dots at once");
    assert.equal(frames.size, 0, "the release cancels the pending frame");
    const before = requested;
    h.emit(chunkAt(0.9));
    assert.equal(requested, before, "a stray level after the release requests no frame");
    h.dispose();
  });
});

test("the meter returns to idle when the capture service disposes", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    await h.start();
    h.emit(chunkAt(0.6));
    runFrames();
    h.capture.dispose();
    assert.equal(h.idle(), true, "the meter idles with the disposed capture");
    assertLevels(h.levels(), dots(), "the bars rest as dots");
    h.dispose();
  });
});

test("disposing the meter removes its slot and stops drawing", async () => {
  await assertNoLeaks(lifecycle, async () => {
    const h = harness();
    await h.start();
    h.emit(chunkAt(0.5));
    h.meter.dispose();
    assert.equal(h.element.isConnected, false, "the slot leaves the bar");
    assert.equal(frames.size, 0, "the pending frame is cancelled");
    const before = requested;
    h.emit(chunkAt(0.5));
    assert.equal(requested, before, "a disposed meter requests no frame");
    h.dispose();
  });
});
