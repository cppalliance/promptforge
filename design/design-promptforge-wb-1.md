# Design Choices: promptforge-wb stage 1

Running log of design choices for the PromptForge Workbench stage 1 build. Each entry states the choice, the evidence behind it, and the cost it carries.

## 1. Two crates, promptforge-wb-server and promptforge-wb, as workspace members

- Choice: the workbench is split into an HTTP server binary (`promptforge-wb-server`) and a desktop window shell binary (`promptforge-wb`), both globbed into the promptforge workspace as `crates/*` members.
- Evidence: the workspace already ships one binary per concern (gateway, mcp-server, cli), and the workbench needs a local HTTP API the shell and a browser can both reach, which is two processes with different dependency stacks.
- Cost: two binaries to build, run, and eventually package together; the shell cannot call server internals directly and must go through the HTTP API.

## 2. axum for the HTTP server

- Choice: the workbench server is built on axum 0.8 with tower for testing.
- Evidence: axum is the ecosystem default HTTP server in the rust rulebook, is already a workspace dependency used by promptforge-gateway, and its Router is directly testable with tower's `ServiceExt::oneshot` without binding a port.
- Cost: pulls the tokio/tower stack into the workbench; handlers are async even where the work is synchronous.

## 3. Default bind 127.0.0.1:7910

- Choice: the server binds to 127.0.0.1:7910 when no override is given.
- Evidence: the workbench is a local companion to the desktop shell, so loopback-only is the safe default; 7910 is an unassigned high port that does not collide with common development servers.
- Cost: the port is fixed rather than discovered, so a collision fails the bind instead of picking another port; remote access requires an explicit future override.

## 4. wry/tao deferred to the shell step

- Choice: `promptforge-wb` is an empty binary skeleton; the wry/tao window and its dependency tree are not added yet.
- Evidence: wry/tao pull in large platform GUI stacks (WebView2, gtk) that dominate build time, and nothing in the scaffolding step needs a window; keeping them out lets the workspace build fast while the server takes shape.
- Cost: the shell cannot be run end to end until the shell step lands, and the wry/tao integration risk is concentrated there instead of spread out.

## 5. Fresh config module in promptforge-wb-server, not promptforge-gateway-config reuse

- Choice: `workbench.toml` loading lives in a small `config` module inside promptforge-wb-server; the promptforge-gateway-config crate is not a dependency.
- Evidence: gateway-config's `Config` models the gateway's own server (models, endpoints, dominions, profiles, recursive includes), a different shape from the workbench's client-side view (`gateway.base_url`, `gateway.api_key`, `tape.path`, `server.bind`), and its `${VAR}` interpolation is `pub(crate)`, so reuse would require widening that crate's public surface for a consumer with no overlap in schema. The workbench mirrors its convention instead: parse TOML to a `toml::Value`, interpolate string leaves only (CFG-007 semantics), then deserialize, with `$$` as a literal `$` and unresolved variables as errors.
- Cost: about 80 lines of interpolation logic duplicated in spirit; the two crates now evolve the same convention in parallel and could drift.

## 6. Verbatim relay via raw bytes, statuses passed through

- Choice: the gateway client returns status plus raw body bytes, and the routes relay them with content-type `application/json`; a non-success gateway status is relayed, not mapped to a workbench error.
- Evidence: the step requires byte-for-byte relay, and re-serializing a `serde_json::Value` cannot guarantee key order (serde_json's default map sorts keys), so raw bytes are the only honest pass-through; relaying the gateway's own error envelopes keeps failure semantics in one place.
- Cost: the workbench cannot inspect or transform gateway responses at this layer, and the content-type is asserted rather than copied from the gateway response.

## 7. Route shapes: GET /v1/models mirrors the gateway, POST /chat is workbench-local

- Choice: the models proxy keeps the gateway's own path `/v1/models`; chat is `POST /chat` accepting `{"model", "messages"}` as a typed `ChatRequest` whose messages are unchecked `serde_json::Value` items; handlers reach the gateway client through axum `State<AppState>`.
- Evidence: mirroring `/v1/models` lets OpenAI-aware clients point at the workbench unchanged, while `/chat` is named for the workbench UI and typing only model plus messages keeps the stage-2 contract minimal; axum state is the idiomatic way to share one connection-pooled client across handlers.
- Cost: two naming conventions on one server, and extra fields in a chat request are silently dropped rather than forwarded.

## 8. Error design: one thiserror enum per module, boxed sources

- Choice: `ConfigError` (Read / Parse / UnresolvedVar / Interpolation) and `GatewayError` (Build / Transport / ReadBody), both `#[non_exhaustive]`, with dependency errors boxed as `Box<dyn Error + Send + Sync>` behind `#[source]`; handlers collapse any `GatewayError` to `502 Bad Gateway` with a JSON envelope.
- Evidence: matches the rust rulebook's one-enum-per-unit-of-fallibility and the gateway crate's own convention of boxing reqwest errors so dependency types stay out of the public API; a missing `workbench.toml` surfaces as `Read` naming the expected path.
- Cost: boxing erases the concrete source type for any caller that wanted to downcast, and the 502 mapping loses the distinction between connect, transport, and body-read failures at the HTTP boundary.
