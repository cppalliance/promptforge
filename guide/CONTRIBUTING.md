# Contributing to the guide

The guide has four documentation sets, one per audience: `src/workshop/`, `src/gateway/`, `src/language/`, and `src/agent/`. Chapters inside a set carry a numeric prefix that fixes the reading order.

## Ownership

Two files kinds are generated. Do not hand-edit them.

- `src/SUMMARY.md` and the per-part `src/<set>/index.md` files belong to the assembler. Regenerate them with `cargo run -p build-user-guide`.
- `src/introduction.md` belongs to the generator. Regenerate it by running `tools/document.md` with the `intro` lens.

Chapters are hand-editable. Small fixes land directly in the chapter file.

## Freshness

There is no freshness gate. CI never runs the generator; the `guide.yml` workflow only builds and deploys the checked-in book. Sets regenerate on demand when the sources have moved enough to matter.

## Rebuilding

The generator is `tools/document.md`, a harness tool file. Run it in Cursor or Claude Code.

- Run it with a lens name (`workshop`, `gateway`, `language`, `agent`, `intro`) to rebuild one set.
- Run it with no lens to rebuild everything: the four sets in audience order, then the introduction, then the assembler.
- Delete `guide/scratch/` first to force a full rebuild from zero.

Regeneration is surgical by default. The reuse rule keeps complete drafts and gate verdicts in `guide/scratch/<set>/`, so a re-run overwrites a hand-edited chapter only if that chapter's draft or verdict file was deleted first. To regenerate one chapter, delete its draft under `guide/scratch/<set>/drafts/` and re-run the lens.

## House rules

- No em-dash and no double-dash. Use a single dash.
- Open every code fence with four backticks.
- Keep each paragraph on one line.
