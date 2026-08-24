# Vendored: murm-ui

- Source: <https://github.com/levmv/murm-ui>
- Version: 0.2.0 (npm release; upstream has no `v0.2.0` git tag, so the
  release commit was used)
- Commit: `336ff7db79d928373e83c3672db6041a0adbc868` "chore: prepare 0.2.0
  release" (main HEAD at fetch time)
- License: MIT (see `LICENSE` in this directory)
- Fetched: 2026-08-24

## What this is

The full TypeScript source of murm-ui 0.2.0 (`src/` in the upstream repo),
vendored so the workbench can adapt the chat UI to its own transport
(WebSocket provider, observer integration) and palette without waiting on
upstream. The npm package ships only compiled `dist/`, which is why the
source comes from the git repository rather than the tarball.

## Deviations from upstream

- Test files excluded: every `*.test.ts` and `tsconfig.test.json` (they
  need the upstream tsx/jsdom harness, which is not vendored).
- No import or code changes yet: relative imports are extensionless, which
  esbuild resolves natively, and the lone runtime dependency `marked` is a
  workspace npm dependency.
