---
name: Crate extraction execution
overview: "Execute all nine crate extractions from the extraction menu in dependency order: gateway protocol, gateway client, workshop client facade, store, gateway-local, lua, parser, UI artifact package, and the gateway web-search service. Each extraction is one or more tested commits under the Vibe and Rust rulebooks, with HTML/CSS and TypeScript rulebooks governing the UI step."
todos:
  - id: extract-protocol
    content: Extract promptforge-gateway-protocol (wire + upstream, re-type shutdown, http_util)
    status: pending
  - id: extract-client
    content: Extract promptforge-gateway-client (core client + model, picker stays in core)
    status: pending
  - id: workshop-facade
    content: Rewire workshop client as facade over shared wire crate
    status: pending
  - id: extract-store
    content: Extract promptforge-store (store.rs + store/ together)
    status: pending
  - id: extract-local
    content: Extract promptforge-gateway-local behind additive local feature
    status: pending
  - id: extract-lua
    content: Extract promptforge-lua plus core-support crate for untrusted/cancel/observe
    status: pending
  - id: extract-parser
    content: Extract promptforge-parser
    status: pending
  - id: ui-artifact
    content: Convert UI build to versioned artifact consumption
    status: pending
  - id: extract-search-service
    content: Extract gateway web-search service behind additive feature
    status: pending
isProject: false
---

# Crate extraction execution

## 0. Context handoff
- Repository: `C:\Users\Vinnie\cursor\promptforge`. Worktree was clean at planning time (HEAD `936198b`).
- This plan executes all nine items from the decision menu at `C:\Users\Vinnie\.cursor\plans\crate_extraction_menu_c8f94316.plan.md`. That file holds the measured line counts and traced dependency edges for every item; coders must read the relevant menu item before working. The menu is the evidence base; this plan is the execution contract.
- The operator chose to implement all nine items, including item 9 (web-search service), overriding its defer recommendation.
- `promptforge-confinement` was dropped by the operator after its trace showed no shared jail exists. Do not extract it.
- Already merged and out of scope: `promptforge-tools`, `promptforge-web-search`, `promptforge-transcribe`, `promptforge-desktop-shell` (CUDA plan, commits `fb152ee`..`f1894e3`).

## 1. What this builds
Nine crate extractions in dependency order, each preserving behavior through compatibility re-exports, each with crate-root `AGENTS.md` boundary rules, each verified by the existing test suite plus `cargo public-api` snapshots. No behavior changes; this is architectural separation only.

## 2. Governing rulebooks
- `C:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md` - process, all steps.
- `C:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md` - all Rust steps.
- `C:\Users\Vinnie\cursor\tools-public\rulebooks\typescript-rulebook.md` and `C:\Users\Vinnie\cursor\tools-public\rulebooks\html-css-rulebook.md` - Step 8 (UI artifact) only.
- Root `AGENTS.md` plus nested `AGENTS.md` on the ancestor chain of every touched file; the rules manifest records them.

## 3. Execution protocol
Full-path Vibe run. Scratch: `cabinet/_scratch/vibe-crate-extractions/` with `vibe-ledger.md`, `vibe-review.md`, `rules-manifest.md`, and `api-snapshots/`.

Operator standing override to the Vibe default: each step gets exactly one Review-and-Fix invocation, which must fix every finding of every severity and leave zero open findings, or return blocked and stop the run. No second review pass. A Verify failure gets one comprehensive Coder repair and one re-Verify; still red stops the run.

From `refactor-rust.md`, without adding subcontexts: reducers-first review order (delete, narrow, dedup, reshape, fix, add); `cargo public-api` snapshots before and after each extraction step for affected library crates, with intended-surface notes from real call sites written to scratch; Review-and-Fix checks actual surface against intent; Verify fails on unplanned API drift. Ensure `cargo-public-api` is installed before Step 1; if missing, stop and report.

Pre-flight before Step 1: clean worktree required; write rules manifest; record `cargo public-api` version; snapshot the public API of `promptforge-core`, `promptforge-gateway`, and `promptforge-ws-server` into scratch as the baseline.

## 4. Numbered steps

1. **`promptforge-gateway-protocol`.** Move gateway `wire.rs` + `upstream.rs` into a new crate. Re-type `Upstream::shutdown` away from `LocalError` (own error type or associated type) so no edge points back into local. Move the six `http_util` helpers with it (both gateway and the future local crate consume them). Split the `GatewayError` seam: the crate owns transport/protocol error variants; the gateway keeps route-level variants and wraps. Gateway re-exports preserve every current path. Add crate `AGENTS.md`: OpenAI wire protocol and upstream abstraction only; no local inference, no routing, no axum handlers. Menu item 1 holds the traced edges. Verify: mandatory (first step, closes the protocol component).

2. **`promptforge-gateway-client`.** Move core `src/client/` + `src/model/` into a new crate. `promptforge_tool_picker` stays in core; the new crate exposes catalog/binding types and core adapts. Core re-exports preserve `promptforge_core::client::*` and `promptforge_core::model::*`. Own its error types instead of core's `Error`/`Result`; inject or duplicate the one `normalize` use per the coder's finding, stated in the commit. (Resolved during execution: the coder found `client/transport.rs` is the only consumer of `normalize::normalize`, so the whole `normalize` module moved with its consumer instead of inject/duplicate - a pure relocation, reversible by moving it back if a core-side consumer appears.) Add crate `AGENTS.md`: gateway model client only; never a universal client; no parser, Lua, or executor deps. Menu item 2 holds the trace. Verify: mandatory if review dirtied the tree, otherwise skip.

3. **Workshop client facade.** Rewire `ws-server/src/gateway.rs` + `protocol.rs` onto the shared wire types from Step 1 where the trace showed duplication (dual `ChatRequest`, dual `ThinkingMode`); keep Workshop UI frames (`Activity`, `Status*`, stream frames, voice constants) in ws-server. The workshop client stays an opaque relay facade; no new crate unless the coder finds the facade cleaner as one - that choice is reversible, record it in the ledger with its falsifier. Menu item 3 holds the trace and merged design. Verify: mandatory (Step 3, closes the client/protocol component).

4. **`promptforge-store`.** Move core `src/store/` **and** `store.rs` together (the `WriteScope` registry lives in `store.rs`). Promote `pub(crate)` APIs as needed; core re-exports. Add crate `AGENTS.md`: virtual filesystem only; no executor, Lua, or tool deps. Menu item 4 holds the trace (zero core imports; mechanical). Verify: skip unless review dirtied the tree.

5. **`promptforge-gateway-local`.** Move gateway `src/local/` (10,201 lines, including CUDA staging) into a new crate behind an additive `local` feature on the gateway, defaulting on. Depends on Step 1 for wire types, `Upstream`, and `http_util`. Gateway keeps `run_switch` orchestration and calls the crate's `LocalRuntime` lifecycle; the blob-cache HTTP adapter (`cache.rs`) stays in the gateway and consumes the crate's store API. Add crate `AGENTS.md`: local inference provisioning and llama-server lifecycle only; runtime never compiles native dependencies (root rule). Menu item 5 holds the trace. Verify: mandatory (closes the gateway decomposition component).

6. **`promptforge-lua`.** Move core `src/lua/` + `src/lua_models/` plus the coroutine protocol vocabulary (`protocol::Request`, `YieldParse::Request`) and the 9 binding symbols execute imports. Create `promptforge-core-support` for `untrusted` and `cancel` (both move-ready, zero core deps); `observe` moves there too if its prod surface is clean per the trace (only tests touch `Prompt`); `error` stays in core. `section_vm` stays with execute. Execute imports one-directionally from the new crate. Add crate `AGENTS.md`: Lua sandbox and host surface only; markdown-to-table host functions land here, never in the parser. Menu item 6 holds the full coupling measurement. Verify: mandatory (Step 6, closes the lua component).

7. **`promptforge-parser`.** Move core `src/parser/`. Its only lua import is `LuaProgram`; depends on Step 6. Add crate `AGENTS.md`: PromptForge prompt documents only; no general markdown utilities. Menu item 7 holds the trace. Verify: skip unless review dirtied the tree.

8. **Workshop UI artifact package.** Replace `ws-server/build.rs`'s inline esbuild pipeline with consumption of a versioned `ui/dist` artifact: a placement step (script or xtask) produces `dist/` plus a manifest (content hash, version) before rustc; `build.rs` verifies the manifest and fails with instructions when absent or stale. Preserve all five traced behaviors: non-empty compile-time embed, the `check-layers.mjs` gate, release minify, wipe-then-rebuild freshness, and the debug "edit TS then cargo build" loop (debug may keep building in place; release consumes the verified artifact). Update CI to run the UI build as its own job producing the artifact. TypeScript and HTML/CSS rulebooks govern any `ui/` edits; this step should not need to touch UI source. Menu item 8 holds the trace. Verify: mandatory (closes the component; includes UI typecheck and tests).

9. **Gateway web-search service.** Move gateway `tools.rs` + `tools/brave.rs` + `tools/web_search_process.rs` (1,493 lines) into `promptforge-web-search-service` behind an additive `web-search` gateway feature, defaulting on. The 45-ref `GatewayError` coupling resolves through the error seam established in Step 1; `check_auth` and bounded-read helpers come from the gateway via a thin mount/reload shim in `lib.rs`. Add crate `AGENTS.md`: search provider service only; credentials never in Debug/Display. Menu item 9 holds the trace. Verify: mandatory final full suite.

## 5. Commit rationale map
The Message role uses these only when the staged diff proves the step occurred; plain language, no plan or step references.

1. **Protocol:** the gateway's wire types and upstream trait are consumed by routing, local inference, and external clients; a dedicated crate lets headless and remote-only builds avoid local-inference dependencies and gives every consumer one protocol contract.
2. **Client:** executor code needs gateway communication without the parser, Lua, or executor coming along; a scoped client crate isolates HTTP and keeps the model catalog reusable.
3. **Workshop facade:** two parallel gateway clients had drift-prone duplicate wire types; one wire crate with a typed facade and an opaque relay facade removes the duplication without forcing Workshop UI frames into the protocol.
4. **Store:** the virtual filesystem has zero dependencies on the rest of core; extraction makes it independently testable and reusable by tools, Lua, and future addons.
5. **Gateway-local:** local inference is a 10k-line subsystem with heavy archive and process dependencies; extraction keeps headless gateway builds lean and confines the CUDA staging boundary.
6. **Lua:** the sandbox and host surface are core's second-largest subsystem and its heaviest dependency (`mlua`); extraction improves build parallelism and gives the planned markdown host function a home.
7. **Parser:** parse-only consumers should not link the executor; a narrow prompt-document parser crate serves MCP and CLI preview directly.
8. **UI artifact:** the Rust build should consume a verified, versioned UI artifact instead of running npm mid-build; this separates UI CI from Rust CI and produces the artifact an installer needs.
9. **Web-search service:** provider credentials and Brave HTTP are a self-contained service; extraction puts them behind a feature and a single secret-handling boundary.

## 6. Verification gates
Per-step focused tests as named. Scheduled Verify: Steps 1, 3, 5, 6, 8, and 9 (final, full suite). Every Verify runs `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and the step's focused tests; component-closing and final Verify runs add `cargo test --locked --workspace --all-features`, doctests, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`, and `cargo public-api` diff review against the step's intent notes. Step 8 adds `npm run typecheck && npm test` in `crates/promptforge-ws-server/ui`. Final Verify is the full gate set.

## 7. Data-flow review
- Order is topological per the traces: 1 before 5 (`Upstream::shutdown` re-typed, wire/http_util below both); 2 and 4 before 6; 6 before 7; 3 inside the 1+2 design; 8 and 9 independent of the chain.
- Each step receives what earlier steps produce: Step 5 consumes Step 1's crate; Step 6 consumes Steps 2 and 4; Step 7 consumes Step 6; Step 9 consumes Step 1's error seam.
- No step is open to two interpretations: each names its source files, target crate, compatibility requirement, AGENTS.md rule, and the menu item holding its trace.
- Steps 4 and 8 are parallelizable with the chain in principle, but commits stay sequential to keep history legible.
- Confidence: high - every edge was traced against the merged tree on 2026-08-29.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: crate_extraction_execution_d7fa5b21

## Where the plan came from
- The extraction menu originated in an operator-directed sweep: "spawn 4 subagents and explore @promptforge//crates and look at gateway, ws, ws-server, core, see if you can find big pieces that deserve to be in separate crates."
- Scope was checked against a future plan: "given that I plan to do this eventually @c:\Users\Vinnie\.cursor\plans\addon_dll_abi_435f28aa.plan.md what do you suggest in terms of these crate extractions?" - the extraction boundaries were chosen to serve the planned addon DLL ABI.
- The decision was staged deliberately: "start a plan for items 1 through 10. I will decide which of those I want to keep and which to defer" - then the operator decided to implement all of them: "now we are going to implement all of these crate extractions". That is why item 9's defer recommendation was overridden.
- `promptforge-confinement` was dropped on the evidence: after the trace showed no shared jail exists, the operator directed dropping it and renumbering (paraphrase of "well promptforge-confinement and edit the plan to renumber the items").

## Operator-imposed process (the why behind sections 3 and 5)
- The one-review-pass override is verbatim operator policy: "i dont want fix-forward I want fix-everything (but just one review pass)".
- Section 5 exists because the operator asked: "enrich the plan to include rationale so that the commit messages have enough information to answer 'why'".
- Per-crate AGENTS.md boundary rules were the operator's idea: "should you put AGENTS.md in the root of the crates that are being changed, that dont already have it (or append) with rules? for example tools: No HTTP client, Lua, parser, executor, or gateway dependencies".

## Naming and boundary decisions (why the crates are shaped this way)
- The client crate is gateway-scoped because the operator asked "what happens to promptforge-client when I want to add an mcp client" and then directed "rename it to promptforge-gateway-client" - hence the crate rule "never a universal client".
- Same for local: "promptforge-local is for the gateway?" -> "rename it to promptforge-gateway-local".
- Step 6's rule that markdown-to-table host functions land in the lua crate comes from a stated future feature: "eventually I am going to want to have a markdown parser function in the Lua which returns a table".
- The root rule "runtime never compiles native dependencies" (cited in Step 5) came from the local-inference work. The operator pushed "why cant we just compile llama.cpp as needed" and "I have the CUDA Toolkit. Anyone who builds promptforge-workshop is expected to have the toolkit if they want CUDA", then on seeing the prohibition asked "You put this in an AGENTS.md? "Permanent prohibition on runtime compilation" ?" and directed "add it to the root AGENTS.md".
- Steps 5 and 8 were motivated by distribution questions: "Okay but what happens when we want to build PromptForge as a binary with installer?" and "what happens when one of my team members builds on windows and they have a regular rtx 4090 instead of a blackwell".

## Deviations and decisions during execution
- Step 1: `ShutdownError` as its own type chosen over an associated type (falsifier: a later step needing `LocalError` detail through `dyn Upstream::shutdown`); variant-level `#[non_exhaustive]` on wire data bags dropped to avoid lockstep version bumps; error named `ProtocolError` with a `GatewayError::Protocol` `#[from]` wrap.
- Step 2: the `normalize` relocation recorded in the plan was itself a deviation - the plan offered inject-or-duplicate; the coder moved the whole module with its only consumer (paraphrase).
- Step 3: facade kept in ws-server, no new crate (falsifier: a second binary needing the workshop relay); the menu's shared HTTP/auth/SSE helper layer was not applied because no genuine duplication was found; `ThinkingMode` untouched - a trace correction, none existed in ws-server Rust.
- Step 5: the coder created an additional crate, `promptforge-gateway-routing`, not named in the plan, to break the `Model`/`Endpoint`/`DominionQueue` backward edge; accepted as additive and reversible.
- Step 8 coder decisions (verified by the review pass): npm script over xtask; staleness defined as sha256 over input files mirrored in Node and Rust; the artifact is always minified and debug never consumes it; the manifest lives inside `ui/dist/`; the verifier is shared via `#[path]` includes; design doc section 58 deliberately left stale.
- Step 8's design was overridden post-run by the operator. The plan's fail-with-instructions gate meant every release build after a debug build (debug wipes `ui/dist/`) needed a separate `npm run package`. Operator verdict, verbatim: "what in the fuck are you crazy? this is a huge pain in the ass! I want to build with 1 command". The release build script now auto-produces the artifact when absent or stale and fails only when it cannot; the manifest contract is unchanged. This supersedes the plan text "fails with instructions when absent or stale".
- Step 9 needed one repair round: rustdoc private-link errors from the crate-boundary visibility change; fixed and re-Verified green.
- Environmental: the operator's live gateway was squatting on 127.0.0.1:8081, which the mcp-server test fixtures hardcode as an unreachable address; the operator chose to stop the server so Verify could pass.
- History: the operator's unrelated commit `ade0f3e` landed mid-run; at their request the Step 8 review fixes were folded into the Step 8 commit and the design file into `ade0f3e` via a local history rewrite (new Step 8 hash `b33ca88`); the force-push was left to the operator.
- Post-run, the operator directed "I want to rename promptforge-ws to promptforge-workshop and rename promptforge-ws-server to promptforge-workshop-server" - packages, binaries, and directories were all renamed, so plan references to `promptforge-ws*` are historical.

## Discarded alternatives
- xtask for the UI placement step - rejected in favor of an npm script (Step 8).
- Associated type for the `Upstream::shutdown` error - rejected for a standalone `ShutdownError` (Step 1).
- Variant-level `#[non_exhaustive]` on wire data bags - dropped (Step 1).
- Shared HTTP/auth/SSE helper layer between the two gateway clients - traced but not applied (Step 3).
- Inject-or-duplicate of `normalize` - rejected for a wholesale move (Step 2).
- A new crate for the workshop facade - rejected; the facade stays in ws-server (Step 3).
- `promptforge-confinement` extraction - dropped; no shared jail exists.
- Fail-with-instructions as the release gate (the plan's original Step 8 design) - discarded after the operator's one-command override; auto-package-on-demand is the surviving design.
