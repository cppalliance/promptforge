// Global window zoom: the Ctrl+= / Ctrl+- / Ctrl+0 keybindings and the
// Window menu's zoom entries all dispatch through these functions, so
// every surface shares one factor. Desktop uses WebView2's viewport-aware
// zoom; Dockview's resize animation is disabled in zones.css so its
// dividers settle in the same paint. Browser mode uses CSS zoom.
//
// The factor persists as a bare number in the UI-state adapter's user
// bucket: the composition root restores it with restoreZoom(initial) at
// boot and installs the writer with persistZoom, and every later change
// goes through that writer. The contribution's run bodies call zoomIn and
// friends directly, so the writer lives in this module rather than on an
// instance.

import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { toDisposable } from "../../base/lifecycle";
import type { IDisposable } from "../../base/lifecycle";

/** The writer each zoom change hands the new factor to. */
export type ZoomWriter = (value: unknown) => void;

const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2.0;
const ZOOM_STEP = 0.1;
const ZOOM_DEFAULT = 1.0;

// The last applied factor is tracked here and restored from the user
// bucket at boot.
let currentZoom = ZOOM_DEFAULT;

// The installed writer; a no-op until the composition root binds one.
const noWriter: ZoomWriter = () => {};
let write: ZoomWriter = noWriter;

/** The current zoom factor; 1.0 is 100%. */
export function getZoom(): number {
  return currentZoom;
}

function applyToWindow(factor: number): void {
  if (window.__TAURI_INTERNALS__ !== undefined) {
    void getCurrentWebviewWindow()
      .setZoom(factor)
      .catch((error: unknown) => {
        console.error("native zoom failed:", error);
      });
    return;
  }
  document.body.style.position = "relative";
  document.documentElement.style.zoom = String(factor);
}

/** Clamps and applies a factor without persisting it. */
function applyFactor(factor: number): void {
  // The 0.1 step accumulates binary float error (0.7 + 0.1 !== 0.8), so
  // every write normalizes back to one decimal place.
  currentZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(factor * 10) / 10));
  applyToWindow(currentZoom);
}

/** Applies a factor and writes it through the installed writer. */
function setFactor(factor: number): void {
  applyFactor(factor);
  try {
    write(currentZoom);
  } catch (error: unknown) {
    // The adapter reports its own failures; a throwing writer must not
    // undo a zoom that already applied.
    console.error("zoom persistence failed:", error);
  }
}

/** Ctrl+=: zoom one step larger, clamped at 2.0. */
export function zoomIn(): void {
  setFactor(currentZoom + ZOOM_STEP);
}

/** Ctrl+-: zoom one step smaller, clamped at 0.5. */
export function zoomOut(): void {
  setFactor(currentZoom - ZOOM_STEP);
}

/** Ctrl+0: back to 100%. */
export function resetZoom(): void {
  setFactor(ZOOM_DEFAULT);
}

/**
 * Installs the writer every later zoom change goes through, replacing the
 * previous one. Disposing restores the no-op writer.
 */
export function persistZoom(writer: ZoomWriter): IDisposable {
  write = writer;
  return toDisposable(() => {
    if (write === writer) {
      write = noWriter;
    }
  });
}

/**
 * Re-applies the persisted zoom factor at boot from the value the user
 * bucket held. Anything but a finite number within range - null, a
 * string, an object, an out-of-range factor - leaves the default 100% in
 * place. The restore applies without writing: the value came from the
 * store, so echoing it back would be a wasted request.
 */
export function restoreZoom(initial: unknown): void {
  if (typeof initial !== "number" || !Number.isFinite(initial)) {
    return;
  }
  if (initial < ZOOM_MIN || initial > ZOOM_MAX) {
    return;
  }
  applyFactor(initial);
}
