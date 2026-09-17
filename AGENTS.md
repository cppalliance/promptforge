# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime, inference gateway, and Workshop desktop product.

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

## Structure

- The three main products are PromptForge, Gateway, and Workshop
- Workshop crates are named workshop-* and must not depend on gateway crates
- Gateway's public surface is two root crates, `gateway-api` and `gateway-api-discovery`; everything else lives under crates/gateway/, a manifestless container private to the family - no outside crate may depend into it, and workshop crates may name only the public pair. Gateway crates must not depend on promptforge or workshop crates
- Workshop crates live under crates/workshop/, a manifestless container private to the family - no outside crate may depend into it; the shell is crates/workshop/shell (package `workshop`), and the server and its subsystems sit beside it with short directory names
- The composed topology rule: a crate in a family container (crates/promptforge/, crates/gateway/, crates/workshop/) may depend only on crates at the crates/ root and its own siblings; the root is the public layer. Crates named build-* are meta tooling, exempt from container privacy
- PromptForge crates are named promptforge-* and must not depend on gateway or workshop crates
- PromptForge is one door: crates outside the promptforge-* family may depend only on promptforge-api-runtime and promptforge-api-types, never on the internal promptforge-* substrate crates; the crates under crates/promptforge/ are private to the family, and promptforge-api-runtime is the only outside crate permitted to depend into them
- The Workshop shell (the `workshop` crate) depends on `workshop-server-api` and never on `workshop-server`; the facade is the shell's entire view of the server
- Shared crates are named shared-*, contain the public API surface across products and downstream crates, and must not depend on any product crates. PromptForge's own public surface is promptforge-api-runtime and promptforge-api-types, named promptforge-* now that the types crate has left shared-*; Gateway's is gateway-api and gateway-api-discovery, named gateway-* now that both have left shared-*
- Crates named build-* are for building specific outputs
- Dependency rules bind all kinds: normal, dev, build, and target-specific dependencies

## Engineering

- Prefer types and compiler checks, then behavior tests and deterministic fault injection. Add a structural check only with explicit user approval for a stable product or security boundary that has no ordinary equivalent.
- Repository policy binds plans. A plan cannot introduce a source parser, snapshot, allowlist, count, ceiling, topology check, import walker, or other structural enforcement unless the user explicitly approves that exception.
- Behavior changes ship with tests in the same change. Preserve product and behavior tests during refactors. Structural tests that an approved plan identifies as unsupported may be removed without replacement by another structural proxy.
- A Cargo feature gates a real constraint such as a toolchain requirement or heavy native build. It does not describe product shape. Feature-disabled builds must not leak optional types into core paths.
- Runtime and serve paths never compile native dependencies or invoke build tools. Library and serve paths return failures instead of exiting the process or installing process-global state.
- Long-running work reports through `shared-progress`. Producers report operation state, hosts forward it, and renderers format it.
- Unsafe code stays in its explicitly owned boundary. Every unsafe block documents its safety invariants immediately before the block.
- Comments explain a non-obvious constraint, ordering requirement, or workaround. Every platform or external-bug workaround cites its upstream issue URL in the explanatory comment.

## Verification

- Full suite: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`). Clippy is a superset of `cargo check` and shares no artifacts with it, so never run a standalone `cargo check --workspace` beside the clippy runs; the one exception is the headless feature-combination gate `cargo check -p gateway --no-default-features`, which checks a build shape clippy --all-features does not cover.
- Formatter: `cargo fmt --all --check`.
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`. Rustdoc lints are not covered by clippy; never skip the docs gate.
- Boundary and structural harness: `cargo test -p build-xtask`.

## Structural Rules

- Dependencies flow one way: shell -> features -> services -> vocabulary. Never add a dependency from a lower tier to a higher one. If Cargo rejects a cycle, the design is wrong, not the graph. On the SPA side, lazy-loaded panels never import the boot shell; shared code lives in services/ or base/.
- Every workshop-* crate's lib.rs opens with a //! doc carrying a `## Invariants` marker that lists what the crate may depend on and what it may not. Read it before adding an import. Every SPA concern directory (ui/editor/, ui/agent/, etc.) has the same in its index.ts.
- No file exceeds 500 lines. If an edit would push a file past 500, split first, then edit. `cargo test -p build-xtask` enforces the tier graph, the lint inheritance, the ceiling over the Rust files in the workshop crates carrying the marker, and the product-boundary matrix above (including the one-door rule and the container privacy rules for crates/promptforge/, crates/gateway/, crates/workshop/, and the nested crates/gateway/stt/ subsystem, whose only family-visible crate is gateway-stt) across every workspace manifest; `cargo test -p gateway-stt --test it architecture` checks the same product matrix from cargo metadata; the Tauri shell (the `workshop` crate) is exempt until the headless agent mode plan.
- Source directories are flat by default. A subdirectory of source files must contain at least three files; one or two files belong beside the parent module as `foo-bar.rs` (parent stem, dash, kebab label), wired with an explicit path attribute so the module name stays clean: `#[path = "foo-bar.rs"] mod bar;`. The two forms are convertible in both directions: when a `foo-*.rs` sibling group grows to three files, rehydrate it into a `foo/` subdirectory in standard module layout (`foo/bar.rs` beside `foo.rs`) and drop the path attributes; when a subdirectory shrinks below three files, flatten it back to kebab siblings. Apply whichever conversion applies when you touch files in a group on the wrong side of the line. Top-level `tests/` and `benches/` trees are exempt; they follow Cargo target conventions.

## SPA and CSS Rules

- CSS lives beside its TypeScript, never in a separate styles/ tree. A designer finds the styles for the agent chat at ui/agent/agent-session.css, not by grepping a flat directory. Every feature directory is self-contained: .ts, .css, and index.ts together.
- No raw color, size, or spacing values in component CSS. Use --ws-* tokens from tokens/. Primitives go in tokens/base.css, intent aliases in tokens/semantic.css, per-component overrides in tokens/component.css. A designer themes the app by editing semantic.css.
- No `localStorage`. The SPA never reads or writes browser storage; every persisted value goes through the `ui-storage` adapter to the server. UI state has two homes by scope: account state (preferences and ephemera alike, such as editor toggles, zoom, recent files, and command history) goes to `ui-state.json` in the state directory (the `workshop-user-state` crate, `/user/state`), and workspace-scoped state (anything that should travel with the `.pfwork` document) goes to the workspace file through the server (`/workspace/file/state`). A new persisted value is a new allow-listed key in one of those two buckets, added on the server first.
