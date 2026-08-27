# Workshop UI Rules

These rules bind the embedded UI under `crates/promptforge-ws-server/ui/`. The repo-root and server-crate AGENTS.md apply on top.

## Vendored code is never edited

`src/chat/` is vendored (see its PROVENANCE.md): no reformatting, no restructuring, no comment edits. It is an opaque dependency; preserving the upstream diff is worth more than any local improvement.

## One-way layer imports

`ui` may import `services` may import `base`, never the reverse. `main.ts` is the composition root - it may import every layer, and nothing imports it. `chat/` is importable from `services` and `ui` as a dependency, never from `base`.

- `base/`: generic, DOM-free, app-agnostic primitives.
- `services/`: app-aware but DOM-free state and I/O.
- `ui/`: everything that touches the DOM.

The rule is defined once, in `check-layers.mjs`, and enforced three ways: an esbuild `onResolve` plugin in `build.mjs` (build and watch), a spawned walk in `build.rs` (a violation fails `cargo build`), and the `typecheck` script in `package.json` (CI signal).

## No module-level mutable shared state

Shared state lives in a service class with a change emitter, constructed once at the composition root and passed through constructors - never in module-level mutable variables.

## Tests

- Discovery is by glob only: `node --test "test/**/*.mjs" "src/**/*.test.mjs"` from `ui/`. No hand-ordered script chains.
- Tests live colocated next to their source (`*.test.mjs`) or under `test/`.
- Test names are plain English sentences describing the behavior under test, not the function under test.
- Tests that construct disposables run under `test/helpers/leak-check.mjs`: a test that leaks an undisposed `DisposableStore` fails.
