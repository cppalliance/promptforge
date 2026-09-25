// Test double for "@tauri-apps/api/event", substituted into the bundle by
// esbuild's `alias` in the workspace-files unit test. Only `emit` is
// doubled: it is the one export the bundled contributions import. Every
// emit records its event name and payload on window.__TAURI_EVENTS__ so
// the test can assert what the page told the desktop app; a scripted failure
// (window.__TAURI_EVENTS__.fail = true) rejects the emit the way a page
// without event permissions would.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.

function log() {
  if (window.__TAURI_EVENTS__ === undefined) {
    window.__TAURI_EVENTS__ = { emitted: [], fail: false };
  }
  return window.__TAURI_EVENTS__;
}

export function emit(event, payload) {
  const events = log();
  if (events.fail) {
    return Promise.reject(new Error("event.emit not allowed"));
  }
  events.emitted.push({ event, payload });
  return Promise.resolve();
}
