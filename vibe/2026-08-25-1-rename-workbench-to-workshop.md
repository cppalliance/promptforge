---
name: Rename Workbench to Workshop
overview: Rename PromptForge Workbench to PromptForge Workshop across promptforge (crates, config, UI, CI, design logs) and the workspace design-wb archive. Abbreviation wb becomes ws. wg21-paperflow has no Workbench/wb product names and needs no edits.
todos:
  - id: git-mv
    content: git mv crates, example toml, design files, design-wb, cabinet output
    status: completed
  - id: rewrite-idents
    content: Longest-first workbench/wb rewrite only; never touch WebSocket ws tokens (see guardrails)
    status: completed
  - id: ws-guardrails
    content: After rewrite, pin chat_ws.rs, GET /ws, axum extract::ws, ws:// URLs, pcm-worklet
    status: completed
  - id: cargo-lock-ci
    content: Workspace Cargo.toml, crate manifests, Cargo.lock, CI job names
    status: completed
  - id: config-compat
    content: workshop.toml canonical plus workbench.toml discovery fallback
    status: completed
  - id: verify
    content: rg leftovers; cargo test the two workshop crates
    status: completed
isProject: false
---

# Rename PromptForge Workbench to Workshop

Product name: **PromptForge Workshop**. Abbreviation: **ws** (crates, binaries, CI job, TypeScript types, config file).

[`wg21-paperflow`](wg21-paperflow) only depends on `promptforge-core` / `promptforge-tool-picker`. No Workbench, `wb`, or `workbench.toml` references. No paperflow changes.

## Identity map

| Old | New |
|---|---|
| PromptForge Workbench | PromptForge Workshop |
| `promptforge-wb` crate/bin | `promptforge-ws` |
| `promptforge-wb-server` | `promptforge-ws-server` |
| `promptforge_wb_server` | `promptforge_ws_server` |
| `workbench.toml` / `workbench.example.toml` | `workshop.toml` / `workshop.example.toml` |
| `WorkbenchProvider` / `WorkbenchSocket` | `WorkshopProvider` / `WorkshopSocket` |
| `ui/src/workbench-*.ts` | `workshop-*.ts` |
| CI job `check-workbench` | `check-workshop` |
| [`promptforge/design/design-promptforge-wb-1.md`](promptforge/design/design-promptforge-wb-1.md) | `design-promptforge-ws-1.md` |
| [`promptforge/design/design-promptforge-workbench.md`](promptforge/design/design-promptforge-workbench.md) | `design-promptforge-workshop.md` |
| workspace [`design-wb/`](design-wb) | `design-ws/` |
| [`cabinet/_output/architect-promptforge-workbench.md`](cabinet/_output/architect-promptforge-workbench.md) | `architect-promptforge-workshop.md` |

Apply the same substitutions in comments, READMEs, [`promptforge/README.md`](promptforge/README.md) crate table, [`promptforge/vibe-review.md`](promptforge/vibe-review.md), `.gitignore` fixture paths, `ui/package.json` name `promptforge-wb-ui`, test helpers (`struct Workbench`, `connect_workbench`), and the crate-name unit test in [`promptforge/crates/promptforge-wb/src/main.rs`](promptforge/crates/promptforge-wb/src/main.rs).

## WebSocket guardrails (searched, do not rewrite these)

The product abbreviation becomes `ws`. The chat transport is already `ws` (WebSocket). Those are different tokens. Rewrite **only** Workbench/`wb` product names. **Never** run a global `\bws\b` replace. After the crate is named `promptforge-ws`, leftover cleanup that touches `ws` will destroy the protocol.

**Rule:** a `ws` that is not preceded by `promptforge-` / `promptforge_` as the new crate prefix is WebSocket (or a local socket variable). Leave it.

Frozen strings (must be identical after the rename):

- Route: `.route("/ws", get(chat_ws::upgrade))` in [`app.rs`](promptforge/crates/promptforge-wb-server/src/app.rs). Tests use `.uri("/ws")`. Error body: `streaming moved to GET /ws; POST /chat is buffered only`.
- Module and file: `mod chat_ws;` in [`lib.rs`](promptforge/crates/promptforge-wb-server/src/lib.rs). File stays [`chat_ws.rs`](promptforge/crates/promptforge-wb-server/src/chat_ws.rs) (chat WebSocket, not Workbench). `use crate::chat_ws;` stays.
- Axum: `use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};` in `chat_ws.rs` and [`voice.rs`](promptforge/crates/promptforge-wb-server/src/voice.rs). Handler param `ws: WebSocketUpgrade` and `ws.on_upgrade(...)`.
- Workspace Cargo: `axum = { version = "0.8", features = ["ws"] }` and the comment that axum's `ws` feature pulls tungstenite ([`promptforge/Cargo.toml`](promptforge/Cargo.toml)).
- Client URLs: `format!("ws://{addr}/ws")` in `chat_ws.rs` and [`provision.rs`](promptforge/crates/promptforge-wb-server/src/provision.rs); `format!("ws://{addr}/voice")` in `voice.rs`. Expect strings `connect to /ws`.
- Browser: [`workbench-socket.ts`](promptforge/crates/promptforge-wb-server/ui/src/workbench-socket.ts) (renamed to `workshop-socket.ts`) keeps `` `${... "wss" : "ws"}://${location.host}/ws` ``. [`voice.ts`](promptforge/crates/promptforge-wb-server/ui/src/voice.ts) keeps local `ws: WebSocket`, `let ws`, and `` `"wss" : "ws"}://${location.host}/voice` `` (voice is `/voice`, not `/ws`).
- README route table: `GET /ws` row and "streaming lives on `/ws`" in the server README.
- Design log entries 62-65, 75 in `design-promptforge-wb-1.md`: keep `GET /ws` / `/ws` as the protocol. Rewrite `WorkbenchProvider` / `workbench-socket.ts` around them.

**Also frozen (not WebSocket, same trap class):**

- AudioWorklet asset `/pcm-worklet.js`, `STATIC_FILES` in [`build.rs`](promptforge/crates/promptforge-wb-server/build.rs), route `.route("/pcm-worklet.js", ...)`, `voice.ts` `addModule("/pcm-worklet.js")`. "worklet" is not "workbench".
- Window title `"PromptForge"` in [`window.rs`](promptforge/crates/promptforge-wb/src/window.rs) unless a later pass wants "PromptForge Workshop".

**Forbidden outcomes:** no `chat_workshop.rs`, no `extract::workshop`, no route `"/workshop"`, no `ws://.../workshop`, no rename of `pcm-worklet.js`.

**Safe rewrite order:** `promptforge-wb-server` then `promptforge-wb`; `promptforge_wb_server` then `promptforge_wb`; `workbench.toml`; `WorkbenchProvider` / `WorkbenchSocket` / `workbench-`; then word `Workbench` / `workbench`. That never matches `/ws` or `chat_ws`.

Replace in longest-first order so partial hits do not corrupt names. After text rewrite, `rg` leftover `workbench`, `Workbench`, `promptforge-wb`, `promptforge_wb` under `promptforge/` and `design-ws/`.

**Pin WebSocket after rewrite (fail the job if any miss):**

- `rg -F '.route("/ws"'` and `rg -F 'mod chat_ws'` and `rg -F 'axum::extract::ws'` still hit.
- `rg -F 'ws://{addr}/ws'` and `rg -F 'location.host}/ws'` still hit.
- `rg chat_workshop` and `rg '/workshop'` (as a path) are empty in crate sources.
- `ls crates/promptforge-ws-server/src/chat_ws.rs` exists.

## Git moves then text

`git mv` directories/files first so history stays attached:

- `promptforge/crates/promptforge-wb` -> `promptforge-ws`
- `promptforge/crates/promptforge-wb-server` -> `promptforge-ws-server`
- config example, design markdown filenames, `design-wb` -> `design-ws`
- cabinet output file (move, then rewrite; no Delete tool)

Update [`promptforge/Cargo.toml`](promptforge/Cargo.toml) workspace dep path/`promptforge-ws-server`, both crate `Cargo.toml` package and `[[bin]]` names, then regenerate [`promptforge/Cargo.lock`](promptforge/Cargo.lock) with a workspace cargo command.

## Config discovery

[`discover.rs`](promptforge/crates/promptforge-wb/src/discover.rs) `CONFIG_FILE_NAME` becomes `workshop.toml`. New default template and generate-in-profile path use that name.

One compatibility line: at each search location, prefer `workshop.toml`, then still accept an existing `workbench.toml` so `~/.promptforge/workbench.toml` keeps working. README documents that the canonical name is `workshop.toml`. Server `DEFAULT_CONFIG_PATH` is `workshop.toml` (cwd only), same as today.

## Archives (user chose rewrite everywhere)

Rewrite product names inside [`design-ws/list.md`](design-wb/list.md), [`design-ws/pairs/*.md`](design-wb/pairs), and the two promptforge design logs, including quoted session text. That is a historical rewrite, not a new design entry.

## Verify

- `cargo test -p promptforge-ws -p promptforge-ws-server` (and UI typecheck if the crate build does not already run it).
- Confirm CI yaml excludes/includes the new package names.
- Run the WebSocket pin checks in the guardrails section (route, `chat_ws` module, axum `extract::ws`, `ws://` URLs, no `/workshop` route).


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Rename Workbench to Workshop (plan 99f7a67d)

## Origin and intent

The rename was a direct user directive with no preceding problem statement: "Rename PromptForge Workbench to PromptForge Workshop everywhere. "wb" becomes "ws"". The word "everywhere" carried the scope; the assistant confirmed it with an explicit question rather than assuming.

## Discarded alternatives (scope of rewrite)

The assistant offered three scopes:

1. Rewrite everywhere including design logs and archives - chosen by the user.
2. Live product plus current design docs, leaving historical pair archives as-is - discarded.
3. Live product only (crates, config, UI, CI, user-facing docs) - discarded.

The plan line "Archives (user chose rewrite everywhere)" records only the outcome; the chat shows the two narrower scopes were explicitly offered and rejected. Consequence of the chosen scope: quoted session text inside the design logs was rewritten too, which the plan itself flags as "a historical rewrite, not a new design entry."

## Why the WebSocket guardrails exist

The user ordered: "search the files and bake the websocket guardrails into the plan". The decisive point (paraphrase): the guardrails are evidence-based, built from actual searches over the repo, not a generic skip list. The core hazard (paraphrase): the new product abbreviation `ws` collides with the pre-existing WebSocket `ws` token, so once the crate is named `promptforge-ws`, any global `\bws\b` cleanup would destroy the chat protocol. `pcm-worklet.js` was caught in the same trap class: "worklet" is not "workbench".

## Blast radius reasoning (chat only, not in the plan)

- Framing (assistant, paraphrase): the rename is "wide in git, narrow in runtime". Most of the churn is archives and design logs; the runtime surface is two unpublished crates plus one config filename.
- `publish = false` on both crates means crates.io and downstream CI sit outside the radius.
- Risk split (paraphrase): the mechanical rename plus `git mv` is low risk. The real footguns are (1) accidentally rewriting WebSocket `/ws` or `pcm-worklet.js`, (2) missing a `Cargo.toml` or CI exclude, (3) breaking first-run users whose only config is still named `workbench.toml`. Footgun 3 is the entire reason the config discovery fallback exists: the plan phrase "so `~/.promptforge/workbench.toml` keeps working" is a compatibility commitment to existing installs, not a new feature.

## Deferred option

The window title `"PromptForge"` in `window.rs` was deliberately left unchanged; the plan notes a later pass could make it "PromptForge Workshop". The user never asked for it, so it stayed out of scope.

## Post-plan divergence worth recording

The plan says to git-mv and rewrite `cabinet/_output/architect-promptforge-workbench.md`. After execution the user noticed it looked like `promptforge/design/design-promptforge-workshop.md`. The chat established they are two snapshots of one design lineage (paraphrase): the cabinet file is the older 2026-08-23 consolidation, the design file is the later durable record that belongs in the repo. The user then ordered: "delete the one in _output". It was moved to `_trash/` per workspace deletion rules. Final state therefore has no cabinet output file, contrary to the plan's identity map.

## Execution facts from the chat

- Execution was triggered by a single word: "run".
- Nothing was committed until the user later said "git add commit". Result: commit `eafab46` on promptforge `master`, 128 files, not pushed at that time.
- Verification that passed: `cargo test --locked -p promptforge-ws -p promptforge-ws-server` (113 server unit tests plus shell tests).
- CI nuance (paraphrase): the Ubuntu check job excludes the workshop crates on purpose while the Windows job builds them; a local `cargo build` builds everything because the workspace has no `default-members`.
- wg21-paperflow needed zero changes because it path-depends only on `promptforge-core` and `promptforge-tool-picker`, whose names did not change.
