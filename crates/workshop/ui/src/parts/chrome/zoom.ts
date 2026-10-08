// Global window zoom: the Ctrl+= / Ctrl+- / Ctrl+0 keybindings and the
// Window menu's zoom entries all dispatch through these functions, so
// every surface shares one factor. Zoom is a level from -8 to 8, and each
// level multiplies the factor by 1.2 (VS Code's zoomLevelToZoomFactor),
// so level 1 is 120% and level -1 is 83.33%. Desktop uses WebView2's
// viewport-aware zoom; Dockview's resize animation is disabled in
// zones.css so its dividers settle in the same paint. Browser mode uses
// CSS zoom. Both modes publish the factor as the --ws-zoom-factor custom
// property on the root, which the title bar's window controls divide by
// (window-chrome.css) so they keep their physical size.
//
// The factor persists as a bare number in the UI-state adapter's user
// bucket: the composition root restores it with restoreZoom(initial) at
// boot and installs the writer with persistZoom, and every later change
// goes through that writer. A stored factor maps back to the nearest
// level, so a value an older build wrote on a 0.1 grid still restores.
// The contribution's run bodies call zoomIn and
// friends directly, so the writer lives in this module rather than on an
// instance.

import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { toDisposable } from "@workshop/platform/lifecycle";
import type { IDisposable } from "@workshop/platform/lifecycle";

/** The writer each zoom change hands the new factor to. */
export type ZoomWriter = (value: unknown) => void;

const ZOOM_LEVEL_MIN = -8;
const ZOOM_LEVEL_MAX = 8;
const ZOOM_BASE = 1.2;
const ZOOM_DEFAULT_LEVEL = 0;
const ZOOM_FACTOR_PROPERTY = "--ws-zoom-factor";

// The last applied level is tracked here and restored from the user
// bucket at boot.
let currentLevel = ZOOM_DEFAULT_LEVEL;

// The installed writer; a no-op until the composition root binds one.
const noWriter: ZoomWriter = () => {};
let write: ZoomWriter = noWriter;

/** The factor a level maps to, rounded to four decimals so it stays readable and persists cleanly. */
function levelToFactor(level: number): number {
  return Math.round(ZOOM_BASE ** level * 10000) / 10000;
}

/** The current zoom factor; 1.0 is 100%. */
export function getZoom(): number {
  return levelToFactor(currentLevel);
}

/** The current zoom level, an integer from -8 to 8; 0 is 100%. */
export function getZoomLevel(): number {
  return currentLevel;
}

function applyToWindow(factor: number): void {
  document.documentElement.style.setProperty(ZOOM_FACTOR_PROPERTY, String(factor));
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

/** Clamps and applies a level without persisting it. */
function applyLevel(level: number): void {
  currentLevel = Math.min(ZOOM_LEVEL_MAX, Math.max(ZOOM_LEVEL_MIN, level));
  applyToWindow(levelToFactor(currentLevel));
}

/** Applies a level and writes the resulting factor through the installed writer. */
function setLevel(level: number): void {
  applyLevel(level);
  try {
    write(levelToFactor(currentLevel));
  } catch (error: unknown) {
    // The adapter reports its own failures; a throwing writer must not
    // undo a zoom that already applied.
    console.error("zoom persistence failed:", error);
  }
}

/** Ctrl+=: zoom one level larger, clamped at level 8. */
export function zoomIn(): void {
  setLevel(currentLevel + 1);
}

/** Ctrl+-: zoom one level smaller, clamped at level -8. */
export function zoomOut(): void {
  setLevel(currentLevel - 1);
}

/** Ctrl+NumPad0: back to 100%. */
export function resetZoom(): void {
  setLevel(ZOOM_DEFAULT_LEVEL);
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
 * bucket held. The factor maps to the nearest level; anything but a
 * positive finite number whose level lands within -8 to 8 - null, a
 * string, an object, zero, an out-of-range factor - leaves the default
 * 100% in place. The restore applies without writing: the value came from
 * the store, so echoing it back would be a wasted request.
 */
export function restoreZoom(initial: unknown): void {
  if (typeof initial !== "number" || !Number.isFinite(initial) || initial <= 0) {
    return;
  }
  const level = Math.round(Math.log(initial) / Math.log(ZOOM_BASE));
  if (level < ZOOM_LEVEL_MIN || level > ZOOM_LEVEL_MAX) {
    return;
  }
  applyLevel(level);
}
