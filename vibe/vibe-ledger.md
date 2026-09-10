# Vibe Ledger

## 2026-09-10-1-unified-prompt-model

- Step 1: message record validation in the Lua protocol - `cargo test -p promptforge-lua protocol` - 42 passed, 0 failed (also `cargo test -p promptforge-agent`: 23 passed; clippy and fmt clean).
  - Decision: tool call records require string `id` and `name`, so the guide's legacy `{ id = call.id }` replay shape now errors | Falsifier: the plan's normalized `{id, name, arguments}` record is what `models.loop` appends and step 12 migrates the guide.
  - Decision: cross-record checks (unique call IDs, call-result pairing, alternation) stay out of this parse | Falsifier: step 7 explicitly owns per-dispatch pairing/uniqueness validation.
  - Decision: `ContentPart::ImageUrl` keeps only the URL string, dropping extras like `detail` | Falsifier: the Multimodal contract is data-URI image parts; no prompt or template consumes `detail`, and the type can grow if one does.
- Step 2: `messages.new()` builders module - COMPONENT verify: build, `cargo fmt --check`, clippy clean; `cargo test -p promptforge-lua` - 192 passed + 2 doc-tests, 0 failed.
  - Decision: builder methods live behind the list metatable's `__index` rather than as direct fields, so serde conversion and protocol validation see only plain records | Falsifier: `lua.from_value` on builder output fails or serializes functions.
  - Decision: `install_messages` runs during `inject_host_with_var` beside the H2 models table since the shim needs no privileged captures | Falsifier: a later step requires `messages` in a VM that never injects host values.
  - Decision: used `cargo test -p promptforge-lua` instead of installing cargo-nextest | Falsifier: the project survey lists `cargo test -p <crate>` as an accepted component test command, and installing a global toolchain binary is a heavier, host-level change than the failure warrants.
