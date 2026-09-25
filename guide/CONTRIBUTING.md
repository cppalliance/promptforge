# Contributing to the guide

The guide has three documentation sets, one per audience: `src/gateway/`, `src/language/`, and `src/agent/`. Chapters inside a set start with a numeric prefix that fixes the reading order.

## Ownership

- `src/SUMMARY.md` and the per-part `src/<set>/index.md` files belong to the assembler. Do not hand-edit them; regenerate them with `cargo run -p build-user-guide`.
- Chapters and `src/introduction.md` are hand-edited. Fixes land directly in the file.

## Freshness

There is no freshness gate. The `guide.yml` workflow only builds and deploys the checked-in book. No generator exists for these sets yet, so update a chapter by hand when its sources change.

## House rules

- No em-dash and no double-dash. Use a single dash.
- Open every code fence with four backticks.
- Keep each paragraph on one line.
