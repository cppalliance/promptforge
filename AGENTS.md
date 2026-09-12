# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime, inference gateway, and Workshop desktop product.

## Principles

- Do more with less. Prefer simple, foundational primitives over specific solutions: a primitive that naturally enables today's functionality and also generalizes beats a custom mechanism specified as a laundry list of requirements. Generality is the payoff, not a goal.
- When evaluating how to implement a capability, check whether the existing facilities subsume the work before building new machinery. Prioritize in this order:
  1. Reuse an existing facility
  2. Make the smallest improvement to an existing facility which enables the capability.
  3. Add a new facility. New machinery must have a material benefit beyond tidiness.
- When improving an existing facility, prefer an improvement that serves a problem class beyond the current case over one that solves only the case at hand, when the general shape costs no more.

## Roles

- Workshop is a user-facing agentic development environment: a Tauri desktop application with an HTML/CSS/TypeScript UI
- PromptForge is the runtime execution engine for the PromptForge Prompting Language: structured Markdown files with live Lua code fences
- Gateway is an independent service that proxies local and remote inference through one OpenAI-compatible HTTP and WebSocket endpoint

## Structure

- The three main products are PromptForge, Gateway, and Workshop
- Workshop crates are named workshop-* and must not depend on gateway crates
- Gateway crates are named gateway-* and must not depend on promptforge or workshop crates
- PromptForge crates are named promptforge-* and must not depend on gateway or workshop crates
- Shared crates are named shared-*, contain the public API surface across products and downstream crates, and must not depend on any product crates
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
