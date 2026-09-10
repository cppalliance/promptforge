# Vibe Ledger

## 2026-09-10-1-unified-prompt-model

- Step 1: message record validation in the Lua protocol - `cargo test -p promptforge-lua protocol` - 42 passed, 0 failed (also `cargo test -p promptforge-agent`: 23 passed; clippy and fmt clean).
  - Decision: tool call records require string `id` and `name`, so the guide's legacy `{ id = call.id }` replay shape now errors | Falsifier: the plan's normalized `{id, name, arguments}` record is what `models.loop` appends and step 12 migrates the guide.
  - Decision: cross-record checks (unique call IDs, call-result pairing, alternation) stay out of this parse | Falsifier: step 7 explicitly owns per-dispatch pairing/uniqueness validation.
  - Decision: `ContentPart::ImageUrl` keeps only the URL string, dropping extras like `detail` | Falsifier: the Multimodal contract is data-URI image parts; no prompt or template consumes `detail`, and the type can grow if one does.
