// Test double for "@tauri-apps/plugin-dialog", substituted into the bundle
// by esbuild's `alias` in the workshop-panel and files-actions unit tests.
// The test scripts each pick's answer through window.__TAURI_DIALOG__.answer
// (a path string, or null for a cancelled dialog) and reads back the options
// each open or save received; every call records its kind ("open" or "save")
// beside the caller's options.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.

function record(kind, options) {
  if (window.__TAURI_DIALOG__ === undefined) {
    window.__TAURI_DIALOG__ = { calls: [], answer: null };
  }
  window.__TAURI_DIALOG__.calls.push({ kind, ...options });
  return Promise.resolve(window.__TAURI_DIALOG__.answer);
}

export function open(options) {
  return record("open", options);
}

export function save(options) {
  return record("save", options);
}
