# crates/

`crates/` is the workspace's public and shared layer - the family containers (`promptforge-internal/`, `gateway/`, `workshop/`, `harness-internal/`) are private, and cross-family dependencies resolve only here.

## gateway-api-types

The gateway's public vocabulary crate: the versioned provider model sheet schema, the model vocabulary, and the `Progress` busy-and-text snapshot, all as pure serde data types. The gateway family reads and writes the sheet through it, the Workshop decodes progress through it, and it is one of the two gateway crates outside crates may name. Types only, no code. No workspace dependencies.

## gateway-api-discovery

The gateway discovery seam: the `gateway.json` discovery file, the launch lock, stale detection, and the health probe. The gateway writes it at boot and the workshop shell, server, and gateway client read it to attach to a running gateway. No workspace dependencies.

## promptforge

The PromptForge API: the one promptforge crate outside crates may name. A facade of single-item re-exports grouped into documented role modules - prompt parsing, the sans-IO `Run` state machine that executes sections as effects the Harness performs, and the effect, event, model, tool, capability, and vfs vocabulary those effects carry. The Harness and the workshop crates reach the Engine only through it. Its surface is committed as `public-api.txt` and checked by `cargo xtask api --check`. Depends on the crates in `promptforge-internal/` that define what it re-exports. The first-party capabilities and the tool implementations behind a run live in the Harness, not here.

## harness-gateway-client

The standard way a Host talks to the PromptForge Gateway: `GatewayBroker`, the `harness::InferenceBroker` a Gateway Host passes to `Harness::new`, which runs each model round on a `GatewayChat` and lists the Gateway's models through `fetch_model_catalog`; `GatewayChat`, which sends a `Chat` effect's round over HTTP and streams the reply back; and `GatewaySearch`, the `harness_web::SearchProvider` a Gateway Host supplies, which runs one web search through the Gateway's search relay under a 30-second deadline. Under them sits the OpenAI chat-completions wire code that turns a `Chat` effect into the one request body every round sends, reads the streamed reply over a caller's `ChunkSource` into a `Completion` under the byte cap and the `[DONE]` rule, and classifies a failed response into a `CompletionError`. The wire code opens no connection and reads no clock, so another broker can reuse it. A root Harness crate beside `harness`; among the workspace crates it depends on `promptforge`, `harness`, and `harness-web`.

## harness-web

The `promptforge/web` capability a Host registers: `Web`, which contributes the fetch tool and the search tool as one pair. The fetch tool is the SSRF boundary between a model-supplied URL and the network, and every fetch runs on the tokio runtime handle the Host provides under `TOKIO_RUNTIME`. The search tool validates the model's arguments and runs the search through the `SearchProvider` the Host provides under `SEARCH_PROVIDER`; Workshop's provider delegates to `GatewaySearch`. A root Harness crate beside `harness`; it depends on `harness` and `promptforge` among the workspace crates, and `harness-gateway-client` depends on it for `SearchProvider`.

## shared-error-source

The shared error-source wrappers: `JsonSource`, `HttpSource`, and `DatabaseSource`, one crate-owned newtype per third-party error (`serde_json`, `reqwest`, `turso`) a public error surface would otherwise name. Each sits behind its own feature (`json`, `http`, `database`) so a consumer takes only the third-party dependency it already has. The workshop and gateway families wrap their causes through it; Harness crates may not depend on it, because `promptforge` is their only outside dependency. No workspace dependencies - that independence is what keeps it off the cross-family edge.

## shared-loopback

The loopback wall: the `require_loopback` and `require_loopback_host` middleware plus the per-product WebSocket origin policies. The gateway applies it to the admin surface and every loopback-bound build, and config-ui wraps its SPA assets with it. No workspace dependencies; axum is the only third-party crate.

## workspace-hack

The hakari-managed feature-unification crate: a pinned third-party feature set with no product logic. Nearly every workspace crate depends on it so `-p` and `--workspace` builds stop rebuilding shared dependencies. No workspace dependencies; managed by `cargo hakari` per `.config/hakari.toml`.

## build-llama-cuda

Builds the CUDA llama-server release zip from a llama.cpp checkout for Windows x64. Release tooling only; nothing depends on it. No workspace dependencies.

## build-ui

The build-script helper that bundles a crate's `ui/` with esbuild into `OUT_DIR`. workshop-server and gateway-config-ui use it as a build dependency. No workspace dependencies.

## build-user-guide

Checks the user guide chapters in `guide/src/<set>/` and writes the per-set exports. Its `stage <out>` mode writes one mdBook tree per book, with each book's summary and per-part indexes, for `cargo xtask site`. Run by hand and in CI; nothing depends on it. No workspace dependencies.

## build-workshop

The desktop release orchestrator: builds the gateway, stages the sidecar, builds the workshop app, and cleans up. It drives the `cargo workshop` alias; nothing depends on it. No workspace dependencies.

## build-xtask

Workspace automation: the new-crate scaffolder, the tidy checks (tier graph, lint inheritance, file ceiling), and the product-boundary matrix. `cargo test -p build-xtask` runs the structural checks every change must pass. No workspace dependencies.

Note: `shared-ui` is not a Rust crate - it is the TypeScript+CSS package the Gateway config UI consumes (the Workshop uses its fork, `workshop/look`), so the `crates/*` member glob skips it.
