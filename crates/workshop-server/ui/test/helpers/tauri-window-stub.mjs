// Test double for "@tauri-apps/api/window", substituted into the bundle by
// esbuild's `alias` in the window-chrome unit test. The real module talks
// to the Tauri runtime over window.__TAURI_INTERNALS__; this double records
// each native window command on window.__TAURI_STUB__ so the test can read
// them back out of the bundled module, and lets the test script the
// maximized state and fire resize events.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.

function state() {
  if (window.__TAURI_STUB__ === undefined) {
    window.__TAURI_STUB__ = { calls: [], maximized: false, resizeHandlers: [] };
  }
  return window.__TAURI_STUB__;
}

export function getCurrentWindow() {
  return {
    minimize() {
      state().calls.push("minimize");
      return Promise.resolve();
    },
    toggleMaximize() {
      const s = state();
      s.maximized = !s.maximized;
      s.calls.push("toggle-maximize");
      return Promise.resolve();
    },
    close() {
      state().calls.push("close");
      return Promise.resolve();
    },
    startDragging() {
      state().calls.push("drag");
      return Promise.resolve();
    },
    isMaximized() {
      return Promise.resolve(state().maximized);
    },
    onResized(handler) {
      state().resizeHandlers.push(handler);
      return Promise.resolve(() => {
        const s = state();
        s.resizeHandlers = s.resizeHandlers.filter((h) => h !== handler);
      });
    },
  };
}
