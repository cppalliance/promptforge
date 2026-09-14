---
name: CI and hygiene fixes
overview: "Fix the three highest-priority findings from the quality assessment: CI native prerequisites, two Clippy lint failures, and the gateway client missing request timeouts."
todos:
  - id: ci-fix
    content: "Commit 1: CI platform-aware matrix, exclude workbench from Ubuntu, add Windows workbench job"
    status: completed
  - id: clippy-fix
    content: "Commit 2: Fix manual_is_multiple_of and unfulfilled expect(dead_code)"
    status: completed
  - id: timeout-fix
    content: "Commit 3: Gateway client connect and request timeouts, streaming exempted"
    status: completed
isProject: false
---

# CI, Clippy, and gateway timeout fixes

Three commits, one per finding.

## Commit 1: CI platform-aware matrix

`.github/workflows/ci.yml` runs everything on `ubuntu-latest` but the workbench crates need GTK/GLib/WebKit dev packages. Also, the workbench voice (whisper-rs-sys) needs cmake + libclang.

**Fix:** split the CI matrix. The portable crates (everything except `promptforge-wb` and `promptforge-wb-server`) run on Ubuntu with no special deps. The workbench crates get their own job that provisions the GUI stack, or are excluded from the Ubuntu jobs.

Simplest approach: exclude the two workbench crates from the main `check` and `msrv` jobs:

```yaml
- name: Clippy
  run: cargo clippy --workspace --exclude promptforge-wb --exclude promptforge-wb-server --all-targets --all-features -- -D warnings

- name: Test
  run: cargo test --locked --workspace --exclude promptforge-wb --exclude promptforge-wb-server --all-features
```

Same for the MSRV job. The workbench gets its own job on `windows-latest` (the primary target platform, where WebView2 and CUDA are available) or on Ubuntu with provisioned packages. Windows is the natural choice since that's where it ships.

Add a `check-workbench` job on `windows-latest` that builds and tests only `promptforge-wb` and `promptforge-wb-server` (needs npm for the UI build step). Note: CUDA is not available on CI runners, so whisper-rs must build without the `cuda` feature in CI. The `cuda` feature is currently set unconditionally in root `Cargo.toml` - gate it behind a `--no-default-features` or move it to a feature flag on the server crate rather than baking it into the workspace dependency. This is the one design call in this commit.

## Commit 2: Clippy lint fixes

Two files:

1. `crates/promptforge-tool-picker/build.rs:262`: `source.len() % 4 != 0` -> `!source.len().is_multiple_of(4)` (the `manual_is_multiple_of` lint).

2. `crates/promptforge-mcp-server/src/watch/reload.rs:86-92`: the `#[expect(dead_code)]` is unfulfilled. Either the code it protects is now used (remove the expect) or the reason no longer applies (update or switch to `#[allow(dead_code, reason = "...")]` if the classifier is still legitimately test-only). Check which.

## Commit 3: Gateway client request timeouts

`GatewayClient::new` builds a `reqwest::Client` with no timeout policy at line 216-218.

**Fix:** set `connect_timeout` and `timeout` on the client builder:

```rust
let http = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(5))
    .timeout(Duration::from_secs(30))
    .build()
    .map_err(|source| GatewayError::Build(Box::new(source)))?;
```

The 30s whole-request timeout covers catalog and buffered chat. For **streaming** chat (`chat_completion_stream`), the whole-request timeout must NOT apply (a long generation legitimately takes minutes). Use a per-request override: `.timeout(Duration::ZERO)` or remove the client-level timeout and instead set per-request timeouts on the non-streaming methods only:

```rust
fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    // ... existing bearer logic ...
}

// In list_models, chat_completion (non-streaming):
self.authorize(self.http.get(...)).timeout(REQUEST_TIMEOUT).send()

// In chat_completion_stream:
self.authorize(self.http.post(...)).send()  // no timeout, stream can run long
```

The health probe already has its own `HEALTH_PROBE_TIMEOUT` at 2s - leave that as-is.

Constants: `CONNECT_TIMEOUT = 5s`, `REQUEST_TIMEOUT = 30s`. Add as module constants with doc comments. The cache API (`cache_ensure`) is a streaming SSE response like chat - no whole-request timeout on it either.

Test: existing tests already use mock gateways and should pass with the timeouts (they respond fast). Add one test that a connect to a dead port times out with `GatewayError::Transport` rather than hanging.
