// The transcript feed's scroll behavior (src/parts/agent/transcript/feed-scroll.ts)
// against a fake scroller and a ResizeObserver shim: the feed starts pinned;
// scrolling up within 250ms of an input event unpins it, scrolling with no
// recent input (a layout shift, its own follow) does not, and reaching the
// bottom pins again; while pinned, growth up to one viewport follows
// smoothly at no more than 3.6px/ms, bigger growth and reduced motion jump;
// a send forces an instant pin; growth while unpinned leaves the feed where
// the operator put it; the trailing spacer is 14px plus a fifth of the
// feed's height clamped to 80-240px.
// Run: node test/feed-scroll.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export {
        FeedScroll,
        spacerHeight,
        PIN_THRESHOLD_PX,
        INPUT_WINDOW_MS,
        FOLLOW_SPEED_PX_PER_MS,
        FOLLOW_SNAP_MS,
      } from "./src/parts/agent/transcript/feed-scroll.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;

// A frame queue the test steps by hand.
let frames = [];
let nextFrame = 0;
globalThis.requestAnimationFrame = (callback) => {
  frames.push({ id: ++nextFrame, callback });
  return nextFrame;
};
globalThis.cancelAnimationFrame = (id) => {
  frames = frames.filter((frame) => frame.id !== id);
};
function runFrame(time) {
  const due = frames;
  frames = [];
  for (const frame of due) frame.callback(time);
}

// A ResizeObserver whose callbacks the test fires.
const observers = [];
globalThis.ResizeObserver = class {
  constructor(callback) {
    this.callback = callback;
    this.live = true;
    observers.push(this);
  }
  observe() {}
  disconnect() {
    this.live = false;
  }
};
const resize = () => {
  for (const observer of observers) if (observer.live) observer.callback([]);
};

const bundlePath = path.join(os.tmpdir(), "promptforge-feed-scroll-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  FeedScroll,
  spacerHeight,
  PIN_THRESHOLD_PX,
  INPUT_WINDOW_MS,
  FOLLOW_SPEED_PX_PER_MS,
  FOLLOW_SNAP_MS,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** A scroller whose metrics the test sets; scrollTop clamps like a real one. */
function makeFeed({ clientHeight = 400, scrollHeight = 1000, scrollTop = 600 } = {}) {
  const element = document.createElement("div");
  const content = document.createElement("div");
  const spacer = document.createElement("div");
  element.append(content, spacer);
  document.body.appendChild(element);
  const state = { scrollTop, scrollHeight, clientHeight };
  Object.defineProperty(element, "scrollTop", {
    get: () => state.scrollTop,
    set: (value) => {
      state.scrollTop = Math.max(0, Math.min(value, state.scrollHeight - state.clientHeight));
    },
  });
  Object.defineProperty(element, "scrollHeight", { get: () => state.scrollHeight });
  Object.defineProperty(element, "clientHeight", { get: () => state.clientHeight });
  return { element, content, spacer, state };
}

let clock = 0;
const now = () => clock;
const wheel = (element) => element.dispatchEvent(new window.Event("wheel"));
const scrolled = (element) => element.dispatchEvent(new window.Event("scroll"));

function start(options) {
  const feed = makeFeed(options);
  const layouts = [];
  const scroll = new FeedScroll(feed.element, feed.content, {
    spacer: feed.spacer,
    now,
    onLayout: (reason) => layouts.push(reason),
  });
  const bottom = () => feed.state.scrollHeight - feed.state.clientHeight;
  return { ...feed, scroll, layouts, bottom };
}

// --- The trailing spacer ---------------------------------------------------------------

check("the spacer is 14px plus the 80px floor for a short feed", spacerHeight(100) === 94);
check("the spacer is 14px plus a fifth of a mid-size feed", spacerHeight(900) === 194);
check("the spacer is 14px plus the 240px ceiling for a tall feed", spacerHeight(2000) === 254);
{
  const feed = start({ clientHeight: 400 });
  check("the spacer takes its height from the feed on construction", feed.spacer.style.height === "94px");
  feed.state.clientHeight = 900;
  resize();
  check("the spacer follows a resize of the feed", feed.spacer.style.height === "194px");
  feed.scroll.dispose();
}

// --- Pinned, released, re-pinned -----------------------------------------------------

check("the thresholds are the plan's", PIN_THRESHOLD_PX === 4 && INPUT_WINDOW_MS === 250 && FOLLOW_SPEED_PX_PER_MS === 3.6 && FOLLOW_SNAP_MS === 250);

{
  const feed = start();
  check("a feed starts pinned", feed.scroll.pinned === true);

  clock = 1000;
  wheel(feed.element);
  clock = 1100;
  feed.state.scrollTop = 500;
  scrolled(feed.element);
  check("scrolling up within 250ms of an input unpins", feed.scroll.pinned === false);
  check("a scroll reports a layout pass", feed.layouts.includes("scroll"));

  feed.state.scrollTop = feed.bottom() - PIN_THRESHOLD_PX;
  scrolled(feed.element);
  check("coming within 4px of the bottom pins again", feed.scroll.pinned === true);
  feed.scroll.dispose();
}

{
  const feed = start();
  clock = 2000;
  feed.state.scrollTop = 550;
  scrolled(feed.element);
  check("scrolling up with no input at all (a layout shift) does not unpin", feed.scroll.pinned === true);

  wheel(feed.element);
  clock = 2000 + INPUT_WINDOW_MS + 1;
  feed.state.scrollTop = 500;
  scrolled(feed.element);
  check("scrolling up after the input window has closed does not unpin", feed.scroll.pinned === true);

  clock = 3000;
  wheel(feed.element);
  clock = 3000 + INPUT_WINDOW_MS;
  feed.state.scrollTop = 400;
  scrolled(feed.element);
  check("the window's last millisecond still counts", feed.scroll.pinned === false);
  feed.scroll.dispose();
}

// --- Following growth ---------------------------------------------------------------------

{
  const feed = start();
  feed.state.scrollHeight = 1100;
  resize();
  check("growth within a viewport follows on the next frame, not at once", feed.state.scrollTop === 600 && frames.length === 1);
  runFrame(0);
  const first = feed.state.scrollTop;
  runFrame(16);
  const second = feed.state.scrollTop;
  check(
    "the follow eases: each step is at most 3.6px per elapsed millisecond",
    first > 600 && first - 600 <= FOLLOW_SPEED_PX_PER_MS * 16 + 1e-9 && second > first && second - first <= FOLLOW_SPEED_PX_PER_MS * 16 + 1e-9,
  );
  runFrame(32);
  runFrame(48);
  check("the follow arrives at the bottom within the snap window", feed.state.scrollTop === feed.bottom() && frames.length === 0);
  feed.scroll.dispose();
}

{
  const feed = start();
  feed.state.scrollHeight = 1300;
  resize();
  clock = 100;
  wheel(feed.element);
  runFrame(0);
  clock = 110;
  feed.state.scrollTop = feed.state.scrollTop;
  scrolled(feed.element);
  runFrame(16);
  scrolled(feed.element);
  check("the follow's own downward scrolls never unpin", feed.scroll.pinned === true);
  feed.scroll.dispose();
}

{
  const feed = start();
  feed.state.scrollHeight = 1600;
  resize();
  check(
    "growth bigger than a viewport jumps straight to the bottom",
    feed.state.scrollTop === feed.bottom() && frames.length === 0,
  );
  feed.scroll.dispose();
}

{
  window.matchMedia = (query) => ({ matches: true, media: query });
  const feed = start();
  feed.state.scrollHeight = 1100;
  resize();
  check(
    "under reduced motion even small growth jumps",
    feed.state.scrollTop === feed.bottom() && frames.length === 0,
  );
  feed.scroll.dispose();
  delete window.matchMedia;
}

{
  const feed = start();
  feed.state.clientHeight = 300;
  resize();
  check(
    "a shorter feed while pinned keeps the bottom in view",
    feed.state.scrollTop === feed.bottom() && frames.length === 0,
  );
  feed.scroll.dispose();
}

// --- Not following ------------------------------------------------------------------------------

{
  const feed = start();
  clock = 5000;
  wheel(feed.element);
  feed.state.scrollTop = 300;
  scrolled(feed.element);
  feed.state.scrollHeight = 1500;
  resize();
  check(
    "growth while unpinned leaves the operator where they are",
    feed.scroll.pinned === false && feed.state.scrollTop === 300 && frames.length === 0,
  );

  feed.scroll.forcePin();
  check(
    "a send pins at once and jumps to the bottom",
    feed.scroll.pinned === true && feed.state.scrollTop === feed.bottom() && frames.length === 0,
  );
  feed.state.scrollHeight = 1600;
  resize();
  check(
    "growth right after a send jumps rather than easing",
    feed.state.scrollTop === feed.bottom() && frames.length === 0,
  );
  feed.state.scrollHeight = 1700;
  resize();
  check("the jump is spent: the next small growth eases again", frames.length === 1);
  feed.scroll.dispose();
}

// --- Disposal ----------------------------------------------------------------------------------

{
  const feed = start();
  feed.state.scrollHeight = 1100;
  resize();
  feed.scroll.dispose();
  check("disposal cancels a running follow", frames.length === 0);
  const before = feed.layouts.length;
  clock = 9000;
  wheel(feed.element);
  feed.state.scrollTop = 100;
  scrolled(feed.element);
  resize();
  check("a disposed controller ignores scrolls and resizes", feed.layouts.length === before);
}

if (failures.length > 0) {
  console.error(`feed-scroll: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("feed-scroll: all assertions passed");
process.exit(0);
