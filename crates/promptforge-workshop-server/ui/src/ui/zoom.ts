// Global window zoom: the Ctrl+= / Ctrl+- / Ctrl+0 keybindings and the
// Window menu's zoom entries all dispatch through these functions, so
// every surface shares one factor. Desktop uses WebView2's viewport-aware
// zoom; Dockview's resize animation is disabled in zones.css so its
// dividers settle in the same paint. Browser mode uses CSS zoom.

import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/** The localStorage key carrying the persisted zoom factor. */
export const ZOOM_STORAGE_KEY = "promptforge.workshop.zoom";

const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2.0;
const ZOOM_STEP = 0.1;
const ZOOM_DEFAULT = 1.0;

// The last applied factor is tracked here and restored from storage at boot.
let currentZoom = ZOOM_DEFAULT;

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

function setFactor(factor: number): void {
  // The 0.1 step accumulates binary float error (0.7 + 0.1 !== 0.8), so
  // every write normalizes back to one decimal place.
  currentZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(factor * 10) / 10));
  applyToWindow(currentZoom);
  try {
    window.localStorage.setItem(ZOOM_STORAGE_KEY, String(currentZoom));
  } catch (error: unknown) {
    // Storage can be unavailable (private mode); the zoom still applies.
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
 * Re-applies the persisted zoom factor at boot. A missing, corrupt, or
 * out-of-range stored value leaves the default 100% in place.
 */
export function restoreZoom(): void {
  const raw = window.localStorage.getItem(ZOOM_STORAGE_KEY);
  if (raw === null) {
    return;
  }
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed < ZOOM_MIN || parsed > ZOOM_MAX) {
    return;
  }
  setFactor(parsed);
}
