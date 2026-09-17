// A synthetic lazy feature directory for the panel-registry test: stands
// in for a feature barrel (src/ui/<feature>/index.ts) loaded through a
// panel type's import thunk. register() installs the panel factory, as the
// real barrels do, and records its own invocations on globalThis so the
// test can count them from outside the bundle. The returned disposable
// records its disposal the same way.
//
// The registry's registerPanelFactory arrives through a global injected
// by the test bundle: a static import of the TS source would resolve to a
// second module-instance copy of the registry when this file is bundled,
// and would break plain `node --test` discovery of this helper.
// Export-only fixture: running this file directly must (and does) exit 0.

export function register() {
  globalThis.__lazyFeatureRegisters = (globalThis.__lazyFeatureRegisters ?? 0) + 1;
  globalThis.__testRegisterPanelFactory("fake", () => ({
    element: Object.assign(document.createElement("div"), { className: "fake-lazy-panel" }),
    init(params) {
      this.element.dataset.params = JSON.stringify(params.params);
    },
    dispose() {
      globalThis.__lazyFeatureDisposes = (globalThis.__lazyFeatureDisposes ?? 0) + 1;
    },
  }));
  return {
    dispose() {
      globalThis.__lazyFeatureRegisterDisposed = true;
    },
  };
}
