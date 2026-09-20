// Test double for "@tauri-apps/api/webviewWindow", substituted into the
// bundle by esbuild's `alias` in the zoom unit test. The real module talks
// to the Tauri runtime over window.__TAURI_INTERNALS__; this double records
// each setZoom factor on window.__TAURI_WEBVIEW_STUB__ so the test can read
// them back out of the bundled module.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.

function state() {
  if (window.__TAURI_WEBVIEW_STUB__ === undefined) {
    window.__TAURI_WEBVIEW_STUB__ = { zooms: [] };
  }
  return window.__TAURI_WEBVIEW_STUB__;
}

export function getCurrentWebviewWindow() {
  return {
    setZoom(factor) {
      state().zooms.push(factor);
      return Promise.resolve();
    },
  };
}
