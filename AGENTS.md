# PromptForge

Multi-crate Rust workspace for the Engine, the Harness that runs it, the Gateway, and Workshop, a Host.

## Definitions

Four words have exactly one meaning each, everywhere in this repository: code comments, docs, rulebooks, and plans. Write them capitalized.

- **Engine**: the `promptforge` and `promptforge-*` crates. The Engine parses a prompt and steps a run. Whenever a run needs a model reply, a tool result, a timer, or a file, the Engine emits an effect and waits for its caller to answer it. In production the caller is the Harness.
- **Harness**: the `harness` and `harness-*` crates. The Harness steps the Engine, performs every effect, returns each answer, and records every run through the recorder the Host supplies. Production code runs prompts only through the Harness.
- **Host**: an application that runs prompts through the Harness, such as Workshop or Papergate. The Host makes every policy decision. It runs prompts only through the Harness; it may also use the Engine's parser and types to read prompts and show events.
- **Plugin**: a `plugin-*` crate implementing `promptforge-plugin`, installed into a `HostContext` under a local name the Host chooses, such as `web`. Every tool it offers sits under that name (`web/fetch`). Every run receives every usable Plugin's tools: those of each installed Plugin that built and whose per-run services the run has. Declaring a Plugin under `plugins:` in a prompt's frontmatter installs its Lua prelude and makes it required, and the prompt's Lua decides what the model sees. A Plugin has no other name: never capability, pack, or addon.

### Using the terms

- "Host", in any capitalization and as hosts, hosted, or hosting, means only the Host. Other meanings use these words:
  - "Engine globals" for the Lua globals the Engine installs in every section, and "Engine call" for a call to one
  - "real files" and "the real filesystem" for the operating system's files
  - "machine" for the computer something runs on
  - the part's own name for a program or UI part that embeds another, such as "the desktop app" or "the container element"
  - "run", "serve", "embed", or "hold" for the verb
- "Engine", "Harness", and "Plugin" mean only the defined terms. Anything else gets a qualified lowercase name: the gateway's speech engine, the database, Rust's built-in test harness, a Tauri or ProseMirror plugin. Inside `crates/gateway/stt/`, a bare "engine" means the speech engine. This repository's own checks and test scaffolding are "structural checks", "test support", or "fixtures".
- Engine crates (`crates/promptforge` and `crates/promptforge-internal`: docs, comments, tests, strings, and code names) call the code that steps a run and answers its effects "the caller", and never mention the Host or a Host application. They name the Harness only to say that a responsibility belongs to it, never how the Harness performs it or which of its crates or types does. `crates/workshop/ui/test/docs-claims.mjs` enforces the parts a scan can see.
- Names defined outside this repository are used exactly as defined: the HTTP `Host` header and URL host names, the gateway config key `max_per_host`, Cargo's host and target vocabulary and `harness = false`, GitHub's self-hosted runners, cargo-dist's `host` step and `host-jobs`, CSS `:host`, and the DOM's `ShadowRoot.host`.
- Crate names are written as they are, such as `harness-runner` and `promptforge-engine`. Code names follow the same terms.
- Quotations of people stay verbatim.
- `crates/workshop/ui/test/docs-claims.mjs` enforces these rules in every `AGENTS.md`, every `## Invariants` crate doc, and every `.cursor/rules` file.

## Principles

- Do more with less. Prefer simple, foundational primitives over specific solutions: a primitive that naturally enables today's functionality and also generalizes beats a custom mechanism specified as a laundry list of requirements. Generality is the payoff, not a goal.
- When evaluating how to implement a feature, check whether the existing facilities subsume the work before building new machinery. Prioritize in this order:
  1. Reuse an existing facility
  2. Make the smallest improvement to an existing facility which enables the feature.
  3. Add a new facility. New machinery must have a material benefit beyond tidiness.
- When improving an existing facility, prefer an improvement that serves a problem class beyond the current case over one that solves only the case at hand, when the general shape costs no more.
- Error and status messages are designed assuming model consumption: concise, factual, and self-contained, naming what is missing or unmet with required versus actual, because a message may arrive as tool output that a model reasons about.

## Engineering

- Prefer types and compiler checks, then behavior tests and deterministic fault injection. Gate on yes/no properties, never on a score threshold that ordinary work edits.
- Behavior changes ship with tests in the same change. Preserve product and behavior tests during refactors.
- A Cargo feature gates a real constraint such as a toolchain requirement or heavy native build. It does not describe product shape. Feature-disabled builds must not leak optional types into core paths.
- Runtime and serve paths never compile native dependencies or invoke build tools.
- Long-running gateway work reports through `gateway-progress`, a private gateway family crate: a producer begins an activity with a text, replaces the text as work moves, and drops the guard when done. Consumers outside the family read only the `Progress` wire type from `gateway-api-types`.
- Unsafe code stays in its explicitly owned boundary.
- Comments explain a non-obvious constraint, ordering requirement, or workaround. Every platform or external-bug workaround cites its upstream issue URL in the explanatory comment.
- JSON that reaches a recorder or a replay comparison round-trips exactly - `to_value`, `to_string`, `from_str` yield an identical value, object keys stay canonical (sorted), numbers must be finite. Exact parsing (`float_roundtrip`) carries that guarantee; a value derived and then logged is additionally rounded to its meaningful precision at the source.

## Verification

- Full suite: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` (workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`); the gate runs both with `CARGO_BUILD_WARNINGS=deny`. Clippy reports everything `cargo check` does, so never run a standalone `cargo check --workspace` beside the clippy runs; the one exception is the headless feature-combination gate `cargo check -p gateway --no-default-features`, which checks a build shape clippy --all-features does not cover.
- Formatter: `cargo fmt --all --check`.
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`. Rustdoc lints are not covered by clippy; never skip the docs gate.
- Facade docs: `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`, without `--all-features`, so the facade's docs build with default features.
- Facade surface: `cargo +<pinned nightly> xtask api --check`, where the pinned nightly is the one named in `crates/build-xtask/src/api/toolchain.rs`; on any other toolchain it fails at once, naming the nightly it needs. It checks that every path a surface item's signature, fields, bounds, impls, or doc links name is a facade re-export (or std, core, alloc, or an allowlisted crate), that no surface doc text names an internal crate, and that the surface listing matches the committed `crates/promptforge/public-api.txt`.
- Boundary and structural checks: `cargo test -p build-xtask`. It enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Wording and rulebook scan: `node --test crates/workshop/ui/test/docs-claims.mjs`. It enforces the Engine wording rule, the rulebook Definitions, and the Plugin lifecycle wording (installed, snapshotted, or declared). It needs no `npm ci` and runs from any directory, because the file resolves the repository root from its own path.

## Structural Rules

- If Cargo rejects a dependency cycle, the design is wrong, not the graph.
- Source directories are flat by default. A subdirectory of source files must contain at least three files, and its count includes the files in its nested subdirectories; one or two files belong beside the parent module as `foo-bar.rs` (parent stem, dash, kebab label), wired with an explicit path attribute so the module name stays clean: `#[path = "foo-bar.rs"] mod bar;`. The two forms are convertible in both directions: when a `foo-*.rs` sibling group grows to three files, rehydrate it into a `foo/` subdirectory in standard module layout (`foo/bar.rs` beside `foo.rs`) and drop the path attributes; when a subdirectory shrinks below three files, flatten it back to kebab siblings. Both conversions apply at each level of nesting. Apply whichever conversion applies when you touch files in a group on the wrong side of the line. Top-level `tests/` and `benches/` trees are exempt; they follow Cargo target conventions.

## SPA and CSS Rules

- No raw color, size, or spacing values in Workshop UI component CSS; use the custom properties defined in `@workshop/look` and `ui/src/tokens/component.css`.
- Every persisted value goes through the `ui-storage` adapter to the server. UI state has two homes by scope: account state (preferences and ephemera alike, such as editor toggles, zoom, recent files, and command history) goes to `ui-state.json` in the state directory (the `workshop-user-state` crate, `/user/state`), and workspace-scoped state (anything that should travel with the `.pfwork` document) goes to the workspace file through the server (`/workspace/file/state`). A new persisted value is a new allow-listed key in one of those two buckets, added on the server first.
