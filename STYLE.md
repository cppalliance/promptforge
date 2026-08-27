# Style

House style for source comments, errors, and structure. Subordinate to AGENTS.md: "do more with less" outranks everything here.

## Scope

The workshop sections below bind the workshop crates - `crates/promptforge-ws` and `crates/promptforge-ws-server`, including the embedded UI under `crates/promptforge-ws-server/ui/`. Vendored code (`ui/src/chat/`, see its PROVENANCE.md) is exempt from all of it: no reformatting, no restructuring, no comment edits.

## Comment policy

`///` doc comments on public items are mandatory (AGENTS.md; `missing_docs` warns) and are not what this section governs. For every other comment, write it only when it earns its place. A comment is justified by exactly four things:

- A non-obvious why: the reason for a decision that the code cannot express.
- An invisible constraint: something that must hold but no type or test enforces.
- An external-bug workaround: code shaped by a defect in a dependency or platform.
- A subtle ordering requirement: statements that look reorderable but are not.

Comments that narrate what the code already says are deleted on sight. File prologues follow the same rule: a module doc earns its place by documenting the domain, not by restating the file name.

## Workaround convention

Every platform or external-bug workaround carries its upstream issue URL inline, in the comment that explains the workaround. When the workaround dies, the URL says when it can be buried; without it, nobody ever dares.

```rust
// wry's drag-drop handler suppresses HTML5 drag events on Windows
// (https://github.com/tauri-apps/tauri/issues/15138), so ...
```

## Two-zone error policy

The process has two zones with opposite failure postures. The boundary: config load plus window/server construction is zone one; request and session handling is zone two.

- Zone one (construction/startup): fail loudly and immediately. Panics, `expect`, and hard process exit are all correct - a misconfigured process must not limp into serving.
- Zone two (steady state): never panic. Errors are values: they become error frames, 4xx/5xx responses, status-bus reports, or logged degradation. A lock poisoned by a panicking peer recovers the value rather than wedging the process.

Degrade-not-crash features (voice provisioning, gateway outages) are zone two by definition: absence of a capability is a state, not an error.

## Workshop UI layers

One-way imports: `ui` may import `services` may import `base`, never the reverse. `main.ts` is the composition root - it may import every layer, and nothing imports it. `chat/` is vendored and opaque (`ui/src/chat/PROVENANCE.md`): importable from `services` and `ui` as a dependency, never from `base`, and never edited under this document.

- `base/`: generic, DOM-free, app-agnostic primitives.
- `services/`: app-aware but DOM-free state and I/O.
- `ui/`: everything that touches the DOM.

The rule is defined once, in `ui/check-layers.mjs`, and enforced three ways: an esbuild `onResolve` plugin in `build.mjs` (build and watch), a spawned walk in `build.rs` (a violation fails `cargo build`), and the `typecheck` script in `package.json` (CI signal).

## Workshop UI tests

- Discovery is by glob only: `node --test "test/**/*.mjs" "src/**/*.test.mjs"` from `ui/`. No hand-ordered script chains.
- Tests live colocated next to their source (`*.test.mjs`) or under `ui/test/`.
- Test names are plain English sentences describing the behavior under test, not the function under test.
- Tests that construct disposables run under `ui/test/helpers/leak-check.mjs`: a test that leaks an undisposed `DisposableStore` fails.
