# Workshop UI Rules

These rules bind the embedded UI under `crates/workshop-server/ui/`. The repo-root and server-crate AGENTS.md apply on top.

## One-way layer imports

`ui` may import `services` may import `base`, never the reverse. `main.ts` is the composition root - it may import every layer, and nothing imports it.

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

## Cursor visual parity

- Cursor Dark is the visual specification for workshop chrome and agent controls. Copy exact values and structure from Cursor's installed source and `cursor-dark-color-theme.json`; never approximate from a screenshot or substitute a similar token.
- Do not mix Cursor surfaces. The workshop agent panel targets Cursor's workspace-sidebar agent in `.part.auxiliarybar`, including `.composer-bar` and `.ai-input-full-input-box`, not the Glass Agents Window or editor-tab agent.
- Use the matching token system: VS Code theme tokens for workbench chrome, Cursor design tokens for Cursor-native agent controls. Preserve Cursor's original `color-mix()` formulas; never encode premultiplied RGB as straight-alpha hex.
- Before claiming parity, compare PromptForge and Cursor at 100% zoom for the same surface and state. If no visual comparison is available, say that verification is pending.
- Cursor menus and controls define focus through state backgrounds, opacity, or underlines. Focus frames are globally suppressed; never add an outline, ring, or focus-like box-shadow unless the matching Cursor control visibly has one.
- Keep source-text tests synchronized with copied values. Tests must pin the Cursor value or structure, not a previous PromptForge approximation.
