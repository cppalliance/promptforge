---
name: Coroutine Protocol Executor
overview: "Replace promptforge-core's block_in_place bridge with a coroutine yield/resume protocol: Lua host calls become request messages, a chain-stack scheduler drives section execution on one thread per run, and the multi-threaded tokio requirement evaporates. Upgrade to latest mlua (0.12+) and Lua 5.5 first, as one isolated commit; the sync API only - no async feature, no fork."
todos:
  - id: design-doc
    content: "Step 1: write design-ws/coroutine-executor-design.md capturing the protocol and scheduler design"
    status: pending
  - id: upgrade-mlua
    content: "Step 2: upgrade mlua 0.10 -> 0.12+ and lua54 -> lua55 as one isolated commit"
    status: pending
  - id: raii-teardown
    content: "Step 3: SectionContext RAII teardown with completed flag"
    status: pending
  - id: protocol-types
    content: "Step 4: Request/Answer enums with strict validation"
    status: pending
  - id: shim-layer
    content: "Step 5: Lua shim prelude installed per VM (infer, handle:infer, execute)"
    status: pending
  - id: coroutine-chunks
    content: "Step 6: section chunks run via Thread::resume; hook/pcall/jump spikes resolved"
    status: pending
  - id: scheduler-core
    content: "Step 7: Chain + driver loop + Infer/Execute dispatch; nested execute end-to-end on current-thread runtime (DECISION GATE)"
    status: pending
  - id: walk-core
    content: "Step 8: walk translation - fall-through, off-walk, reply/var roll-forward, id counter"
    status: pending
  - id: walk-jumps
    content: "Step 9: walk translation - jumps, return semantics, depth cap, observation boundaries"
    status: pending
  - id: h1-live
    content: "Step 10: H1 live pass on the scheduler (BlockRunMode)"
    status: pending
  - id: fanout-join
    content: "Step 11: fanout as N chains with join, concurrency window, ordered results"
    status: pending
  - id: fanout-semantics
    content: "Step 12: fanout write-race, fatal-arm abort, cancellation while suspended"
    status: pending
  - id: flip
    content: "Step 13: flip run() to the scheduler, delete the old engine (walkers, old fanout, bridge callers)"
    status: pending
  - id: cleanup
    content: "Step 14: delete bridge_blocking, fix doc example, current-thread callers, module docs"
    status: pending
isProject: false
---

# Coroutine-Protocol Executor for PromptForge

This plan is self-contained. It assumes no knowledge of the conversation that produced it.

**Task size (vibe-rulebook): Full.** Governing rulebooks: `tools-public/rulebooks/vibe-rulebook.md` (process), `tools-public/rulebooks/rust-rulebook.md` (all code steps). `tools-public/rulebooks/html-css-rulebook.md` and `tools-public/rulebooks/typescript-rulebook.md` are loaded but bind no step - this plan touches no web code.

**Rules manifest (vibe-rulebook rule 3):** every subagent dispatch names `promptforge/AGENTS.md`; steps touching `promptforge/crates/promptforge-ws-server/` also name `promptforge/crates/promptforge-ws-server/AGENTS.md`; steps touching `promptforge/crates/promptforge-ws/` also name `promptforge/crates/promptforge-ws/AGENTS.md`. No step touches `promptforge-ws-server/ui/` (its AGENTS.md and the TS/HTML rulebooks stay out of scope).

**Run machinery:** scratch dir `cabinet/_scratch/vibe-coroutine-executor/` holds `vibe-ledger.md` (append-only, one line per step: step, commit hash, Verify status, solo decisions with falsifiers) and `vibe-review.md` (open findings). If the promptforge worktree is dirty at run start, stop and tell the user to commit or stash first. The tool never pushes.

## Background: what this project is

PromptForge is a prompt-programming system. A prompt is a markdown document: an H1 section holds setup, H2+ sections hold units of work. Each section contains ordered blocks of two kinds: **Lua blocks** (scripting) and **prose blocks** (text sent to an LLM via a gateway, with a tool loop). `promptforge-core` (crate at `promptforge/crates/promptforge-core`) parses and executes these prompts. The executor lives in `promptforge/crates/promptforge-core/src/execute/`.

Scripts call host functions: `models.infer` (LLM call), `execute(section)` (run another section as a contained subroutine), `fanout(worker, items)` (map a worker section over a collection, concurrently), `store.*` (file I/O), plus registration/proxy calls (`tools.*`, `models.bind`, `var`, `sys`, `log`). Each section runs in a **fresh Lua VM** (isolation boundary). Lua is embedded via **mlua 0.10**, features `lua54, vendored, serialize, send` - the `async` feature is NOT enabled.

## Problem statement

The executor's Lua host callbacks are synchronous Rust functions, but inference is async (HTTP). The bridge is `bridge_blocking` in `execute/support.rs` (~line 29): `tokio::task::block_in_place` + `Handle::block_on`. Consequences:

- A **multi-threaded tokio runtime is mandatory**; the bridge detects `RuntimeFlavor::CurrentThread` and returns `Error::Internal`.
- Every in-flight Lua host call **parks a tokio worker thread** for the duration (seconds, for inference). A 20-arm fanout can occupy 20 workers.
- The doc example in `execute.rs` (~line 188) shows `new_current_thread`, which only works for prompts that never call a host function - misleading.

## Alternatives already evaluated and rejected (do not re-litigate)

- **mlua `async` feature** (`create_async_function`/`call_async`): rejected permanently, unconditionally. The async API broke in both 0.11 and 0.12; the bug family "GC vs. suspended coroutines" is live (mlua#510 open; mlua#723 fixed 2026-07 but unreleased); the pending-propagation contract (`poll_pending` sentinel) is semi-hidden; hooks are per-coroutine. The coroutine protocol is strictly better for this project: PromptForge's suspending surface is a closed set of ~8 operations (infer, execute, fanout, parallel, mcp, plus at most 1-2 future additions), and a small owned protocol beats a general-purpose mechanism we would depend on but not control. This is not a fallback option under any outcome - if the Step 7 gate fails, the fallback is the status-quo bridge (it works; it costs threads) or the thread-per-VM pattern, never mlua async. NOTE: this rejection is of the async FEATURE, not of mlua upgrades - the sync API we use is the crate's most stable surface.
- **Staying pinned to mlua 0.10**: rejected. mlua#700 - the `send` feature's borrow tracking relied on compiler internals and broke on Rust 1.93; the fix shipped in 0.12 and was not backported. We use `send`. Pinning means hoping no future rustc breaks the old implementation. Upgrade first, once, before the protocol work touches `lua/`.
- **piccolo** (pure-Rust stackless Lua): architecturally ideal but dormant (no commits since 2025-07, maintainer active elsewhere), two open correctness bugs (#144 table panics, #145 stale stack reads), no pattern-matching engine (promptforge scripts use `string.find`/`string.match` with patterns), everything `!Send`. Adopting it means owning a fork.
- **Custom Lua wrapper over lua-sys**: tractable but the unsafe invariants (longjmp-over-Rust-frames UB, stack discipline, panic boundaries) are exactly where generated code fails silently.
- **Dedicated OS thread per VM + channel bridge** (the pattern used for whisper in `promptforge-ws-server`): viable fallback, but strictly worse than the chosen design - more threads, same blocking semantics.
- **Open dispatch for the protocol, in all three evaluated forms** - handler-id registry, capability closures embedded in the message (an `Arc<dyn Fn>` carried as userdata), and two-function messages (`on_yield` + `on_complete`, the `io_context::post` model): all rejected in favor of the closed enum. Reasons: (a) structural messages (`execute`/`fanout`) cannot be handlers at all - they reconfigure the scheduler (push chains, join, enforce the depth cap) and a closure called by the driver mid-dispatch cannot do that without re-entering the driver loop or capturing `Rc<RefCell>` scheduler internals, which scatters the state machine across shim-install sites; (b) open dispatch type-erases args to `Value`, losing the compiler-checked per-message contracts; (c) the closed enum is the audit surface - "what can a script cause the host to do?" is one 20-line read, which matters in a hardened DSL; (d) at ~8 operations lifetime, the registry/closure machinery costs as much code as the enum's boilerplate while checking nothing. The two-phase structure open dispatch tries to provide already exists in named places: the Lua shim body runs at yield time, and `resume(co, answer)` is the completion - the suspended coroutine IS the continuation. If the leaf-I/O count ever grows past a dozen, the documented extension point is one additive `Generic(HandlerId, Value)` variant - do not build it now.

## Level 1: What we are building

A coroutine-protocol executor. Section Lua runs inside a coroutine (mlua `Thread`) instead of a direct `Function::call`. Host calls that need to suspend become **Lua-side shims** that `coroutine.yield` a request table; a Rust driver loop resumes the coroutine, dispatches the request (awaiting I/O without blocking any thread), and resumes with the answer. `execute`/`fanout` yield structural requests that push/pop frames on an explicit chain stack, replacing the `walk_siblings` recursion. One thread per prompt run; the multi-threaded runtime requirement disappears. Before any of that, mlua and Lua are upgraded to latest in one isolated commit.

The protocol has two message kinds, and the distinction is the design's spine. **Leaf** messages (`infer`, `mcp`) say "do this I/O and resume me with the result" - any dispatch mechanism could serve them. **Structural** messages (`execute`, `fanout`) say "change what the scheduler is running" - push a chain, fork N chains, join - so they are the scheduler's instruction set and can only be implemented by the driver itself. The OS analogy: leaf messages are `read()`, structural messages are `exec()`/`fork()`/`wait()`. Concurrency comes from interleaving chains at I/O points on one thread, not from threads - the same model as a single-threaded Boost.Asio `io_context`: parked coroutines are outstanding async operations, the pending table is the operation queue, an arriving answer posts the chain back to ready. Because one thread runs everything, the scheduler needs no locks; the store's write-race detection remains the semantic guard.

## Level 2: High-level components (dependency order)

- **A. Design doc** - everything depends on it; the design currently exists only in conversation.
- **B. Version upgrade** (mlua 0.12+, lua55) - isolated, before the protocol work, so `lua/` is not migrated twice.
- **C. RAII teardown** - independent refactor of existing `SectionContext`; the existing suite is the oracle. Placed early because it is self-contained and de-risks the scheduler's lifecycle work.
- **D. Protocol + shim layer** - the message types and the Lua-side yield wrappers; the scheduler depends on both.
- **E. Scheduler core** - chain state, coroutine chunk execution, driver loop; the decision gate.
- **F. Walk translation** - every `walk_siblings` rule as scheduler transitions; depends on E's proven frame shape.
- **G. Fanout on the scheduler** - N chains with a join; depends on F (arms run the same walk).
- **H. Flip + cleanup** - `run()` switches to the scheduler, the old engine is deleted in the same commit, then the bridge, docs, and callers follow; safe only after F+G give the scheduler full coverage.

## Level 3: Build order within components

All components build their pieces **sequentially** - each layer is the next layer's foundation, and the test suites are cumulative. The only independence: C (RAII teardown) could run in any position; it is placed before D because it is a pure refactor with an existing oracle, giving an early green commit.

## Level 4: Steps (each is one commit carrying code + tests)

Every step names its verification. Focused test command per step; the full suite runs at the flip (Step 13) and again at the end (Step 14, workspace-wide), plus the Verify schedule below.

### Step 1 - Design doc (component A)

Write `design-ws/coroutine-executor-design.md`: protocol table with exact Lua table shapes and Rust enums, the leaf-vs-structural distinction (the scheduler's instruction set), shim layer, scheduler structures (chain stack LIFO / ready queue FIFO / pending table), walk-semantics translation rules, RAII teardown, fanout/parallel() unification, local-tool-handler limitation, and the rejected-alternatives record (mlua async permanently; open dispatch in all three forms; piccolo; custom wrapper; thread-per-VM). Verification: doc review against this plan; no code.

### Step 2 - Version upgrade (component B)

Bump `mlua` in `promptforge/Cargo.toml` to 0.12+ and switch `lua54` to `lua55`. Fix every compile error (removed deprecated APIs, `Function::wrap*` signatures, `MaybeSync` userdata bounds, re-export reorganization). No behavior change intended. Verify explicitly: (a) the `send` feature compiles and links on the current toolchain (mlua#700 territory); (b) the error line-mapping parser in `lua/program.rs` still matches PUC's `[string "loc"]:N:` format under 5.5; (c) golden Lua scripts produce identical output. If lua55 breaks anything not fixable in the same commit, fall back to lua54 on 0.12 and record the decision + falsifier in the ledger. Test: `cargo test -p promptforge-core` green, unchanged suite. NOTE: no `--locked` on this step - the lockfile must update with the bump; `--locked` returns from Step 3 onward. This step is independent of Step 1 (different repos); the two may run in parallel.

### Step 3 - RAII teardown (component C)

`SectionContext` (`execute/section_context.rs`) gains a `Drop` impl with an armed/disarmed `completed` flag: the success path sets the flag so `SECTION_FINISHED` fires only on completion (existing contract), and `Drop` guarantees exactly-once teardown on every exit path. Remove the driver's explicit `teardown()` calls. Test: existing executor suite unmodified, plus a new test that an error path tears down exactly once without `SECTION_FINISHED`.

### Step 4 - Protocol types (component D)

New module `execute/protocol.rs`: `enum Request { Infer { .. }, Execute { .. }, Fanout { .. }, Mcp { .. } }` - all four variants defined now (Step 11 needs `Fanout`; `Mcp` carries its reserved fields and is never dispatched yet - an `Mcp` request received is a typed protocol error), `enum Answer` with the `(ok, result)` error envelope, and strict validation - a yield that is not a well-formed request table becomes a Lua error ("scripts may not yield directly"). Test: unit tests for well-formed parse, malformed rejection, error-envelope round-trip.

### Step 5 - Shim layer (component D)

Per-VM Lua prelude installed via `rawset` BEFORE hardening strips globals (`lua/vm.rs`, `lua/hardening.rs`): `models.infer`, `handle:infer`, and `execute` become `coroutine.yield` wrappers using the `(ok, result)` envelope (`if not ok then error(result, 0) end` - level 0 suppresses the position prefix so shim-raised errors carry no internal-chunk noise). CRITICAL COEXISTENCE CONSTRAINT: `setup_section_vm` gains a mode - `Legacy` installs the existing Rust control globals, `Scheduler` installs the yield shims. The old engine drives chunks via `Function::call` (no coroutine), so a shim's `coroutine.yield` under it errors with "attempt to yield from outside a coroutine"; installing shims unconditionally would break the legacy engine before the flip. The mode threads through `SectionVmSetup`; the scheduler's driver always selects `Scheduler`, the legacy walk selects `Legacy`, and the mode dies with the old engine at the flip. Placement: shim source lives in real `.lua` files next to the Rust (`crates/promptforge-core/src/lua/__impl_coro.lua` etc.), pulled in with `include_str!` - chunk line 1 is file line 1, so the mapping is 1:1 with no arithmetic, and editors give Lua highlighting on the shim source. Chunks are named with `set_name("@crates/promptforge-core/src/lua/__impl_coro.lua")`: PUC's `luaO_chunkid` displays `@`-prefixed names verbatim as file paths, so unexpected shim errors render as clickable `file:line:` references with no `[string "..."]` wrapper, and the line mapper (`lua/program.rs:264`) never touches them (it only rewrites the section's own `[string "{location}"]:` marker). Deliberate shim failures still raise with `error(result, 0)` (no position prefix). Compiled once via the existing `LuaProgram` machinery (a `LazyLock<LuaProgram>` or once per run, threaded like `setup.shared`) and loaded per VM. Install as a new fixed step in `setup_section_vm` (`execute/section_vm.rs:99`) after the host tables exist and before `replay_shared`, so the shared library and author scripts see the shimmed names and captured bindings still install last. Note: coroutine driving itself (`Thread::create`/`resume`) is pure Rust - the only Lua that must exist is the yield shims, because yield cannot cross the C boundary. Test: shims yield well-formed requests; error envelope raises a Lua error at the call site; a traceback through a shim shows `__impl_*` frames unmapped.

### Step 6 - Coroutine chunk execution (component E, spike resolution)

Section chunks run via `Thread::resume` instead of `Function::call` (currently `lua/vm.rs:1032`). Granularity: **one coroutine per Lua block**, created on the section's persistent VM (blocks are separate chunks; the VM and conversation roll forward across them; a coroutine that returns ends that block, scalar return semantics preserved). Resolve the four spikes with tests: (a) instruction hooks are per-coroutine in PUC Lua - the budget/cancellation hook (`lua/hardening.rs:66-87`) currently installs via `Lua::set_hook` on the main state; on mlua 0.12 check `Thread::set_hook` / thread event callbacks first, else install from the shim prelude via a privileged `debug.sethook(coroutine.running(), ...)` reference captured before hardening nils `debug`; (b) yield across `pcall` works (5.4+ semantics, re-confirm on 5.5); (c) `jump` (error-as-control-transfer) propagates through `Thread::resume` unchanged; (d) `set_name` on mlua 0.12 still passes `@`-prefixed chunk names through to `lua_load` untouched, so shim errors render as verbatim `file:line:`. Test: one focused test per spike plus chunk-return semantics.

### Step 7 - Scheduler core (component E, DECISION GATE)

New module `execute/scheduler.rs`. State placement: a `Scheduler` struct created and owned by the top-level `run()` call, living entirely in the driver loop's stack frame - no `Arc`, no `Mutex`, no sharing (one thread; the Lua shims only yield and never call into Rust for suspending operations, so the scheduler state is unreachable from Lua). Contents: a chain arena (`Vec<Chain>` + `ChainId(u32)` newtype - ids, not references, per the rust-rulebook's graph guidance), `ready: VecDeque<ChainId>`, `pending: HashMap<RequestId, ChainId>`, a join table (`HashMap<FanoutId, JoinState { remaining, results, parent }>`), the answer channel (each `infer` spawns a `spawn_local` task sending `(RequestId, Answer)` into an `mpsc`; the driver drains ready, then awaits the channel), and the gateway client (already `Clone`). `RunContext` stays the ambient shared read-mostly context, borrowed by chain steps; the scheduler is the exclusively-owned mutable counterpart (do NOT merge the two - RunContext is cloned into callbacks, the scheduler must stay unreachable from the callback layer; the borrow patterns conflict). `struct Chain` embeds the per-section frame: it owns the `SectionContext` (VM, sys, conversation, counts) and adds the chain position (section slice + index), the coroutine handle for the in-flight Lua block, and the walk-scoped slots (reply, `var`). Cancellation while suspended: the driver loop `select`s on the answer channel AND a cancellation notification; on cancel it aborts in-flight I/O tasks and errors the suspended chains with `Error::Interrupted` (same outcome as the hook-driven path while running). Inference in tests: use the existing test support (`test_support.rs`, `env_client_with_limits` fixtures) - no live gateway. The driver loop (`resume -> match request -> dispatch -> resume with answer`), `Infer` dispatched to the gateway client, `Execute` pushing/popping the chain stack with the depth cap as a per-chain execute-depth field check (stack length equals the field only within pure execute nesting; fanout arms increment depth without sitting on the execute stack, per the design doc). Gate: a prompt with nested `execute` + inference runs end-to-end on a **current-thread** tokio runtime. Pass: proceed. Fail: findings into the design doc; the fallback is the status-quo bridge or thread-per-VM, never mlua async (see rejected alternatives). Test: the gate scenario plus cancellation while suspended on `infer`.

### Step 8 - Walk translation, core rules (component F)

Port from `walk_siblings` (`execute/engine.rs:145`): fall-through order, off-walk skips (addressed targets run anyway), reply roll-forward, `var` seed/roll-forward/discard-on-execute, run-global id counter (H1 keeps 0). Test: NEW scheduler-side tests mirroring the existing walk cases. Until the flip (Step 13), the existing suite exercises the legacy engine and must stay green untouched; the scheduler proves itself with its own parallel tests.

### Step 9 - Walk translation, control transfer (component F)

Jump targets (sibling move vs child descent; parent resumes after the jumper when the child level exhausts), scalar return ends only its own chain, observation events at identical boundaries (`SECTION_FINISHED` only on completion). Test: new scheduler-side jump/return/observation tests mirroring the existing cases; existing suite stays green against the legacy engine.

### Step 10 - H1 live pass (component F)

The live H1 pass (`execute/h1.rs`) runs on the scheduler via the existing `BlockRunMode` split. Test: new scheduler-side H1 tests mirroring the existing cases; existing suite stays green against the legacy engine.

### Step 11 - Fanout, mechanics (component G)

Replace the `JoinSet` (`fanout/mod.rs:293`) with N chains interleaved by the driver: same concurrency window (`max_fanout_concurrency`), results in collection order via preallocated per-index slots, per-arm `CancelHandle`. `parallel()` is NOT built in this step - it lands later as thin sugar over this machinery. Test: new scheduler-side fanout tests mirroring the existing ordering/concurrency cases, plus a new interleaving-order test; the existing fanout suite stays green against the legacy engine until the flip.

### Step 12 - Fanout, failure semantics (component G)

Store write-write race stays a hard error, appends stay unordered-legal, fatal arm error aborts siblings, `ToolLoopExhausted` soft-degrades, empty collection errors before scheduling. Test: new scheduler-side tests mirroring the existing fanout failure cases, plus cancellation-while-suspended; the existing fanout suite stays green against the legacy engine until the flip.

### Step 13 - The flip (component H)

`run()` routes to the scheduler for everything: H1, the walk, execute chains, fanout. Delete the old engine in the same commit: `walk_siblings` and its helpers (`execute/engine.rs`), `drive_contained_chain`, the old `JoinSet` fanout driver (`fanout/mod.rs`), the `Legacy` VM-setup mode, and every remaining `bridge_blocking` call site. From this commit, the full existing suite - unmodified - runs against the scheduler. Test: `cargo test --locked -p promptforge-core`, the complete existing executor + fanout suites, green against the new engine. This is the commit the whole plan points at; if it cannot go green, stop and re-plan rather than patching forward.

### Step 14 - Cleanup (component H)

Delete `bridge_blocking` and the current-thread `Error::Internal` guard (`execute/support.rs`). Fix the misleading `new_current_thread` doc example (`execute.rs:188`) - it now just works. Move callers to current-thread runtimes ONLY where the multi-thread runtime existed solely for the bridge - expect that to be `promptforge-cli`; `promptforge-ws-server` keeps multi-thread (axum and the whisper workers need it independently). Update `execute.rs` module docs (the multi-thread contract inverts). Test: full workspace suite, `cargo test --locked --workspace --all-features`.

## Per-step behavior (vibe-rulebook contract)

Each step: create the step checklist; dispatch the **Coder** subagent (role, this plan's path, step number, `<rule-book>` block name, governing AGENTS.md paths from the manifest, `tools-public/rulebooks/rust-rulebook.md`); commit with the message format below; dispatch **Review-and-Fix** (adds `<code-review>`); amend if it dirtied the tree; run **Verify** when scheduled. Verify schedule: every 3rd step (3, 6, 9, 12), at the end of each component, when review dirtied the tree, on Step 13 (the flip: full core suite against the scheduler), and on Step 14 (full workspace suite). An unfixed Critical finding blocks the next step. No done-claim without the test command and its result line.

Commit messages: first line <= 60 chars; body 100-400 tokens with an overview of the high-level changes; zero to 3 bullets for non-obvious notes or plan deviations with what forced them; no step numbers.

## Semantics that must be preserved (the spec)

The existing test suites pin these; they must pass unmodified:

- Walk rules: fall-through order, off-walk skips (addressed targets run anyway), jump targets (sibling vs child descent, parent resumes after the jumper), scalar return ends only the chain it fires in, reply roll-forward, `var` discipline (seeded per section, rolled forward, cloned-and-discarded for `execute`).
- Run-global id counter: H1 keeps id 0; every section entry and fanout arm takes the next id.
- Observation events at identical boundaries; `SECTION_FINISHED` only on completion.
- Fanout: empty collection errors before scheduling; fatal arm aborts siblings; `ToolLoopExhausted` soft-degrades; store write-write race is a hard error; appends unordered-legal; results in collection order.
- Cancellation honored while running AND while suspended on a request.
- Error line-mapping (`lua/program.rs`) unchanged in behavior.

## Verification

- Existing executor and fanout suites are the spec: unmodified and green at every step.
- New tests per step as named above: malformed-yield rejection, error-envelope round-trip, the four spike tests, the current-thread gate scenario, cancellation while suspended, interleaving order, teardown-once-without-finish.
- Rust-rulebook gates before every commit: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`.

## Data flow check (defect pass completed, review findings folded in)

Step 1 captures the design (exists only in conversation until written). Step 2 needs nothing from Step 1 (independent upgrade; the two may run in parallel - different repos). Step 3 needs neither D nor E (pure refactor). Steps 4-5 need Step 1's protocol shapes and Step 2's final mlua version. Step 6 needs Step 5's shims. Step 7 needs Steps 4-6. Steps 8-10 need Step 7's proven frame shape. Steps 11-12 need Steps 8-9 (arms run the same walk). Step 13 (the flip) needs Steps 8-12 - full scheduler coverage - and deletes the old engine in the same commit so no dead code survives it. Step 14 is safe only after Step 13 removes the bridge's last caller. Review pass two found and fixed: the missing flip, the Legacy/Scheduler VM-mode coexistence constraint, the `--locked` self-conflict in Step 2, the Chain/SectionContext relationship, per-block coroutine granularity, the cancellation-while-suspended mechanism, the Step 4 variant ambiguity, and the ws-server runtime caveat. No step blocks on anything a previous step does not produce; no step admits two interpretations.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Coroutine Protocol Executor (coroutine_protocol_executor_56e69c5d)

Sources: creator chat 586734c2 (the design emerged mid-chat, after an unrelated text-to-speech discussion), its same-day fork cba53479, and run chat 63f90e6a. Chat 9b6df3b4 concerns a different plan (addon_dll_abi) and contributes nothing to this one.

## Origin: why this plan exists

The design began from the user's discomfort with the bridge ceremony (creator chat, 2026-08-27):

> "we don't really need to be multi-threaded. I mean, the Lua blocks all execute fast. And then when we do the inference, instead of a blocking call, it could just be a callback, like a resumption later, a continuation. And then we can just have one thread dispatching everything. ... Even the fanout, because think about it, the fanout is mostly just waiting for inference. ... There's no reason to do multi-threading."

Before choosing, the user explicitly asked to evaluate the conservative path: "stay with what we have. Build out core a little more ... Wait until mlua async stabilizes then consider a refactor." The coroutine protocol won on cost - "That's almost no cost at all. There are not that many suspension points in PromptForge" - and on scope: "there are two calls missing: mcp client, and parallel(). Not a big deal."

The concurrency model endorsement below is why the plan treats interleaving-at-I/O-points as the design's spine rather than an implementation detail:

> "'it comes from interleaving at I/O points, not from threads' HELL YEAH :) This is wonderful. It is what I do when I write highly concurrent Boost.Asio servers. This Is The Way."

Sizing decisions settled in the chat: the unit of computation is sections ("sections of course. that's the unit of computation."), and the granularity is one thread per prompt run ("so we're talking about 1 thread per prompt?", confirmed).
## Why the mlua async rejection is permanent

The plan's "rejected permanently, unconditionally" language comes directly from the user, twice:

> "plan says 'Revisit mlua async' but why would we ever want to go to that when we can just do this simple beautiful coroutine thing?"

> "I never want async mlua. the coroutine solution is so elegant, and PromptForge's needs are modest. We are only missing 2 primitives, parallel and mcp tool calls. Maybe in the future we add 1 or 2 more, but that's it. For just 8 operations which can be async (i.e. need messages in the event pump) the coroutine solution seems perfect"

Earlier the same afternoon: "this is far better than relying on some async hackery in the mlua or whatever." The mlua bug evidence in the plan (mlua#510 open, mlua#723 fixed-but-unreleased, per-coroutine hooks) came from a five-subagent research sweep of the mlua commit log and web sentiment that the user ordered that day; the piccolo findings came from a parallel five-subagent sweep he ordered immediately after.

## Why upgrade mlua and Lua first

The appetite was the user's: "I am thinking I want latest lua latest mlua, why not?" The mlua#700 pinning risk cited in the plan is the assistant's supporting evidence (paraphrase); the directive to be on latest was his. Note the rejection is of the async FEATURE, not of the crate - the user wanted latest mlua precisely while rejecting its async surface.

## Why the closed enum beat open dispatch (the user's own proposals)

All three rejected open-dispatch forms were proposed by the user, and the chat holds his actual motivation, which the plan's reason (d) answers:

> "why even use enums or strings? why not just embed a callback in the message? At that point they are general purpose, and would support infinite functions"

> "open dispatch is not because I plan to add infinite things, in theory I thought it would result in less code"

So the rejection was not driven by fear of unbounded extensibility; it was that the closure/registry machinery does not in fact save code at ~8 operations, and structural messages (execute, fanout) cannot be handlers at all because they reconfigure the scheduler. The two-function variant was also the user's: "instead of 1 function on the message there's 2. The second function is 'code to run first', when the Lua coroutine yields." His observability worry ("for observability we could still attach a display string") survives in the plan as the audit-surface rationale. He accepted the rejections with "FINE. make the plan self-contained." - which is also the origin of the plan's self-contained header and its "assumes no knowledge of the conversation" line.

## Smaller design requirements stated by the user

- Scheduler state placement was user-probed: "where will you put the state for the event loop?" followed by "shouldn't this go in RunContext?" The plan's "do NOT merge the two" passage is the resolved answer (paraphrase: RunContext is cloned into callbacks; the scheduler must stay unreachable from the callback layer).
- Shim chunk naming is a user requirement: "I want those chunks labeled with names like __impl_coro", and he asked whether chunk names could carry the real Rust source filenames with correct line numbers - the motivation behind spike (d) on @-prefixed chunk names passing through to lua_load.
- The mcp message kind and the shim prelude both originated in fork chat cba53479: "now MCP is a network I/O so in theory the MCP tool call should be a coroutine", and, on learning how shims work, "oh so we have a string of Lua code in the crate and what we pre-pend that string to the section's lua text?" (resolved in the plan as a prelude installed per VM, not textual prepending - paraphrase).
## Deviations and decisions during the run (chat 63f90e6a, 2026-08-28)

The run completed all 14 steps the same day it started. Deviations beyond what the plan text records:

- Step 1 (design doc): the depth cap was refined from a stack-length check to a per-chain execute-depth field, because fanout arms increment depth without sitting on the execute stack. The plan's own Step 7 wording was amended mid-run to match - the plan file already carries this, but the chat is why.
- Step 6: spike (a) resolved better than planned. Thread::set_hook exists on mlua 0.12, so the privileged debug.sethook fallback the plan describes was never needed.
- Step 14 held the run's one user decision. The pre-existing test fanout_store_writes_persist_across_arms rendezvoused arms by busy-polling store.glob in a Lua loop with no yielding host call - valid under thread-per-arm preemption, impossible under cooperative interleaving. The runner escalated with this framing: "this test pins an implementation detail (preemption), not any of the plan's listed preserved semantics." The user chose adapting the fixture to rendezvous through a yield (execute on a nop section inside the poll loop) over weakening the pin or re-planning for preemption; both directions were mutant-verified. This is the authoritative precedent for how "existing suites must pass unmodified" bends when a test pins preemption rather than a listed semantic.
- The runtime split landed wider than the plan predicted: cli AND dev moved to current-thread runtimes; ws-server AND mcp-server keep multi-thread. The plan had named only promptforge-cli as the expected mover and ws-server as the keeper.
- Two coder subagents aborted mid-step (Steps 10 and 13); in both cases a recovery coder assessed the partial work as coherent and completed it (Step 13's was briefly stashed, then popped and committed). Review caught two Criticals the plan could not anticipate: the root chain's client slot unseeded, so prose before any infer fell back to the env client (Step 7); and a pre-cancelled fanout returning Ok (Step 13), after which cancellation is checked at every chain-step boundary.
- The user twice interjected "you seem stuck" / "are you stuck?" during long steps; the run nonetheless finished with zero open findings and the full workspace suite green.
