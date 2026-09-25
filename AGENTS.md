# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline engine, the harness that hosts it, the inference gateway, and the Workshop desktop product.

## Principles

- Do more with less. Prefer simple, foundational primitives over specific solutions: a primitive that naturally enables today's functionality and also generalizes beats a custom mechanism specified as a laundry list of requirements. Generality is the payoff, not a goal.
- When evaluating how to implement a capability, check whether the existing facilities subsume the work before building new machinery. Prioritize in this order:
  1. Reuse an existing facility
  2. Make the smallest improvement to an existing facility which enables the capability.
  3. Add a new facility. New machinery must have a material benefit beyond tidiness.
- When improving an existing facility, prefer an improvement that serves a problem class beyond the current case over one that solves only the case at hand, when the general shape costs no more.
- Error and status messages are designed assuming model consumption: concise, factual, and self-contained, naming what is missing or unmet with required versus actual, because a message may arrive as tool output that a model reasons about.

## Roles

- Workshop is a user-facing agentic development environment: a Tauri desktop application with an HTML/CSS/TypeScript UI
- PromptForge is the runtime execution engine for the PromptForge Prompting Language: structured Markdown files with live Lua code fences
- Gateway is an independent service that proxies local and remote inference through one OpenAI-compatible HTTP and WebSocket endpoint
- Harness is the engine's only production host: it owns the tokio runtime, the performers that execute the engine's effects, agent sessions, and the run log; Workshop and other clients drive runs through it

## Vocabulary

- **shell**: a command shell in a terminal, and nothing else.
- **desktop app**: the Tauri crate (package `workshop`), under `crates/workshop/desktop/`.
- **server**: the build check's tier that holds only `workshop-server`.
- **desk**: a UI's main frame.
- **workbench**: the VS Code-style UI architecture, and the Model menu snapshot frame on the `/ws` socket.
- **workshop socket**: the `/ws` socket.
- **page**: a routed screen behind a tab.
- **view**: a DOM component.
- **placeholder**: what a lazy panel shows while its code chunk loads.
- **entry bundle**: the eagerly loaded composition that lazy panels must never import.

## Structure

- The four main products are PromptForge, Gateway, Workshop, and Harness
- Workshop crates are named workshop-* and must not depend on gateway crates; workshop crates may name the gateway public pair, the promptforge public API, and `harness-api`
- Gateway's public surface is two root crates, `gateway-api-types` and `gateway-api-discovery`; everything else lives under crates/gateway/, a manifestless container private to the family - no outside crate may depend into it, and workshop crates may name only the public pair. Gateway crates must not depend on promptforge or workshop crates
- Workshop crates live under crates/workshop/, a manifestless container private to the family - no outside crate may depend into it; the desktop app is crates/workshop/desktop (package `workshop`), and the server and its subsystems sit beside it with short directory names
- Harness crates are named harness-*. Their public surface is one root crate, `harness-api`; everything else lives under crates/harness/, a fourth manifestless container private to the family, and `harness-api` is its one public API - the only outside crate permitted to depend into it. harness-* crates may depend on `promptforge`, `gateway-api-types`, `gateway-api-discovery`, and shared-* crates, never on workshop crates or on a private gateway crate; workshop crates may depend on harness-* only through `harness-api`; promptforge-* and gateway-* crates must not depend on harness crates
- The composed topology rule: a crate in a family container (crates/promptforge-internal/, crates/gateway/, crates/workshop/, crates/harness/) may depend only on crates at the crates/ root and its own siblings; the root is the public layer. Crates named build-* are meta tooling, exempt from container privacy
- PromptForge crates are named `promptforge` and promptforge-* and must not depend on gateway, workshop, or harness crates
- PromptForge has one public crate, `promptforge` at crates/promptforge/: a facade of single-item re-exports grouped into documented role modules. Crates outside the family may depend only on `promptforge`, never on a promptforge-* crate. Everything else lives under crates/promptforge-internal/, a manifestless container private to the family that holds the engine (`promptforge-engine`), the types crate (`promptforge-types`), the virtual filesystem (`promptforge-vfs`), and the lua, parser, store, and model-client crates; `promptforge` is the only outside crate permitted to depend into it
- The desktop app (the `workshop` crate) depends on `workshop-server-api` and never on `workshop-server`; the facade is the desktop app's entire view of the server
- Shared crates are named shared-*, contain the public API surface across products and downstream crates, and must not depend on any product crates. PromptForge's own public surface is the `promptforge` facade, and its types crate (`promptforge-types`) has left shared-* for the private container; Gateway's is gateway-api-types and gateway-api-discovery, named gateway-* now that both have left shared-*; the types crate contains the wire vocabulary only, never code
- Crates named build-* are for building specific outputs
- Dependency rules bind all kinds: normal, dev, build, and target-specific dependencies. One exception: a crate under crates/promptforge-internal/ may list `promptforge` in `[dev-dependencies]` only so its doc examples compile against the facade paths hosts see. No unit test, integration test, or bench imports it. This edge is exempt from the one-way flow rule under Structural Rules.

## Engineering

- Prefer types and compiler checks, then behavior tests and deterministic fault injection. Add a structural check only with explicit user approval for a stable product or security boundary that has no ordinary equivalent.
- Repository policy binds plans. A plan cannot introduce a source parser, snapshot, allowlist, count, ceiling, topology check, import walker, or other structural enforcement unless the user explicitly approves that exception.
- Behavior changes ship with tests in the same change. Preserve product and behavior tests during refactors. Structural tests that an approved plan identifies as unsupported may be removed without replacement by another structural proxy.
- A Cargo feature gates a real constraint such as a toolchain requirement or heavy native build. It does not describe product shape. Feature-disabled builds must not leak optional types into core paths.
- Runtime and serve paths never compile native dependencies or invoke build tools. Library and serve paths return failures instead of exiting the process or installing process-global state.
- Long-running gateway work reports through `gateway-progress`, a private gateway family crate: a producer begins an activity with a text, replaces the text as work moves, and drops the guard when done. Consumers outside the family read only the `Progress` wire type from `gateway-api-types`.
- Unsafe code stays in its explicitly owned boundary. Every unsafe block documents its safety invariants immediately before the block.
- Comments explain a non-obvious constraint, ordering requirement, or workaround. Every platform or external-bug workaround cites its upstream issue URL in the explanatory comment.
- JSON that reaches the run log or a replay comparison round-trips exactly - `to_value`, `to_string`, `from_str` yield an identical value, object keys stay canonical (sorted), numbers must be finite, and serde_json `preserve_order` is never enabled. Exact parsing (`float_roundtrip`) carries that guarantee; a value derived and then logged is additionally rounded to its meaningful precision at the source.

## Verification

- Full suite: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`). Clippy is a superset of `cargo check` and shares no artifacts with it, so never run a standalone `cargo check --workspace` beside the clippy runs; the one exception is the headless feature-combination gate `cargo check -p gateway --no-default-features`, which checks a build shape clippy --all-features does not cover.
- Formatter: `cargo fmt --all --check`.
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`. Rustdoc lints are not covered by clippy; never skip the docs gate.
- Facade docs: `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`, without `--all-features`, so the facade's docs build with default features.
- Facade surface: `cargo +<pinned nightly> xtask api --check`, where the pinned nightly is the one named in `crates/build-xtask/src/api/toolchain.rs`; on any other toolchain it fails at once, naming the nightly it needs. It checks that every path a surface item's signature, fields, bounds, impls, or doc links name is a facade re-export (or std, core, alloc, or an allowlisted crate), that no surface doc text names an internal crate, and that the surface listing matches the committed `crates/promptforge/public-api.txt`.
- Boundary and structural harness: `cargo test -p build-xtask`.

## Structural Rules

- Dependencies flow one way: server -> features -> services -> vocabulary. Never add a dependency from a lower tier to a higher one. If Cargo rejects a cycle, the design is wrong, not the graph. On the SPA side, lazy-loaded panels never import the entry bundle; shared code lives in services/ or base/.
- Every workshop-* and harness-* crate's lib.rs (including harness-api) opens with a //! doc containing a `## Invariants` marker that lists what the crate may depend on and what it may not. The marker is mandatory for those families by package name; a family crate without it fails `cargo test -p build-xtask`. The desktop app (the `workshop` crate) is exempt. Read the marker before adding an import.
- No file in a crate with the marker exceeds 500 lines. If an edit would push a file past 500, split first, then edit. `cargo test -p build-xtask` enforces the tier graph, the mandatory marker, the lint inheritance, the ceiling over the Rust files in every workshop-* and harness-* crate plus any other crate with the marker, and the product-boundary matrix above (including the single-public-crate rules for promptforge and harness and the container privacy rules for crates/promptforge-internal/, crates/gateway/, crates/workshop/, crates/harness/, and the nested crates/gateway/stt/ subsystem, whose only family-visible crate is gateway-stt) across every workspace manifest; the desktop app (the `workshop` crate) is exempt from the marker and the ceiling until the headless agent mode plan.
- Source directories are flat by default. A subdirectory of source files must contain at least three files; one or two files belong beside the parent module as `foo-bar.rs` (parent stem, dash, kebab label), wired with an explicit path attribute so the module name stays clean: `#[path = "foo-bar.rs"] mod bar;`. The two forms are convertible in both directions: when a `foo-*.rs` sibling group grows to three files, rehydrate it into a `foo/` subdirectory in standard module layout (`foo/bar.rs` beside `foo.rs`) and drop the path attributes; when a subdirectory shrinks below three files, flatten it back to kebab siblings. Apply whichever conversion applies when you touch files in a group on the wrong side of the line. Top-level `tests/` and `benches/` trees are exempt; they follow Cargo target conventions.

## SPA and CSS Rules

- CSS lives beside its TypeScript, never in a separate styles/ tree. A designer finds the styles for the agent chat at parts/agent/agent-session.css, not by grepping a flat directory. Every feature directory is self-contained: .ts, .css, and index.ts together.
- No raw color, size, or spacing values in component CSS. Use --ws-* tokens from tokens/. Primitives go in tokens/base.css, intent aliases in tokens/semantic.css, per-component overrides in tokens/component.css. A designer themes the app by editing semantic.css.
- No `localStorage`. The SPA never reads or writes browser storage; every persisted value goes through the `ui-storage` adapter to the server. UI state has two homes by scope: account state (preferences and ephemera alike, such as editor toggles, zoom, recent files, and command history) goes to `ui-state.json` in the state directory (the `workshop-user-state` crate, `/user/state`), and workspace-scoped state (anything that should travel with the `.pfwork` document) goes to the workspace file through the server (`/workspace/file/state`). A new persisted value is a new allow-listed key in one of those two buckets, added on the server first.
