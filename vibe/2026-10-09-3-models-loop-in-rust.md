---
name: Models loop in Rust
overview: First replace models.loop's and models.infer's optional leading handle with methods on the handle (h:loop, h:infer), with every loop argument check ahead of the first yield; then pin that shim's behavior with characterization tests; then move every rule of models.loop into a typed Rust state machine behind a small Lua trampoline, proven exact by those tests; then, as separate changes, fold the notice drain into the chat dispatch and resume the chat answer as an opaque Rust value.
todos:
  - id: handle-methods
    content: Phase 0 - models.loop(messages, compactor?) and models.infer(prompt) run on the section's current model, and model handles gain h:loop(messages, compactor?) and h:infer(prompt), which run on the handle's model, through two field getters over one registry table; models::is_handle backs the new is_model_handle capture; the shim splits each function into an entry per form over a shared body, checks the list before the first yield, and drops the drain guard and the guessing rule; every handle-form call site, test, and Rust doc comment migrates; the methodless-handle bullet is deleted from lua/AGENTS.md and the lua crate's Invariants list; handles_reject_colon_methods is rewritten; new argument tests in models_loop-arguments.rs, shims.rs, and errors.rs
    status: pending
  - id: loop-docs
    content: Phase 0 docs - rewrite the handle-first passages of promptforge-docs chapters 05, 10, 11, and 17 for h:loop and h:infer, together with the messages refactor's open chapter 11 and 17 follow-up, and rebuild the books, as a vibe step committed in promptforge-docs
    status: pending
  - id: characterization-tests
    content: Add the gap tests from the behavior inventory to execute/tests/models_loop_contract.rs (a flat peer registered in execute/tests.rs) and engine/src/lua/tests/models_loop_contract.rs, driven through models.loop and h:loop from Lua; they pass on the Phase 0 shim before any Phase 1 code exists
    status: pending
  - id: loop-machine
    content: Phase 1 port - models_loop.rs adapter, models_loop-machine.rs typed machine (Machine, Phase, Input, Then over the existing ChatResult, ToolCallEvent, ToolCallRecord, MessageList, and Raised), models_loop-tests.rs VM-free and adapter tests; loop_begin passed as the shim chunk's last argument in place of compactors, max_tool_iterations, and is_message_list, returning a per-call step closure that owns the call's machine; one trampoline behind both loop entries replaces run_loop, append_record, drain_task_notices, EMPTY_MODEL_REPLY, NOT_A_LIST, and compact; normalized and the parse's list text shared; quota.rs rewritten; shim, coro.rs, quota.rs, and bench comments updated
    status: pending
  - id: drain-fold
    content: Phase 2, after Phase 1 - prepare_chat drains first and pushes notices onto the list before it reads the records and commits, the empty-list refusal moves from parse_chat to after that push, and Request::DrainTaskNotices and Answer::DrainTaskNotices go with their arms and tests; MessageList::push becomes pub so the Engine can call it
    status: pending
  - id: chat-handle
    content: Phase 3, after Phase 2 - the chat answer resumes as the scheduler's own ChatResult as an opaque userdata that the step takes, and chat_result_table and the adapter's table reader go with their table-shape tests
    status: pending
  - id: verify
    content: AGENTS.md verification commands after each landed change, the facade surface check with no diff, and a clean docs build
    status: pending
isProject: false
---

# Models loop in Rust

<product-contract>

## Product Requirements

`models.loop` is a Lua function in the coroutine shim whose rules run on untyped tables that only a live Lua VM can exercise, and it guesses whether its first argument is a model handle or the message list from the Lua types of its first two arguments. Phase 0 removes the guess: a call on a model handle becomes a method of the handle, `h:loop(messages, compactor?)` and `h:infer(prompt)`, while `models.loop` and `models.infer` run on the section's current model. Characterization tests then pin that shim's behavior through `models.loop` and `h:loop`, and Phase 1 moves every rule of the loop into a typed Rust state machine behind a small Lua trampoline, proven exact by those tests passing unchanged on both sides of it. Two Engine-side improvements follow as separate changes. Every fact below was checked against `master` at `b5ea5d9b4`; paths are relative to the promptforge repository root unless they name `promptforge-docs/` or mlua's sources.

- Problem and users:
  - Engine maintainers. The loop is `models_loop` in `crates/promptforge-internal/lua/src/__impl_coro.lua` (lines 265 to 324), over the helpers `run_local_tool` (lines 145 to 156), `dispatch_tool` (161 to 166), `tools_call_as_model` (186 to 194), `append_record` (196 to 200), `drain_task_notices` (202 to 217), and `compact` (223 to 239). Its state is a handful of Lua locals, so testing any rule takes a VM, a coroutine, and canned answers.
  - Prompt authors, who see one change, in Phase 0: a call that runs on a model handle is written as a method of the handle instead of passing the handle first. Phases 1 to 3 keep everything they see exactly as Phase 0 leaves it.
  - The messages refactor (commits `d7a051f06` to `8ac23f411`, recorded in `vibe/2026-10-09-2-messages-userdata-refactor.md`) made the list a Rust `MessageList` and landed a stopgap in this shim: `append_record` calls `messages:append(record)` (line 199); `drain_task_notices` skips the drain for a value that the `is_message_list` capture rejects (line 211), so the notices wait for a round over a list; and the handle form applies when the first argument is a userdata that is not a list and the second is neither nil nor a function (lines 267 to 269, as refined by commit `b5ea5d9b4`). That rule needed two fixes on 2026-10-09, and the record view a list hands out (`messages-view.rs`, line 22) is one more userdata it has to tell apart. Phase 0 removes the rule and the drain guard; Phase 1 removes the rest of the stopgap.
- Goals:
  - every argument has one meaning: a handle's model is picked by calling the method on the handle, which is Phase 0;
  - the list has one owner, and every list write the loop makes is made in Rust;
  - the loop's text stays in Rust: Phase 1 builds every record in Rust, Phase 3 keeps reply text out of Lua, and Phase 2 keeps notice text out of Lua;
  - the loop's state is typed;
  - the rules are unit-testable without a VM;
  - arguments are checked with exact type checks, every one before the first yield;
  - the yield protocol gets smaller, which is Phase 2;
  - the whole loop's logic is in one language.
- Non-goals:
  - Porting `tools.call`, `models.infer`, `call`, `tasks`, `fanout`, or the store shims. `tools.call` keeps its Lua local-tool handshake. Phase 0 changes how `models.infer` takes its handle, and it stays in Lua.
  - Methods on tool handles. `tools.call(tool, args)` always takes the tool first, so it has nothing to guess.
  - Any change to the scheduler, the protocol parse, dispatch, or answer rendering in Phase 0 or Phase 1, apart from one helper and one text constant shared without a behavior change.
  - A Rust-side yield. mlua 0.12's only one is `pub async fn yield_with` (`mlua-0.12.0/src/state.rs`, line 2374, under `#[cfg(feature = "async")]` at line 2372), and the workspace builds mlua with `lua55`, `vendored`, `serialize`, and `send` only (`Cargo.toml`, line 120).
  - The compactor framework and everything else on the messages refactor's Deferred list (`vibe/2026-10-09-2-messages-userdata-refactor.md`, line 556).
- Success criteria:
  - After Phase 0, `models.loop` and `models.infer` run on the section's current model, `h:loop` and `h:infer` run on the handle's frozen binding, every call site, test, and doc that passed a handle first uses the method, every existing test passes after that migration, and every argument error of either loop entry is raised before the first yield.
  - Every characterization test in Testing Plan lands before the port, passes on the Phase 0 shim, and passes unchanged after Phase 1.
  - Every existing test passes unchanged after Phase 1, except `crates/promptforge-internal/engine/src/lua/tests/quota.rs`, which measures the shim's own Lua instructions and is rewritten as Testing Plan describes.
  - In Phase 1 the scheduler receives exactly the requests it receives after Phase 0, in the same order and shapes, and resumes them with the answers it renders today.
  - The trampoline holds no rule of the loop: it only yields, calls, raises, or returns as the Rust step says.
  - Every rule, including the three that no prompt can observe, has a unit test that runs without a Lua VM.
  - Every record the loop adds reaches the list through `MessageList::push` in Rust.
- Constraints:
  - The Engine stays sans-IO and deterministic: no new effect, no new suspension point, and the step is a pure function of its inputs.
  - Each change is the smallest that meets its goal. No new public facade item in any phase, and the facade surface check shows no diff.
  - The 500-line ceiling on the lua and engine crates (`crates/build-ceiling/src/lib.rs`, line 14) holds for every new and edited `.rs` file, test files included.
  - The messages refactor has landed: Phase 1 uses its `MessageList` type and its `pub(crate)` `push` (`messages.rs`, lines 101 to 115).
- Open questions:
  - None. Every decision is recorded in Decision Record.

## Functional Specification

Phase 0 is the one author-visible change: a call that runs on a model handle becomes a method of the handle, and every loop argument check runs before the first yield. Phase 1 keeps every form, append, exit rule, error, and yield exactly as Phase 0 leaves them, and turns both loop entries into one trampoline over one Rust function, `loop_begin`, and the step closure it returns. The scheduler sees the same requests until Phase 2 changes the protocol.

- Actors and workflows:
  - A prompt author calls `models.infer(prompt)`, `h:infer(prompt)`, `models.loop(messages, compactor?)`, or `h:loop(messages, compactor?)` from a section or from the H1 pass. Both get them through one install path, `setup_section_vm` (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, lines 148 and 149), called from `section_context-construct.rs` (lines 109 and 181).
  - Phase 0's migration: every `models.loop(h, ...)` becomes `h:loop(...)` and every `models.infer(h, prompt)` becomes `h:infer(prompt)`. Every prompt in `prompts/` and the Workshop chat agent already use the list-first and prompt-first forms; the call sites are engine tests, one harness test, three Workshop server tests, and the guide, all listed in Technical Design.
  - From Phase 1, both loop entries are thin Lua functions over one trampoline. Each hands its arguments to `loop_begin` behind a flag that names the entry, then the trampoline performs each action the Rust step returns: yield a request and pass the resume values back, call a handler or the compactor under the raw `pcall` and pass the outcome back, raise a value, or return nil.
  - The scheduler receives `drain_task_notices`, `chat`, `tool_call`, and `local_tool_done` in the same order and shapes through Phase 0 and Phase 1, until Phase 2 changes them. Each method sets the request's `handle` from its receiver, exactly as the handle-first call did, so every effect and event stays the same.
- Inputs and outputs:
  - The forms:
    - `models.infer(prompt)` runs one round on the section's current model, as the one-argument form does today.
    - `h:infer(prompt)` runs one round on `h`'s frozen binding, as `models.infer(h, prompt)` does today.
    - `models.loop(messages, compactor?)` runs on the section's current model, as today's list-first form does.
    - `h:loop(messages, compactor?)` runs every round on `h`'s frozen binding, as `models.loop(h, messages, compactor?)` does today.
  - `h.infer` and `h.loop` read the shim's functions wherever the shim prelude ran; in production `models.loop` is installed right after it (`section_vm.rs`, lines 148 and 149), so both methods exist exactly where `models.infer` and `models.loop` do, and read nil elsewhere, as `models.loop` does.
  - A handle keeps its fields, and `type(h)` stays `'userdata'`. Tool handles keep their current form.
  - Arguments, checked before any yield in this order, each failure raised as a `lua`-kind error table, in the Phase 0 shim and from Phase 1 in Rust with the same texts. The rest of the plan calls the error for a handle passed to `models.loop` or `models.infer` the pointed error, and the error for a method called without a model handle as its receiver the colon error:
    - `models.infer`: a model handle first raises `models.infer takes (prompt); call handle:infer(prompt) to run on a model handle`; more than one argument otherwise raises `models.infer takes (prompt)`.
    - `h:infer`: a first argument other than a model handle, which is a call written with a dot or the function taken off the handle and called bare, raises `call infer on a model handle with a colon: handle:infer(prompt)`; more than two arguments, the handle included, raise `handle:infer takes (prompt)`.
    - `models.loop`: a model handle first raises `models.loop takes (messages, compactor?); call handle:loop(messages, compactor?) to run on a model handle`; more than two arguments otherwise, trailing nils counted, raise `models.loop takes (messages, compactor?)`.
    - `h:loop`: a first argument other than a model handle raises `call loop on a model handle with a colon: handle:loop(messages, compactor?)`; more than three arguments, the handle included and trailing nils counted, raise `handle:loop takes (messages, compactor?)`.
    - Both phases check model handles with `models::is_handle`, an exact `is::<LuaModelHandle>()` test that Phase 0 adds.
    - The compactor, second for `models.loop` and third for `h:loop`: nil selects `compactors.fail`, read through an ordinary index of the `compactors` table captured at install and not checked; a function is used as given; anything else raises `compactor must be a function, got {type}`, where `{type}` is `integer` for an integer and Lua's `type()` name otherwise, so a full or a light userdata reads `userdata`.
    - The list, first for `models.loop` and second for `h:loop`: a `messages.new()` list passes; anything else raises the parse's list text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace` (`protocol/parse/chat.rs`, lines 16 and 17).
  - The return value of either loop entry is exactly one nil.
  - Records the loop adds, each a `MessageRecord` pushed onto the list in Rust from Phase 1:
    - each drained notice: `{ role = "user", content = notice }`;
    - a finished batch: one `{ role = "assistant", content = "", tool_calls = { { id, name, arguments }, ... } }`, then one `{ role = "tool", content = result, tool_call_id = id }` per call, in call order;
    - a reply: `{ role = "assistant", content = reply }`;
    - the clean exit: `{ role = "assistant", content = "" }`.
  - Requests yielded in Phase 1, built in Rust exactly as the shim builds them:
    - `{ op = "drain_task_notices" }`;
    - `{ op = "chat", messages = list, handle = handle }`, with `handle` the receiver of `h:loop` and absent for `models.loop`;
    - `{ op = "tool_call", alias = name, args = arguments, call_id = id, turn = turn }`, with the call's arguments as a table converted from the call's JSON, which the parse reads back to the same value the chat answer's table held, and the round's `turn`;
    - `{ op = "local_tool_done", ok = ok, value = value }`, with `value` set only when the handler returned.
- States and validation:
  - Rounds: at most `max_tool_iterations` rounds, the terminal one included. Each starts with the drain, then the `chat` yield. After the last allowed round's batch is appended, the loop raises `tool_loop_exhausted` instead of starting another round.
  - Emptiness: `messages must not be empty` stays the chat parse's check at every round, after that round's drain, so pending notices can fill an empty list and `h:loop(messages.new())` runs one round over them.
  - A round's answer is judged in this order: a failed answer raises its value; an overflow goes to the compactor; tool calls start a batch; a reply is appended and the loop returns; an empty reply whose `finish_reason` is `"stop"`, after at least one answered call in this loop call, appends the empty record and returns; anything else raises `empty_model_reply`.
  - The answered count includes every call of every finished batch in this loop call, a tool's own failure included, and starts at zero for each call.
  - A batch dispatches its calls one at a time, in order, and buffers each result; its records are appended only once every call has its result.
  - A local tool's call runs its handler between the depth captures, reports the outcome with `local_tool_done`, and then raises the handler's own value if the handler raised, raises the answer's error if the answer failed, and otherwise takes the answer's text as the result.
  - The compactor runs under the raw `pcall` with the overflow reason as its only argument. A compactor that returns raises the deferred-replacement error.
- Errors and recovery:
  - Every raise happens in a Lua frame as `error(value, 0)`, from Phase 1 the trampoline's, with the value the Phase 0 shim raises. So `tostring(err)` is the message, the block guard's handler stashes the same value through `stash_failure` (`coro.rs`, lines 272 to 284), and a failed answer's retained typed error is substituted as it is today (`vm/run.rs`, lines 226 to 243 and 315 to 328).
  - The values raised:
    - a failed answer's error table, unchanged;
    - a handler's own raised value, unchanged;
    - under cancellation, a failed handler's or compactor's raw value;
    - any other compactor failure through `normalize_failure`;
    - new error tables for `tool_loop_exhausted` (`tool-call loop did not converge`, no fields), `empty_model_reply` (the answer's empty detail, else `empty model reply`, with `finish_reason` when the answer has one), the deferred compactor (`the selected compactor returned without raising: replacement compactors are deferred; compactors.fail is the only shipped policy`), and the argument errors.
  - Phase 1 keeps every raised value and every raise point: the argument texts and their timing are Phase 0's.
- Security and privacy behavior:
  - No trust rule moves. The Engine wraps untrusted tool output before it answers (`scheduler/apply.rs`, lines 170 to 189), a handler's text stays trusted, and a notice's task result stays in the untrusted envelope (`scheduler/notices.rs`, lines 60 to 70).
  - `loop_begin` is reachable only as the shim chunk's upvalue, and each call's step closure only as the trampoline's local; the call's state lives inside that closure. The sandbox never loads `debug` (`hardening.rs`, lines 12 to 14), so no author code can call a step or reach a state.
  - Phase 0's `h.loop` and `h.infer` hand authors the shim's own entry functions, which reach exactly what `models.loop` and `models.infer` reach. The receiver check means the functions run only on a real model handle.
- Acceptance criteria:
  - Every Success criterion holds and every Testing Plan item passes.

</product-contract>
<implementation-contract>

## Technical Design

Phase 0 splits each of `models.infer` and `models.loop` into an entry per form over a shared body in the Lua shim, and gives model handles two field getters that return the shim's method entries. Phase 1 makes both loop entries one-line Lua functions over a trampoline that drives `loop_begin`, a Rust function from a new lua-crate module, and the step closure each call returns, which owns that call's state. The step reads Lua values into typed inputs and runs a state machine that needs no VM and pushes its records onto the list itself, then turns the machine's next step into an action that the trampoline performs, so no Rust frame is ever on a suspended stack and every Lua function that may yield is called from Lua. Phases 0 and 1 stay inside the lua crate, apart from tests, comments, and the guide; Phases 2 and 3 then change the scheduler and the protocol, one step each.

- Architecture:

  ```mermaid
  flowchart LR
    A[author block] ==>|"models.loop or h:loop"| T[trampoline]
    T ==>|values| R[Rust step]
    R ==>|action| T
    T ==>|yields request| S[scheduler]
    S ==>|resumes answer| T
    T ==>|raw pcall| C[Lua callback]
  ```

  - Phase 0's methods are Lua functions that a handle's field getters return, because each may suspend and a Rust method cannot. Each sets the request's `handle` from its receiver, so the protocol, the scheduler, and the answers stay as they are.
  - Why a trampoline: a Rust function called from Lua cannot yield without mlua's `async` feature, which this build leaves off, and a Rust function that calls a Lua function cannot let that function yield. Handlers and compactors may yield. A handler's store calls and model rounds are ordinary yields of the block (`execute/tests/local_tools.rs`, lines 173 to 193; `execute/tests/batch_turn.rs`, lines 227 to 263), and the compactor runs under the same yieldable raw `pcall` (`__impl_coro.lua`, line 230).
  - Two layers in the new module. The adapter, `models_loop.rs`, does every conversion between Lua values and the machine's types. The machine, `models_loop-machine.rs`, holds the rules and the whole call's state over typed inputs and pushes each record onto a `MessageList` itself; every Lua value it only passes along is an opaque type parameter, so its unit tests need no VM.
  - The scheduler stays as it is through Phase 1: it parses the same yields (`protocol/parse.rs`, lines 184 to 223), dispatches them through the same arms (`scheduler/dispatch.rs`, lines 124 to 206), and renders the same envelopes (`protocol/render.rs`, lines 144 to 218).
- Modules and interfaces:
  - Phase 0, `__impl_coro.lua`:
    - The first statement (lines 27 to 29) gains `is_model_handle` after `is_message_list`, and the header (lines 1 to 26) says what it reports.
    - `infer` (lines 110 to 122) splits into `run_infer(handle, prompt)`, which keeps today's yield and failure path, and two entries: `infer(...)`, still installed as `models.infer`, raises the pointed error when `is_model_handle((...))` and the arity error past one argument, then calls `run_infer(nil, prompt)`; `handle_infer(...)` checks `is_model_handle((...))` and raises the colon error when it fails, raises the arity error past two arguments, then calls `run_infer(handle, prompt)`. The comment at lines 106 to 109 says handles carry `infer` and `loop`.
    - `models_loop` (lines 265 to 324) splits the same way. `run_loop(handle, messages, compactor)` keeps the compactor default and check (lines 280 to 284), then the new list check, then the round loop (lines 287 to 323). `models_loop(...)` and `handle_loop(...)` hold their entry's handle check and arity check, in Functional Specification's order. The guessing rule (lines 267 to 269) goes, and the comment at lines 241 to 264 describes the two entries.
    - The entry errors, each raised as `raise("lua", { message = ... })` before any yield, in this order per entry: the handle check, then the arity check counting trailing nils, then for the loop entries the compactor check and the list check. The exact texts:
      - `models.infer` given a model handle first: `models.infer takes (prompt); call handle:infer(prompt) to run on a model handle`
      - `models.infer` given more than one argument: `models.infer takes (prompt)`
      - `handle_infer` whose first argument is not a model handle: `call infer on a model handle with a colon: handle:infer(prompt)`
      - `handle_infer` given more than two arguments, the handle included: `handle:infer takes (prompt)`
      - `models.loop` given a model handle first: `models.loop takes (messages, compactor?); call handle:loop(messages, compactor?) to run on a model handle`
      - `models.loop` given more than two arguments: `models.loop takes (messages, compactor?)`
      - `handle_loop` whose first argument is not a model handle: `call loop on a model handle with a colon: handle:loop(messages, compactor?)`
      - `handle_loop` given more than three arguments, the handle included: `handle:loop takes (messages, compactor?)`
    - The list check is `if not is_message_list(messages) then raise("lua", { message = NOT_A_LIST }) end`, where the chunk local `NOT_A_LIST` copies the parse's text (`protocol/parse/chat.rs`, lines 16 and 17) under a comment naming that source. The drain guard (line 211) and the comment sentences that explain it (lines 207 to 209) go, since only a list reaches the drain.
    - The return table (lines 411 to 433) gains `handle_methods = { infer = handle_infer, loop = handle_loop }`.
  - Phase 0, `coro.rs`: `install_shim_prelude` builds `is_model_handle` beside `is_message_list` (lines 186 to 188) over `models::is_handle`, passes it after `is_message_list` in the chunk call (lines 190 to 206), and stashes the chunk's `handle_methods` table under one new private key, `HANDLE_METHODS_REGISTRY`, beside `LOOP_REGISTRY` (line 62). Its doc (lines 120 to 160) names the new capture and the stash. One new function reads the stash for the handle's getters:

    ```rust
    /// The shim's `name` method for a model handle, `infer` or `loop`, from
    /// the table `install_shim_prelude` stashed; nil on a VM where the
    /// prelude never ran.
    pub(crate) fn handle_method(lua: &Lua, name: &str) -> mlua::Result<Value>;
    ```

  - Phase 0, `models-userdata.rs`: `add_fields` (lines 107 to 120) gains the two getters below, and one predicate joins `LuaModelHandle`, re-exported from `models.rs` (line 27) beside it as `messages::is_list` is for lists. The module doc (lines 1 to 7) says the handle carries `loop` and `infer` and why they are Lua functions.

    ```rust
    fields.add_field_function_get("infer", |lua, _| crate::coro::handle_method(lua, "infer"));
    fields.add_field_function_get("loop", |lua, _| crate::coro::handle_method(lua, "loop"));
    ```

    ```rust
    /// Whether `value` is a model handle.
    pub(crate) fn is_handle(value: &Value) -> bool {
        matches!(value, Value::UserData(userdata) if userdata.is::<LuaModelHandle>())
    }
    ```

  - Phase 1, the new module: `models_loop` in `crates/promptforge-internal/lua/src/`, declared in `lib.rs` beside `mod models;` (line 149). Its private child `machine` lives in `models_loop-machine.rs` and its tests in `models_loop-tests.rs`, both wired with `#[path]` under the flat-directory rule in `AGENTS.md` (line 63).
  - Phase 1, the install path: the shim chunk is `__impl_coro.lua`, embedded with `include_str!` (`coro.rs`, lines 37 and 40), compiled once (`SHIM_PROGRAM`, lines 97 to 102), and loaded and called per VM by `install_shim_prelude` (lines 161 to 270 before Phase 0), which passes its privileged captures as chunk arguments (lines 190 to 206 before Phase 0). The chunk binds them to locals in its first statement, so they are upvalues of its functions and never globals. `install_coro_shims` (`vm/install.rs`, lines 270 to 277) calls `install_shim_prelude` with the run's round cap, and in production only `setup_section_vm` calls it, right before `install_section_loop_shim` (`coro.rs`, lines 396 to 402) installs the chunk's `loop` as `models.loop`.
  - Phase 1, how the step gets in: `install_shim_prelude` calls `models_loop::loop_begin(lua, max_tool_iterations, compactors, instruction_budget)` and passes the resulting function as the chunk's last argument, in place of `compactors`, `max_tool_iterations`, and `is_message_list`. Only the loop used those three; the first two now live in `loop_begin`'s captures, and the adapter checks lists with `messages::is_list` (`messages.rs`, lines 49 and 50), so the `is_message_list` capture and its construction (`coro.rs`, lines 186 to 188) go. `is_model_handle` stays, for the two `infer` entries. `loop_begin` is never a global and never in the registry; it is reachable only as the trampoline's upvalue, exactly as `yield` is, and each step closure it returns lives only in the trampoline's local.
  - Phase 1, what the chunk keeps: `raise`, `fail`, and `engine_type`, which the `tasks` and `fanout` chunks use through `helpers` (`__impl_coro.lua`, line 416); `run_local_tool`, `dispatch_tool`, and `tools_call`, which `tools.call` uses; `tools_call_as_model`, which only the test-only `tools.call_as_model` hook uses after the port (`coro.rs`, lines 417 to 425); and Phase 0's `infer`, `handle_infer`, and `run_infer`. What it loses: `append_record`, `drain_task_notices`, `EMPTY_MODEL_REPLY`, `NOT_A_LIST`, `compact`, and Phase 0's `run_loop`; `models_loop` and `handle_loop` keep their names as one-line entries over the trampoline, so the chunk's return table and the registry stashes stay as Phase 0 leaves them.
  - Phase 1, the list: the adapter checks lists with `messages::is_list` and the machine pushes with `MessageList`'s `pub(crate)` `push`, which refuses only a system record after a non-system one (`messages.rs`, lines 101 to 115). `MessageList` is an `Arc`-backed handle whose clones share one list and whose `Default` and `records()` need no VM (`messages.rs`, lines 53 to 82), and `Request::Chat` already holds one (`protocol/request.rs`, lines 178 to 181), so the machine keeps a clone for its pushes beside the userdata its `chat` requests name, and its unit tests read the records back from a plain list. The machine builds each record as a `MessageRecord` (`protocol/request.rs`, lines 377 to 387) and pushes it directly, skipping the JSON round trip that `messages:append` makes. Every record the loop builds passes that validation by construction: its content and ids are strings, its role is never `system`, and a call's arguments are an object, which the model client guarantees (`model-client/src/normalize.rs`, lines 37 to 42).
  - Phases 0 and 1 together remove the messages refactor's whole shim stopgap: the guessing rule and the drain guard in Phase 0, and `append_record`'s `messages:append` and the `is_message_list` capture in Phase 1.
  - Phase 1, one helper and one text constant become shared, with no behavior change, so the loop's up-front checks raise the existing values by construction:
    - `error_value::normalized`, the body of `install_normalize_failure` (`error-value.rs`, lines 405 to 413);
    - `NOT_A_LIST` in `protocol/parse/chat.rs` (lines 16 and 17), made `pub(crate)` and re-exported through `protocol/parse.rs` and `protocol.rs`.
  - Phase 1, the entry checks' texts move from the Phase 0 shim into `models_loop.rs` as constants, and the model-handle check is Phase 0's `models::is_handle`.
  - Phase 1, the existing types the machine reuses, so it declares only four of its own:
    - `ChatResult` (`protocol/answer.rs`, lines 109 to 139) is the machine's chat answer. In Phase 1 the adapter reads it back from the table `chat_result_table` renders (`protocol/render.rs`, lines 64 to 96), and Phase 3 hands over the scheduler's own value.
    - `ToolCallEvent` (`promptforge_types::metrics`, `crates/promptforge-internal/types/src/metrics.rs`, lines 20 to 32), `ChatResult`'s call type, is a batch's call; the machine builds each `ToolCallRecord` (`protocol/request.rs`, lines 364 to 372) from it for the assistant record. Each `tool_call` request's `args` is made from the call's JSON with `to_value`, as the renderer made the answer's table (`protocol/render.rs`, line 82), and the parse reads it back to JSON (`protocol/parse.rs`, lines 385 to 399) and its `turn` as a `u32` (line 401), so the request parses to the same value.
    - `OverflowReason` (`compactors.rs`, lines 33 to 61) is the compactor's reason; its `tag()` is the string the compactor receives.
    - `Raised` (`error-value.rs`, lines 220 to 228), wrapped in `Error::Raised` (`error.rs`, line 164), is every new error the loop raises, rendered through `error_table` (`error-value.rs`, lines 286 to 298).
    - `MessageList`, `MessageRecord`, `MessageContent`, and `MessageRole` carry the records.
  - Phase 1, exact declarations: every new or changed declaration, checked against `master` at `b5ea5d9b4` and Phase 0's design. Bodies are elided with `;` except where the body is the spec, and declarations not listed keep their current form. `__impl_coro.lua` line numbers in this group are from before Phase 0.
  - Phase 1 declaration, `promptforge-lua`, `models_loop.rs`:

    ```rust
    //! The rules of `models.loop`, run in Rust behind the loop shim.
    //!
    //! `models.loop` and a model handle's `loop` are one-line entries over a
    //! Lua trampoline in `__impl_coro.lua` over `loop_begin`, which this
    //! module builds, and the step closure each call returns, which owns that
    //! call's machine. A Rust function called from Lua cannot yield, and
    //! cannot call a Lua function that may yield, so each step returns to the
    //! trampoline with one action, named by its tag: `"yield"` a request and
    //! pass every resume value back; call a `"handler"` between
    //! `enter_local_handler()` and `leave_local_handler()`, or the
    //! `"compactor"`, under the raw `pcall` and pass `ok` and the first
    //! result back; `"raise"` a value with `error(value, 0)`; or `"return"`
    //! nil. This adapter reads the values the trampoline passes into the
    //! machine's typed input, steps the machine, and turns its next step into
    //! that action.

    use mlua::{Function, Lua, MultiValue, Table, Value};

    use crate::error::{Error, Result};
    use crate::hardening::InstructionBudget;

    #[path = "models_loop-machine.rs"]
    mod machine;

    /// `models.loop`'s refusal of a model handle in the list's place.
    const HANDLE_FIRST: &str = "models.loop takes (messages, compactor?); \
                                call handle:loop(messages, compactor?) to run on a model handle";

    /// `models.loop`'s refusal past two arguments.
    const LOOP_ARITY: &str = "models.loop takes (messages, compactor?)";

    /// A handle's `loop` called without a model handle as its receiver.
    const NO_RECEIVER: &str =
        "call loop on a model handle with a colon: handle:loop(messages, compactor?)";

    /// A handle's `loop` past its receiver and two arguments.
    const METHOD_ARITY: &str = "handle:loop takes (messages, compactor?)";

    /// Builds `loop_begin`, the shim chunk's last argument.
    ///
    /// `loop_begin(entry, ...)` takes `false` for `models.loop` or `true` for
    /// a handle's `loop`, then that entry's arguments, and returns the
    /// call's step closure followed by the first action's tag and values;
    /// or `nil`, `"raise"`, and the argument error. The step closure, made
    /// with `create_function_mut`, owns the call's [`machine::Machine`]; it
    /// takes the last yield's resume values, or the last call's `ok` and
    /// first result, and returns the next action's tag and values. mlua
    /// refuses a recursive call of such a closure, which cannot happen,
    /// because the step returns before every yield and every call. Every
    /// error the loop raises is a `"raise"` action, so it is raised from the
    /// trampoline's Lua frame; only a crate bug or an exhausted Lua heap
    /// makes either function fail.
    ///
    /// `max_tool_iterations` is the round cap, `compactors` the table that
    /// `compactors.fail` is read from at each call, and `budget` the VM's
    /// instruction budget, whose cancel flag the step reads where the shim
    /// called `cancel_requested()`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the function cannot be created.
    pub(crate) fn loop_begin(
        lua: &Lua,
        max_tool_iterations: usize,
        compactors: Table,
        budget: &InstructionBudget,
    ) -> Result<Function>;

    /// `loop_begin`'s body: the entry flag, then that entry's argument checks
    /// in Functional Specification's order, then the machine and its step
    /// closure, whose body is `act(lua, machine.step(read_input(lua,
    /// machine.phase(), values, &budget)?))`, then the first action.
    fn begin(
        lua: &Lua,
        args: MultiValue,
        max_rounds: usize,
        compactors: &Table,
        budget: &InstructionBudget,
    ) -> mlua::Result<MultiValue>;

    /// Reads the values the trampoline passed as the input `phase` awaits.
    /// A failed envelope's value goes through [`envelope_failure`]. A `chat`
    /// answer is read back into a [`ChatResult`](crate::protocol::ChatResult)
    /// from the table `chat_result_table` renders: each call's arguments
    /// through `from_value`, the overflow tag through
    /// `OverflowReason::from_tag`, and `model` and `metrics`, which the loop
    /// never reads, left empty. A raw `pcall`'s outcome is read beside the
    /// run's cancel flag.
    fn read_input(
        lua: &Lua,
        phase: &machine::Phase<Value>,
        values: MultiValue,
        budget: &InstructionBudget,
    ) -> mlua::Result<machine::Input<Value>>;

    /// Turns the machine's next step into the trampoline's tag and values:
    /// each request table built as the shim builds it, a call's arguments
    /// converted with `to_value`, the overflow reason as its `tag()`, a
    /// normalized failure through `error_value::normalized`, and a new error
    /// as `error_table` over `Error::Raised`.
    fn act(lua: &Lua, then: machine::Then<Value>) -> mlua::Result<MultiValue>;

    /// The value `fail` raises for an envelope's failure: an error table
    /// unchanged; anything else a new `lua`-kind table whose message is what
    /// the `tostring` global returns for it. The Engine renders every failure
    /// as an error table, so only a hand-built envelope reaches the second
    /// branch.
    fn envelope_failure(lua: &Lua, value: Value) -> mlua::Result<Value>;

    /// Lua's `type()` name for `value`, with an integer named `integer`, as
    /// the shim's `engine_type` names it. mlua's `type_name` differs for a
    /// light userdata (`lightuserdata`) and an mlua error value (`error`),
    /// both of which Lua names `userdata`.
    fn lua_type_name(value: &Value) -> &'static str;

    #[cfg(test)]
    #[path = "models_loop-tests.rs"]
    mod tests;
    ```

  - Phase 1 declaration, `promptforge-lua`, `models_loop-machine.rs`:

    ```rust
    //! The rules of `models.loop` over typed inputs, with no Lua VM: the
    //! round cap, the drain before each round, the order a round's answer is
    //! judged in, the batch rule, a local tool's report and re-raise, and the
    //! compactor's rules. The machine pushes every record onto the list
    //! itself, and every Lua value it only passes along is an opaque `V`, so
    //! the unit tests run over a plain `MessageList` with any `V`.

    use promptforge_types::metrics::ToolCallEvent;

    use crate::compactors::OverflowReason;
    use crate::error_value::{ErrorField, ErrorKind, Raised};
    use crate::messages::MessageList;
    use crate::protocol::{ChatResult, MessageContent, MessageRecord, MessageRole, ToolCallRecord};

    /// The message the empty-reply rule raises when the answer names no
    /// empty detail.
    const EMPTY_MODEL_REPLY: &str = "empty model reply";

    /// One loop call: its rules' state and the values it passes along.
    pub(super) struct Machine<V> {
        /// The round cap: the run's `max_tool_iterations`.
        max_rounds: usize,
        /// The rounds started so far, the current one included.
        rounds: usize,
        /// The calls answered in this loop call, a tool's own failure
        /// included.
        answered: usize,
        /// The list every record the loop adds is pushed onto.
        list: MessageList,
        /// The same list as the value every `chat` request names.
        messages: V,
        /// The receiver of a handle's `loop`, set on every `chat` request;
        /// `None` for `models.loop`.
        handle: Option<V>,
        /// The compactor argument, or `compactors.fail` as read when the
        /// call began, which is not type-checked, as in the shim.
        compactor: V,
        /// The current batch's turn, sent back on every `tool_call` request.
        turn: u32,
        /// The current batch's calls, in the order the model listed them.
        calls: Vec<ToolCallEvent>,
        /// The current batch's results so far, one per answered call.
        results: Vec<String>,
        /// What the machine waits on.
        phase: Phase<V>,
    }

    /// What the machine waits on, which names the values the adapter reads
    /// next.
    pub(super) enum Phase<V> {
        /// The `drain_task_notices` answer: `(ok, notices)`.
        Draining,
        /// The `chat` answer: `(ok, round)`.
        Chatting,
        /// The compactor's raw `pcall`: `(ok, value)`.
        Compacting,
        /// The `tool_call` answer for the batch's next call:
        /// `(ok, result, handler, args)`.
        Calling,
        /// The handler's raw `pcall`: `(ok, value)`.
        Handling,
        /// The `local_tool_done` answer: `(ok, text)`, with the handler's own
        /// raised value when it raised.
        Reporting(Option<V>),
        /// The loop returned or raised.
        Done,
    }

    /// One typed input: the last request's answer, or the last call's
    /// outcome.
    pub(super) enum Input<V> {
        /// The drain answer: the notices in queue order, or the value to
        /// raise.
        Drained(std::result::Result<Vec<String>, V>),
        /// The `chat` answer, or the value to raise.
        Answered(std::result::Result<Box<ChatResult>, V>),
        /// A bound tool's or a task built-in's text, or the value to raise.
        Dispatched(std::result::Result<String, V>),
        /// A local tool's handler and its arguments table.
        Local { handler: V, args: V },
        /// The compactor's or the handler's raw `pcall`: its first returned
        /// or raised value, and whether the run's cancel flag was set.
        Called { outcome: std::result::Result<V, V>, cancelled: bool },
        /// The `local_tool_done` answer: the call's text, or the value to
        /// raise.
        Reported(std::result::Result<String, V>),
    }

    /// What runs next, after the records the step pushed.
    pub(super) enum Then<V> {
        /// Yield `{ op = "drain_task_notices" }`.
        Drain,
        /// Yield `{ op = "chat", messages = messages, handle = handle }`.
        Chat { messages: V, handle: Option<V> },
        /// Yield `{ op = "tool_call", alias = call.name, args = call.arguments,
        /// call_id = call.id, turn = turn }`.
        ToolCall { call: ToolCallEvent, turn: u32 },
        /// Call the compactor with the reason's tag, or nil when there is
        /// none.
        Compact { compactor: V, reason: Option<OverflowReason> },
        /// Call the handler with its arguments table.
        Handle { handler: V, args: V },
        /// Yield `{ op = "local_tool_done", ok = true, value = value }` when
        /// the handler returned, or `{ op = "local_tool_done", ok = false }`
        /// when it raised.
        Report(Option<V>),
        /// Return nil.
        Return,
        /// Raise this value unchanged.
        Raise(V),
        /// Raise a compactor's failure through `normalize_failure`.
        RaiseNormalized(V),
        /// Raise a new error table.
        RaiseNew(Raised),
    }

    impl<V: Clone> Machine<V> {
        /// A loop over `list`, capped at `max_rounds` rounds, and its first
        /// step: the first round's drain, or `tool_loop_exhausted` when the
        /// cap is 0.
        pub(super) fn begin(
            max_rounds: usize,
            list: MessageList,
            messages: V,
            handle: Option<V>,
            compactor: V,
        ) -> (Self, Then<V>);

        /// What the machine waits on.
        pub(super) fn phase(&self) -> &Phase<V>;

        /// Applies one input, pushes the records it settles, and returns what
        /// runs next. A reply is read as the renderer reads it, an empty
        /// string as no reply. An input other than the one
        /// [`Machine::phase`] names is a crate bug and raises an `internal`
        /// error.
        pub(super) fn step(&mut self, input: Input<V>) -> Then<V>;
    }
    ```

  - Phase 1 declaration, the trampoline in `__impl_coro.lua`. The chunk's first statement (lines 27 to 29) becomes the first block below. Phase 0's `run_loop`, `NOT_A_LIST`, and the bodies of `models_loop` and `handle_loop`, with `append_record`, `drain_task_notices`, `EMPTY_MODEL_REPLY`, and `compact` and their comments, are replaced by the second.

    ```lua
    local yield, var_snapshot, models, tools, error_value, stash_failure,
      normalize_failure, enter_local_handler, leave_local_handler,
      cancel_requested, is_model_handle, loop_begin = ...
    ```

    ```lua
    local function drive(step, action, a, b)
      while true do
        if action == "yield" then
          action, a, b = step(yield(a))
        elseif action == "handler" then
          enter_local_handler()
          local ok, value = raw_pcall(a, b)
          leave_local_handler()
          action, a, b = step(ok, value)
        elseif action == "compactor" then
          local ok, failure = raw_pcall(a, b)
          action, a, b = step(ok, failure)
        elseif action == "raise" then
          error(a, 0)
        else
          return nil
        end
      end
    end

    local function models_loop(...)
      return drive(loop_begin(false, ...))
    end

    local function handle_loop(...)
      return drive(loop_begin(true, ...))
    end
    ```

    `drive`'s comment says that the loop's rules run in Rust behind `loop_begin` and the step closure it returns, that this function only performs the action each step returns, and why: a Rust function cannot yield or call a Lua function that may yield. The entries' comment says each names itself to `loop_begin`, so the checks know the form without inspecting the arguments. Both entries return through a tail call, which only drops their own frame from a traceback; every raise is at level 0, and the guard records the author's frames either way. The chunk header (lines 1 to 26) drops `compactors`, `max_tool_iterations`, and `is_message_list` from its list of captures and adds `loop_begin`. `tools_call_as_model`'s comment (lines 177 to 185) says that only the test-only `tools.call_as_model` hook calls it now.
  - Phase 1 declaration, `error-value.rs`, beside `install_normalize_failure`, which becomes `lua.create_function(normalized)`:

    ```rust
    /// What `normalize_failure` returns for `value`: a Rust callback's
    /// failure as the error table its classification names, anything else
    /// unchanged. The capture and the loop's compactor rule share it.
    pub(crate) fn normalized(lua: &Lua, value: Value) -> mlua::Result<Value>;
    ```

  - Phase 1 declaration, `protocol/parse/chat.rs` (lines 16 and 17), with `NOT_A_LIST` re-exported through `protocol/parse.rs` and from `protocol.rs` as `pub(crate)`:

    ```rust
    /// The refusal for a `messages` value that is not a `messages.new()`
    /// list. The chat parse and the loop's argument check share it.
    pub(crate) const NOT_A_LIST: &str = "models.loop needs a messages.new() list; build one with \
                                         messages.new() and :user, :append, or :replace";
    ```

  - Phase 1 declaration, `coro.rs`, `install_shim_prelude` (lines 176 to 206 before Phase 0): the `compactors` read at line 176 stays, the `is_message_list` capture (lines 186 to 188) goes, and the chunk call becomes the block below. The doc of `install_shim_prelude` (lines 120 to 160 before Phase 0) says the round cap and the `compactors` table now go to `loop_begin`, and the doc of `MODEL_TOOL_CALL_REGISTRY` (lines 64 to 70) drops "in production the loop shim reaches the function directly".

    ```rust
        let loop_begin = crate::models_loop::loop_begin(
            lua,
            max_tool_iterations,
            compactors,
            instruction_budget,
        )?;
        let program = SHIM_PROGRAM.as_ref().map_err(Error::shared)?;
        let shims: Table = program
            .load(lua)?
            .call((
                yield_fn.clone(),
                var_snapshot.clone(),
                models,
                tools,
                error_value,
                stash_failure,
                normalize_failure,
                enter_local_handler,
                leave_local_handler,
                cancel_requested,
                is_model_handle,
                loop_begin,
            ))
            .map_err(Error::lua)?;
    ```

  - Phase 1, choices behind these declarations:
    - The module is `models_loop`, after the function it implements, beside `models`.
    - Action tags are short Lua strings, written only in `act` and the trampoline, so the trampoline reads as prose and shares no enum or numbering with Rust.
    - The step is a closure from `create_function_mut` that owns the call's `Machine`, which holds every value the call passes along, so Lua never holds or passes a state handle, the adapter needs no state type of its own, and the chunk gains one argument instead of two. Under mlua's `send` feature the closure must be `Send`; `Value`, `AnyUserData`, `Table`, and `Function` are `Send` under that feature, and `MessageList` and `InstructionBudget` are `Arc`-backed.
    - The two loop entries pass a boolean to one `loop_begin` instead of calling two Rust functions, so the chunk gains one capture, and the checks know which entry ran without inspecting the arguments.
    - The machine requires `V: Clone`, because every `chat` request names the same list and handle values; an mlua `Value` clone is a reference clone.
    - Calls are typed: each `ToolCallEvent` holds the JSON arguments the adapter read once from the `chat` answer, and each `tool_call` request's `args` is a fresh table converted from that JSON, which the parse reads back to the same JSON. A local handler gets a fresh table made from the request's JSON either way (`protocol/render.rs`, lines 184 to 189), so table identity is unobservable.
    - The adapter matches the machine's `Phase` to read the next input, so no second enum names what the machine awaits. The batch's turn, calls, and results are fields of the machine, filled per batch and cleared after it, so there is no batch type. The machine pushes its records itself, so `step` returns only what runs next.
    - `Input::Called` serves both raw `pcall` outcomes, which the phase tells apart, and the three raise kinds are variants of `Then`, so neither needs a type of its own.
    - `Input::Answered` boxes the `ChatResult`, the one large payload, so the enum stays small under clippy's `large_enum_variant` lint.
    - "The loop shim" keeps naming `models.loop`'s implementation, now the trampoline and its step, so the many comments that say what the loop shim yields stay true; only comments that say a rule runs in Lua change.
  - Phase 2, the notice drain folded into the chat dispatch:
    - `prepare_chat` (`execute/scheduler/chat.rs`, lines 104 to 184) starts by taking `self.drain_task_notices(id)` (`scheduler/notices.rs`, line 85), which also joins each task (line 90). When that returns notices, it pushes one user record per notice onto the list with `MessageList::push`, before it reads `list.records()` (line 110) and so before the list's commit ahead of the issue (lines 172 to 182), which records the wire request the Chat effect's `after` and `keep` refer to. The rest of `prepare_chat` stays, because it already reads the records from the list handle.
    - `MessageList::push` (`crates/promptforge-internal/lua/src/messages.rs`, line 103) is `pub(crate)`, and `prepare_chat` is in the Engine crate, so Phase 2 makes `push` `pub` and adds `messages.rs` to its edited files. The Engine already imports `MessageList` through `engine/src/lua.rs` (line 18), and `MessageList` is absent from `crates/promptforge/public-api.txt`, so the facade surface stays the same.
    - The empty-list refusal moves from `parse_chat` to right after that push, with the same text and no event, so `h:loop(messages.new())` still runs when notices are pending, as it does today.
    - The machine drops `Phase::Draining`, `Then::Drain`, and `Input::Drained`; a round starts with `Then::Chat`, and the trampoline stays as it is.
    - Deleted: `Request::DrainTaskNotices` (`protocol/request.rs`, lines 136 to 142); `Answer::DrainTaskNotices` (`protocol/answer.rs`, lines 238 to 241) and its `map_error` arm (line 272); the parse arm (`protocol/parse.rs`, line 219); the render arms (`protocol/render.rs`, lines 171 to 175 and line 203); `blocked_on`'s arm and the dispatch arm (`scheduler/dispatch.rs`, line 97 and lines 187 to 190); `dispatch_drain_task_notices` (`scheduler/notices.rs`, lines 96 to 101).
    - Tests: the two drain tests in `protocol/tests/answer.rs` (lines 283 to 326) and the one in `protocol/tests/parse_tasks.rs` (lines 156 to 165) go; the drain steps of `lua/tests/shims.rs` (lines 262 to 267 and 305 to 308), of `quota.rs` (lines 106 to 147), and of the yield-level characterization test go.
    - Docs: `scheduler/notices.rs` (lines 14 to 18), `scheduler/dispatch.rs` (lines 8 to 10), and the docs of `parse_chat` and `Request::Chat`.
    - What changes for a run: the `chat` of the chain that owns the model tasks is issued in the chain step that finished its batch, rather than after one more trip through the ready queue (`answer_inline`, `scheduler.rs`, lines 337 to 341). Notices land in the same rounds as today, because the queue is read at the same point of that chain's own sequence, so the guide's rule that a notice joins the first round that gathers notices after its task ends (`promptforge-docs/src/language/15-tasks.md`, line 992) stays true. Across chains, the order effects are issued in, and so round ids, effect ids, and event order, changes whenever another chain is ready when the owning chain's batch ends. Chains that move in step keep their order, as the three looping fanout arms of `execute/tests/fanout_acceptance.rs` (lines 351 to 410) do.
    - Cost and risk: about twelve files with a net deletion, plus a review of every multi-chain test whose scripted replies are handed out in issue order; the reordering is the risk its decision weighs. Public API: none.
  - Phase 3, the chat answer as an opaque handle. It lands after Phase 2, because both edit `read_input`:
    - `ChatResult` itself becomes the userdata: `impl mlua::UserData for ChatResult {}`, with no methods, beside its declaration in `protocol/answer.rs`, so no wrapper type is needed. It is absent from `crates/promptforge/public-api.txt`, so the facade stays the same. The `Answer::Chat(Ok(result))` arm of `into_envelope` (`protocol/render.rs`, line 190) resumes `(true, result)` as that userdata, and `chat_result_table` (lines 49 to 96) is deleted.
    - `read_input` takes the result out with `AnyUserData::take::<ChatResult>()` and hands the machine the scheduler's own value, so the adapter's table reader goes. The machine stays as it is: it already reads an empty reply as no reply, the renderer's rule (line 70), and builds each `tool_call` request's table from the call's JSON.
    - Tests: the eight table-shape tests of `protocol/tests/answer_chat.rs` (lines 8 to 300 and 321 to 395) give way to one test that a chat answer resumes as a `ChatResult` userdata the step takes, beside the machine's typed tests; the error test (lines 301 to 320) stays, as does the adapter's table-reader test until the reader goes. `shims.rs` and `quota.rs` render their answers through `into_envelope`, so they keep working.
    - Docs: `ChatResult`'s doc (`protocol/answer.rs`, lines 92 to 107).
    - Public API: none. Risk: low, because the chat answer is never author-visible.
    - Its limit: a model-issued bound tool's text arrives through the `tool_call` answer, rendered as a Lua string (`scheduler/apply.rs`, lines 170 to 189; `protocol/render.rs`, lines 147 to 151), and the `tools.call_as_model` test hook and the Lua `dispatch_tool` read that same answer. Keeping that text out of Lua needs the hook retired or rebuilt over the step, so it is Deferred.
- File and public API changes:
  - Edited in Phase 0: in `crates/promptforge-internal/lua/src/`, `__impl_coro.lua`, `coro.rs`, `models.rs`, and `models-userdata.rs`; the comments that say invocation is namespace-only or show a handle-first call, which are `messages.rs` lines 3 to 6, `vm/install.rs` lines 54 to 56, `protocol.rs` line 4 in both the lua crate and `engine/src/execute/protocol.rs`, `protocol/request.rs` line 27, `protocol/parse.rs` lines 243 to 244 and 257, `tools/userdata.rs` line 8 (tool handles stay methodless, so only the cross-reference changes), `engine/src/execute/tools.rs` line 4, and `engine/src/execute/config.rs` line 186; also `protocol/parse/chat.rs` lines 31 to 40 (the `parse_chat` doc's leading-argument wording), `engine/src/execute/tests/model_and_reply.rs` line 3, and the module doc of `engine/src/lua/tests/shims.rs` at lines 4 and 5.
  - `AGENTS.md` files may be stale. Where a line in any `AGENTS.md` conflicts with this plan, the plan wins and the step that meets the conflict deletes that line, preferring deletion to rewording; a conflict whose resolution is unclear returns blocked for the owner.
  - Deleted in Phase 0, as stale rules the methods contradict: the methodless-handle bullet of `crates/promptforge-internal/lua/AGENTS.md` (line 5), and the same rule's bullet in the `## Invariants` list of `crates/promptforge-internal/lua/src/lib.rs` (lines 51 to 56). Each bullet is deleted whole, not reworded. The Invariants list keeps its other bullets and the marker that `cargo test -p build-xtask` requires, and `crates/workshop/ui/test/docs-claims.mjs` checks only the defined terms, so neither gate is affected.
  - Rewritten in Phase 0: `handles_reject_colon_methods` in `engine/src/lua/tests/shims.rs` (lines 399 to 418), which asserts that reading `infer` off a model handle fails. It becomes a test that `type(h.infer)` and `type(h.loop)` are `"function"` on a model handle and that reading a field the handle does not have still fails.
  - Call sites Phase 0 migrates to `h:loop`: `crates/harness/tests/suite/host.rs` line 55; `crates/workshop/server/tests/it/agents.rs` line 54 and `agents/revoke.rs` lines 29 and 33; `execute/tests/models_loop.rs` lines 347 and 349; `execute/tests/models_loop-arguments.rs` lines 27, 29, 70, and 111; `execute/tests/models_loop_compactors.rs` line 118, a call of the `refusal(...)` helper (defined at lines 109 to 114), which passes its arguments to `pcall(models.loop, ...)`.
  - Call sites Phase 0 migrates to `h:infer`: `crates/harness/tests/suite/host.rs` line 43, `pcall(models.infer, writer, ...)`, which becomes `pcall(writer.infer, writer, ...)`; `engine/src/lua/tests/shims.rs` lines 368 and 387; `engine/src/lua/tests/errors.rs` line 174, whose assertion at line 184 then expects `handle:infer takes (prompt)`, and the comment at line 24; `errors.rs` lines 29 and 48, both `pcall(models.infer, models.get("fast"), "yo", { temperature = 0 })`: the first becomes `pcall(models.infer, "yo", { temperature = 0 })` and its assertion at line 31 expects `models.infer takes (prompt)`, with the test renamed to match, and the second binds `local h = models.get("fast")`, calls `pcall(h.infer, h, "yo", { temperature = 0 })`, and its assertion at line 54 expects `table|lua|handle:infer takes (prompt)`; `execute/tests/debug_and_counts.rs` lines 65 and 307 and the comment at 299; `debug_and_counts-infer-rounds.rs` line 93; `live_infer.rs` lines 13, 39, 253, 307, and 357; `scheduler/live_h1/pass.rs` line 18; `exec_flow.rs` lines 178, 222, and 243 and the comments at 169 and 211; `model_and_reply-handles.rs` lines 127 and 224.
  - Every prompt in `prompts/` and the Workshop chat agent (`crates/workshop/agents/agents/chat.md`, line 38) already use the list-first and prompt-first forms.
  - Phase 0's docs, in `promptforge-docs`: `05-lua-environment.md` line 208, which spells out the optional-leading-handle API and its `handle?` error text; `10-models.md` lines 232 to 297, 409, 416, and 429, the last an error bullet with the `handle?` text; `11-conversations.md` lines 221, 250, 255, 416, and 564, the last the `handle?` loop error text; `17-quick-reference.md` line 69, which says a model handle has no methods, and lines 74 to 76. They land together with the docs follow-up the messages refactor left open (`vibe/2026-10-09-2-messages-userdata-refactor.md`, lines 21 and 779), which also covers chapter 11 line 129 and the list's `pairs` behavior. Line 255 has said "a userdata first argument is taken as the handle" since before lists became userdata.
  - New in the characterization work item: `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`, a flat peer registered in `execute/tests.rs` beside `mod models_loop;` and `mod models_loop_compactors;` (lines 395 and 396) as `models_loop_compactors.rs` is, and `crates/promptforge-internal/engine/src/lua/tests/models_loop_contract.rs`, registered in `engine/src/lua/tests.rs` (lines 15 to 19). It is a flat peer rather than a third `models_loop-*` sibling of `execute/tests/models_loop.rs`, which already has two (lines 20 to 24), because a third would force the group into a `models_loop/` directory (`AGENTS.md`, line 63).
  - New in Phase 1: `crates/promptforge-internal/lua/src/models_loop.rs`, `models_loop-machine.rs`, and `models_loop-tests.rs`.
  - Edited in Phase 1: in `crates/promptforge-internal/lua/src/`, `lib.rs`, `coro.rs`, `__impl_coro.lua`, `error-value.rs`, `protocol.rs`, `protocol/parse.rs`, and `protocol/parse/chat.rs`; `crates/promptforge-internal/engine/src/lua/tests/quota.rs`; the doc comment of `crates/promptforge-internal/engine/benches/models_loop.rs` (lines 1 to 6). The guide is already current after Phase 0's docs change, since Phase 1 is invisible to authors.
  - Public: none in any phase. Model handles are crate-private (`LuaModelHandle` is `pub(crate)`), so Phase 0 adds no facade item; no phase changes `crates/promptforge/public-api.txt`, any `Effect`, effect record, or answer record.
- Data, persistence, failure, security, and privacy constraints:
  - Persisted shapes stay the same. Phase 0, Phase 1, and Phase 3 issue the same effects with the same records; Phase 2 can renumber rounds and effects across chains, as its decision says.
  - The 500-line ceiling counts every `.rs` file under `src`, `tests`, `benches`, and `examples`. Estimates: after Phase 0, `coro.rs` from 459 to about 471 lines, `models-userdata.rs` from 121 to about 130, and `__impl_coro.lua` from 433 to about 465; after Phase 1, `models_loop.rs` about 300, `models_loop-machine.rs` about 260, `models_loop-tests.rs` about 400, `coro.rs` about 470, `error-value.rs` from 462 to about 470, `protocol/parse.rs` from 428 to about 429, and `__impl_coro.lua` about 360. If `models_loop-tests.rs` would pass 450 lines, the machine's VM-free tests move to a file of their own, and the three-file `models_loop-*` group becomes a `models_loop/` directory under the flat-directory rule. The new engine test files stay under 400 lines each.
  - The loop's buffered results and its records now live outside the Lua heap limit (`lib.rs`, line 99), as the list's records already do.
  - An adapter failure, which only a crate bug, a failed conversion, or an exhausted Lua heap can cause, returns `Err` and reaches Lua as an mlua callback error. An input of the wrong kind inside the machine is also a crate bug, but the machine has no VM and returns `Then::RaiseNew` with an `internal`-kind `Raised`, so the trampoline raises it as an error table.
  - Exactness of the port, helper by helper. `__impl_coro.lua` line numbers here are from before Phase 0, which moves these helpers and keeps their bodies as they are:
    - Phase 0's entry checks: `begin` runs `models_loop`'s or `handle_loop`'s checks, as the entry flag names, in the same order with the same texts. `is_model_handle` and `is_message_list` are `is::<LuaModelHandle>()` and `messages::is_list` on both sides, and `select('#', ...)` and the length of the `MultiValue` mlua passes both count trailing nils.
    - `raise(kind, fields)` (`__impl_coro.lua`, lines 54 to 56) is `error(error_value(kind, fields), 0)`, and `error_value` finishes the fields table with `kind`, a `message`, and the shared metatable (`error-value.rs`, lines 271 to 280 and 307 to 320). `Then::RaiseNew(Raised { kind, message, fields })` becomes `error_table` over `Error::Raised` (lines 286 to 298), which displays as its message and goes through the same `finish_table`, and the trampoline raises it with `error(a, 0)`; `empty_model_reply`'s `finish_reason` is an `ErrorField::String` in `fields`, set only when the answer has one. The shim always passes a `message`, so the kind-tag fallback applies on neither side. Key order inside the table may differ, which no author can observe: the sandbox's `pairs` and `next` walk keys in sorted order (`iteration.rs`, lines 1 to 8), and `raised_from` reads the fields into a `BTreeMap` (`error-value.rs`, lines 442 to 456).
    - `fail(result)` (lines 61 to 64) raises an error table unchanged and wraps anything else in a `lua`-kind table of `tostring(result)`. `envelope_failure` gives the same value, calling the `tostring` global just as `fail` reads it. Every failure envelope the Engine renders holds an error table (`protocol/render.rs`, lines 192 to 212), so production only ever takes the first branch.
    - `normalize_failure` (`error-value.rs`, lines 405 to 413): `Then::RaiseNormalized` runs the same body through `normalized`, so a Rust failure such as `compactors.fail`'s typed exhaustion (`compactors.rs`, lines 256 to 264) becomes the same table, and anything else passes through unchanged.
    - `cancel_requested()` (`coro.rs`, lines 182 to 185) is `budget.is_cancelled()`. The adapter reads the same flag when it reads a handler's or a compactor's outcome, and the machine uses it only where the shim did: a handler that failed under cancellation has its raw value raised with no `local_tool_done` yield (`__impl_coro.lua`, line 149), and a compactor that failed under cancellation has its raw value raised, checked after the returned case (lines 231 to 237). The flag is read a few instructions after the shim read it, with no Lua code in between.
    - `run_local_tool` (lines 145 to 156), in order: `enter_local_handler()`, `raw_pcall(handler, args)` keeping only the first result, and `leave_local_handler()` run in the trampoline in the shim's order with the same captured functions; the cancel check is the machine's; the `local_tool_done` table holds `ok` and, only when the handler returned, `value`, as `done` does (lines 150 and 151); after the answer, the handler's own raised value is raised first, then a failed answer's value, and otherwise the text is the call's result (lines 153 to 155), which is the rule `Phase::Reporting` applies.
    - `dispatch_tool` (lines 161 to 166): a failed answer is raised through `fail` before the handler slot is looked at, a third resume value other than nil is a local handler, and otherwise the second value is the text. `read_input` reads the four values in that order.
    - `compact` (lines 229 to 239): if the compactor returned, the deferred error; else, under cancellation, the raw value; else the normalized value. The trampoline's `raw_pcall(a, b)` passes exactly one argument, the reason as the answer gave it.
    - The block guard and retained errors: every raise is `error(value, 0)` in a Lua frame with the shim's value, so the guard's handler stashes it at the raise point with the author's frames still on the stack (`coro.rs`, lines 272 to 284), `block_failure` classifies it as today (`vm/run.rs`, lines 283 to 293), and a failed answer's typed error is substituted when its table surfaces (lines 226 to 243 and 315 to 328). A Rust callback error would reach Lua as an opaque mlua error value instead, which is why `loop_begin` and the step closure return every loop error as a `"raise"` action rather than `Err`.
    - Every Rust frame returns before a suspension: `loop_begin` and the step closure always return before the trampoline yields or calls a handler or the compactor.
    - Globals: the Phase 0 shim reads `error`, `type`, `select`, `ipairs`, and `tostring` as globals at call time. The trampoline still reads `error` that way; the Rust checks read their types directly; and `tostring` is read only on the hand-built-envelope branch.

</implementation-contract>
<verification-contract>

## Testing Plan

Phase 0 migrates every handle-first call and adds tests for the new forms and their errors. The characterization work item then pins, through `models.loop` and `h:loop` from Lua, every behavior of the Phase 0 shim that tests leave open; those tests pass unchanged after Phase 1, which is the proof that the port is exact. Phase 1 adds the machine's VM-free unit tests and the adapter's tests; Phase 0's tests already pin every argument rule. Phases 2 and 3 each update only the tests their deletions touch.

- Unit:
  - `models_loop-tests.rs`, machine tests with a plain test type for `V`, a `MessageList::default()` read back through `records()`, and no VM: every `Phase` transition and every `Then`; the records each step pushes; the order a round's answer is judged in; the answered count across batches and its reset per call; the cap raised after the last allowed batch and before another drain; the empty-detail fallback; a local call's three outcomes and the re-raise order; the cancel branches for handlers and compactors, including a returning compactor winning over cancellation; an input of the wrong kind raising `internal`. Three of these rules are invisible to a prompt, because every cancelled path ends the run as `Interrupted` (`vm/run.rs`, lines 298 to 304) and a drain reports nothing: the cap before another drain, the compactor's cancel branch, and its returned-before-cancel order. These unit tests are their only pin.
  - `models_loop-tests.rs`, adapter tests with a VM: `lua_type_name` for every `Value` kind against Lua's own `type`; both entries' argument checks, their order, and their texts; `envelope_failure` for an error table and for a string; the chat-table reader against the table `Answer::into_envelope` renders for an overflow, a reply, an empty round, and a batch, matching every field the loop reads; each `Then`'s action tag and values from `act`.
- Integration and end-to-end:
  - Phase 0's argument tests:
    - `execute/tests/models_loop-arguments.rs` (146 lines today): `every_argument_form_resolves_the_list` runs `models.loop(msgs)`, `models.loop(msgs, compactors.fail)`, `other:loop(msgs)`, and `other:loop(msgs, compactors.fail)`, with its model and size assertions unchanged; the two plain-array tests call `other:loop(plain)` in place of `models.loop(models.get('other'), plain)`, with their texts unchanged. New cases: `models.loop(other, msgs)` raises the pointed error before any request; `other.loop(msgs)` raises the colon error; arity in both entries, one case with a trailing nil, checking `kind` and `tostring(err)`; a non-list in both entries raises before any request. If the file would pass 450 lines, the error cases move to a flat peer module registered in `execute/tests.rs`, as `models_loop_compactors.rs` is.
    - `execute/tests/models_loop_compactors.rs`: the compactor-type test (lines 98 to 146) keeps both entries, `models.loop(msgs, bad)` and `writer:loop(msgs, bad)`, with the same expected message for each.
    - `engine/src/lua/tests/errors.rs` and `shims.rs`: `h:infer(prompt)` yields an `infer` request whose handle is `h`, as `models.infer(h, prompt)` did; `models.infer(h, prompt)` raises the pointed error; `h.infer(prompt)` raises the colon error; `h:infer('a', 'b')` raises `handle:infer takes (prompt)`, replacing the three-argument case at `errors.rs` line 174; the two `pcall(models.infer, handle, ...)` cases at `errors.rs` lines 29 and 48 and `handles_reject_colon_methods` in `shims.rs` change as Technical Design lists.
    - Every migrated call site keeps its assertions.
  - Characterization, the work item after Phase 0: the gaps marked in the behavior inventory below. The end-to-end ones go in `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`, driven with `TokioDriver` over `ScriptedChat` or `OverflowClient`, or with the serial `Run` for the dropped-call case, reusing the loop fixtures in `execute/tests/models_loop.rs` (lines 29 to 125) and the model-task fixtures in `execute/tests/model_tasks.rs` and `model_task_notices.rs`. The one shape no real round produces, an answer with no empty detail, goes in `crates/promptforge-internal/engine/src/lua/tests/models_loop_contract.rs`, with canned answers rendered through `Answer::into_envelope`. Every one of them uses only `messages.new()` lists and their builders and asserts only what an author or the caller sees: the list, the returned or raised value, the effects, and the events. Phase 1 keeps every argument behavior, so every characterization case passes on both sides of it.
  - A light userdata for the compactor gap comes from a JSON null in a structured tool's result (`StructuredFixtureTool`, `execute/tests/fixtures.rs`, lines 83 to 94), which mlua turns into a null light userdata.
  - The behavior inventory: each item is one behavior of the shim as Phase 0 leaves it, where it lives, what pins it, and the gap test the characterization work item adds, if any. `__impl_coro.lua` line numbers are `master`'s at `b5ea5d9b4`, before Phase 0, which moves the loop's body into `run_loop` and keeps the rules below the argument checks as they are. There are 34 behaviors, 18 gap tests, and 3 behaviors that only the machine's unit tests can pin.
  - Inventory, arguments as Phase 0 leaves them:
    - **The forms.** `models.loop(messages, compactor?)` runs on the section's current model and `h:loop(messages, compactor?)` on `h`'s frozen binding. Pinned: Phase 0's `every_argument_form_resolves_the_list` (`execute/tests/models_loop-arguments.rs`, lines 11 to 61 before the migration).
    - **The entry checks.** `models.loop` refuses a model handle first with the pointed error, and `h:loop` refuses a first argument other than a model handle with the colon error, both before any yield. Pinned: Phase 0's tests.
    - **Arity.** More than two arguments for `models.loop`, or more than three for `h:loop` with the handle counted, raise `models.loop takes (messages, compactor?)` or `handle:loop takes (messages, compactor?)`, trailing nils counted (lines 270 to 278 before Phase 0). Pinned: Phase 0's tests.
    - **The default compactor.** A nil compactor selects `compactors.fail`, read from the captured `compactors` table at each call and never checked (lines 280 and 281). Pinned: `execute/tests/models_loop_compactors.rs`, lines 13 to 43 and 67 to 97. Gap: an author who replaces `compactors.fail` changes the default, and a non-function put there fails only at an overflow round, with Lua's own call error raised as a string.
    - **A bad compactor.** A non-function raises `compactor must be a function, got {type}`, with `integer`, `number`, and otherwise Lua's `type()` name (lines 282 to 284 and 44 to 47). Pinned: `models_loop_compactors.rs`, lines 98 to 146, in both entries after Phase 0. Gap: a light userdata in `h:loop`'s compactor position reads `got userdata`, Lua's name, rather than mlua's `lightuserdata`.
    - **A bad list.** Since Phase 0, anything but a `messages.new()` list raises the parse's `models.loop needs a messages.new() list; ...` before any yield, in both entries, so the call raises before any request and a pending notice waits for the next valid call. The parse keeps its own check (`protocol/parse/chat.rs`, lines 43 to 51), unreachable through the API. Pinned: `models_loop-arguments.rs`, lines 63 to 146, for a plain array in both entries, a record view, and a waiting notice, as Phase 0 migrates them. Gap: `42`, no argument at all, and `other:loop(other)`.
    - **An empty list.** The chat parse raises `messages must not be empty` at each round, after that round's drain, so pending notices can fill an empty list (`protocol/parse/chat.rs`, lines 52 to 54). Pinned: `protocol/tests/parse_chat.rs`, lines 190 to 196, through a hand-built table only. Gap: an empty list with nothing pending raises it through `models.loop`, and an empty list with one pending notice runs one round over that notice.
    - **An uncaught argument error.** A `lua`-kind raise maps to `Error::LuaRuntime` whose text names the author's prompt line, because it is a Lua `error` above the author's frame (`vm/run.rs`, lines 283 to 293). Pinned: for `models.infer` only (`engine/src/lua/tests/errors.rs`, lines 165 to 194). Gap: an uncaught `models.loop` arity error.
  - Inventory, rounds and exits:
    - **The cap.** At most `max_tool_iterations` rounds, the terminal one included, then `tool_loop_exhausted` with message `tool-call loop did not converge` and no fields, raised after the last batch is appended and before another drain (lines 288 and 323). Pinned: `execute/tests/tool_loop.rs`, lines 49 to 110 and 214 to 233; the kind mapping in `errors.rs`, lines 286 to 294; a fanout arm's stub in `execute/tests/scheduler/fanout/failure_semantics.rs`, lines 115 to 201. Unit only: "before another drain" is invisible to a prompt.
    - **The drain.** Before every round, the first included: one `drain_task_notices` yield, then one user record per notice in queue order, then the `chat` yield (lines 210 to 217 and 289 to 290; `scheduler/notices.rs`, lines 77 to 93). Pinned: the yield order in `engine/src/lua/tests/shims.rs`, lines 252 to 308, and in `quota.rs`; one notice in `execute/tests/model_task_notices.rs`, lines 147 to 202. Gap: two notices drained into one round land as two user records, in the order their tasks ended.
    - **The judging order.** A failed answer, then overflow, then tool calls, then a reply, then the clean exit, then the empty raise (lines 291 to 321). Pinned by the items below.
    - **A failed answer.** A failed `chat` answer raises its error table at the call site, and the typed error is substituted when it escapes the block (line 291; `vm/run.rs`, lines 226 to 243). Pinned: `tool_loop.rs`, lines 235 to 296; `execute/run/tests-drops.rs`, lines 136 to 225; `execute/tests/chat_arm.rs`, lines 282 to 345, for an out-of-scope call and its fields.
    - **Overflow first.** An overflow round calls the compactor before any other rule (lines 292 to 294). Pinned: `chat_arm.rs`, lines 382 to 449; `models_loop_compactors.rs`.
    - **The reply.** A reply appends `{ role = "assistant", content = reply }` and returns nil (lines 310 to 312). Pinned: `execute/tests/models_loop.rs`, lines 126 to 157; `execute/tests/exit_rules.rs`, lines 193 to 224; `chat_arm.rs`, lines 144 to 191. Gap: the call returns exactly one value.
    - **The clean exit.** No reply, no calls, `finish_reason` `"stop"`, and at least one answered call in this loop call append `{ role = "assistant", content = "" }` and return nil (lines 313 to 315). A batch adds all its calls to the count, failed tools included (line 309), and the count starts at zero for each call (line 287). Pinned: `exit_rules.rs`, lines 226 to 253 for the clean exit, and lines 255 to 278 and 297 to 325 for the cases that fall outside it. Gaps: a failing bound tool still counts; a second `models.loop` whose first round is an empty `"stop"` raises, even though the list already holds tool records from the first.
    - **The empty raise.** Otherwise `empty_model_reply`, with the answer's `empty_detail`, else `empty model reply`, as its message, and `finish_reason` when present (lines 316 to 321 and 221). Pinned: `exit_rules.rs`, lines 280 to 295 and 327 to 381; `chat_arm.rs`, lines 346 to 380; the kind mapping in `errors.rs`, lines 304 to 323. Gap: the fallback text, which only a rendered answer with no empty detail produces, since the model client refuses an empty text first.
  - Inventory, batches:
    - **Dispatch order and fields.** Calls run one at a time in order, one `tool_call` yield each, with `alias` the call's name, `args` its arguments table, `call_id` its id, and `turn` the round's turn (lines 186 to 194 and 298 to 300). Pinned: `shims.rs`, lines 252 to 308; `execute/tests/effects.rs`, lines 94 to 157; `execute/tests/local_tools.rs`, lines 60 to 102; `execute/tests/batch_turn.rs`, lines 191 to 351.
    - **The batch rule.** Results are buffered, and the assistant record and the tool records are appended only once every call has its result (lines 295 to 309). Pinned: `models_loop.rs`, lines 371 to 421, where handlers mid-batch see no partial batch; `models_loop.rs` lines 158 to 203 and `tool_loop.rs` lines 69 to 93 pin only that whole exchanges are appended round by round. Gap: a handler that raises its own table on the second call of a batch, under `pcall(models.loop, msgs)`, leaves only the user record in the list, and `err` is that same table.
    - **A bound tool's failure.** It is that call's result, as untrusted text, and the loop goes on (`scheduler/apply.rs`, lines 170 to 189). Pinned: `tool_loop.rs`, lines 153 to 212.
    - **A failed tool answer.** A failed `tool_call` answer raises its error at the call site and appends nothing (line 163). Pinned: for a script call only, `run/tests-drops.rs`, lines 206 to 225. Gap: a dropped model-issued call raises `cancelled` at the `models.loop` call site, and the list holds only the user record.
  - Inventory, local tools:
    - **The handshake.** A `Local` answer (`protocol/render.rs`, lines 184 to 189) makes the shim run the handler under the raw `pcall` between `enter_local_handler()` and `leave_local_handler()`, then yield `local_tool_done` with `ok` and, only when the handler returned, its first return value (lines 145 to 152). The handler's own suspending calls are yields of the same block. Pinned: `shims.rs`, lines 184 to 251; `local_tools.rs`, lines 30 to 58 and 173 to 193; `batch_turn.rs`, lines 227 to 263, for a nested loop.
    - **Returned.** The first return value under the scalar-return rule, with nil read as `""` (`protocol/parse.rs`, lines 415 to 428). Pinned: string returns. Gap: a handler that returns nothing, a number, or two values gives `""`, the number's text, or the first value.
    - **Bad return.** A table return raises the scalar-return error after `TOOL_CALL_FAILED` (`scheduler/tool_call.rs`, lines 273 to 276). Pinned: `local_tools.rs`, lines 345 to 374.
    - **Raised.** `local_tool_done` reports the failure, so `TOOL_CALL_FAILED` fires and `ToolResult` stays silent, and then the handler's own value is raised unchanged, before the answer is looked at (lines 153 and 154). Pinned: `local_tools.rs`, lines 104 to 139; the value's identity only for `tools.call` (`execute/tests/tool_call_arm-local-handlers.rs`, lines 119 to 155). Gap: identity through `models.loop`, covered by the batch-rule gap.
    - **Cancellation.** A handler that fails under cancellation has its raw value raised at once, with no `local_tool_done` yield (line 149). Pinned: `local_tools.rs`, lines 141 to 171, where the run ends `Interrupted`. Gap: that call reports no `TOOL_CALL_FAILED`.
    - **`jump`.** Refused while a handler runs, working again after the loop. Pinned: `local_tools.rs`, lines 205 to 230, 260 to 282, and 322 to 343.
  - Inventory, the compactor:
    - **The call.** `raw_pcall(compactor, overflow_reason)`, inside the block, so the compactor may yield (line 230). Pinned: `chat_arm.rs`, lines 382 to 449, for the reason tags. Gap: the compactor gets exactly one argument, and one that calls `models.infer` before raising suspends and resumes normally.
    - **Returning.** Raises `the selected compactor returned without raising: replacement compactors are deferred; compactors.fail is the only shipped policy` (lines 231 to 236). Pinned: `models_loop_compactors.rs`, lines 147 to 186, by substring only. Gap: the exact text.
    - **Raising.** Under cancellation the raw value (line 237); otherwise `normalize_failure(value)` (line 238). Pinned: `models_loop_compactors.rs`, lines 13 to 66 for typed exhaustion from `compactors.fail`, lines 187 to 221 for cancellation, and lines 222 to 253 for a string passing through. Gap: an author's own table passes through as the same table. Unit only: the cancel branch and the returned-before-cancel order.
  - Inventory, everything else:
    - **Events come from the scheduler.** The scheduler reports every round and call, and the shim reports none of its own (`scheduler/chat.rs`, lines 11 to 19). Pinned in part by the filtered sequences in `exit_rules.rs` and `tool_loop.rs`. Gap: the full ordered trace of observations and content reports for one bound-tool round, one local-tool round, and a reply.
    - **The error shape.** Every raise is an error table under the shared metatable, raised at level 0, so `tostring(err)` is exactly the message and `err.kind` is readable (lines 54 to 56). Pinned by every `tostring(err)` comparison above.
    - **A frozen handle.** The receiver's binding serves every round of `h:loop`. Pinned: `models_loop.rs`, lines 340 to 370, as Phase 0 migrates it.
    - **Instruction cost.** `quota.rs`, lines 77 to 172, counts the shim's Lua instructions per round. See Regression.
    - **Globals read at call time.** The Phase 0 shim reads `type`, `select`, `ipairs`, `tostring`, and `error` as globals, so an author who rebinds one changes the loop. Unpinned, and left unpinned: the port reads only `error` that way (see Assumptions, risks, and notes).
- Regression, security, and performance:
  - Every existing test passes after Phase 0's migration, and unchanged across Phase 1, except `quota.rs`.
  - `quota.rs` (lines 77 to 172) is the only test that depends on Lua instruction counts or hooks spent inside the shim. Its counting hook will see only the trampoline: by estimate about nine instructions per yield, so about 27 for its measured round of three yields, which is under the 300 ceiling and just over the 20 floor. Its doc (lines 1 to 6 and 20 to 37) says that every instruction the shim spends per round is counted and that a round under the floor means "the loop runs outside the shim", which is now true by design. As the `quota.rs` decision says, it is rewritten: the doc says it bounds the trampoline's cost; the floor becomes one instruction per yield in the span (three), which only proves the hook fires on the loop thread; and the ceiling is re-measured and set at about three times the measured cost. After Phase 2 a round has two yields, so it is measured again.
  - Every other hook test loops in author code rather than counting shim instructions: `engine/src/lua/tests/coroutine.rs`, `lua/src/tests/cancellation.rs`, and `lua/src/tests/budgets.rs`.
  - The `models_loop` bench (`engine/benches/models_loop.rs`) should keep its speed; its doc (lines 1 to 6) says the loop runs inside `__impl_coro.lua` and is updated.
  - Security: every test of trust and wrapping stays as it is, because the Engine still wraps tool output before it answers.
- Exit criteria:
  - The verification commands in `AGENTS.md` pass after each landed change.
  - The facade surface check passes with no diff.
  - The `promptforge-docs` books rebuild cleanly after Phase 0's docs change (`promptforge-docs/CONTRIBUTING.md`, line 7).

</verification-contract>
<decision-record>

## Decision Record

"The owner" below is the repository owner, and "the owner's request" is the request that started this plan. A call that runs on a model handle becomes a method of the handle, so every argument has one meaning. Then the loop's rules move into a typed Rust state machine, and the loop entries stay a Lua trampoline only because a Rust frame can neither yield nor host a yielding call. Exactness comes first: Phase 0 makes every author-visible change, characterization tests pin its behavior before any port code exists, and Phase 1 keeps the protocol and every raised value identical. The Engine-side improvements land afterward, each as its own change.

- Decisions:
  - Phase 0 replaces the optional leading handle with methods on the handle, for both loop and infer: `models.loop(messages, compactor?)` and `models.infer(prompt)` run on the section's current model, and `h:loop(messages, compactor?)` and `h:infer(prompt)` run on the handle's binding. Every argument then has one meaning, so the guessing rule and every error that came from guessing wrong go away. It overturns the documented rule that model handles have no methods (`promptforge-docs/src/language/10-models.md`, line 232; `models-userdata.rs`, lines 4 to 7), whose one deliberate exception was the message list (`messages.rs`, lines 3 to 6); tool handles keep the rule. The colon form matches the list's `msgs:user(...)`, and a dot call gets its own error. Owner's words: "what if we give the model handle the loop function, so you write model.loop(...)", then, on doing `infer` in the same change and before the characterization tests, "Yeah lets do that".
  - Every loop argument check runs before the first yield from Phase 0 on, the list check included, with the parse's text. A value other than a list then raises before any request, where today it costs one `chat` yield that the parse refuses, and a call outside a coroutine reports the list error instead of the failed yield; the notices already wait for a valid round through the drain guard and still do. Phase 1's Rust checks then match the Phase 0 shim exactly. Adopted on 2026-10-09; the owner was asked and raised no objection.
  - Emptiness stays a chat-parse check after each drain, so pending notices can fill an empty list and `h:loop(messages.new())` runs one round over them. The owner's choice on 2026-10-09.
  - The loop's rules run in Rust behind a Lua trampoline. A Rust function cannot yield without mlua's `async` feature and cannot call a Lua function that yields, so the only Rust design that keeps the coroutine protocol is one whose Rust frames always return before Lua yields. Owner's request: "The chosen design is therefore a trampoline".
  - Characterization tests come before the port, run against the shim as Phase 0 leaves it, and must pass unchanged after Phase 1. Owner's request: "The plan's first work item adds tests that pin every gap against the CURRENT shim, written so they pass unchanged before and after the port." The owner's later handle-method decision puts Phase 0 ahead of them, so the shim they pin is the one Phase 1 ports.
  - Phase 1 keeps the yield protocol identical, and the Engine's parse, dispatch, and rendering keep their behavior; the only edits there share one helper and one text constant without a behavior change. Owner's request: "The Engine's parse, dispatch, and answer rendering stay untouched."
  - The machine is generic over the Lua values it only passes along, so its tests need no VM. Owner's request: "the rules are unit-testable without a VM".
  - Every error the loop raises is a `"raise"` action performed in Lua with `error(value, 0)`, rather than an `Err` from `loop_begin` or the step closure. The block guard, `raised_from`, and the retained-error substitution all depend on the raised value being the shim's table, raised from Lua.
  - The trampoline calls handlers and the compactor, because either may yield.
  - Records are built in Rust and pushed with `MessageList::push`. Owner's request: "List writes go through the messages plan's `MessageList` Rust API, with records built in Rust."
  - The declarations are the minimum the goals need. The machine declares four types, `Machine`, `Phase`, `Input`, and `Then`, and reuses `ChatResult`, `ToolCallEvent`, `ToolCallRecord`, `OverflowReason`, `Raised`, and `MessageList` with its record types, so no type duplicates one the code has. The adapter declares no type. Phase 0 adds one predicate, one registry key, and one reader, and Phase 3 makes `ChatResult` itself the userdata. Owner's words: "review the declarations and make sure they are the minimum API that achieves the goal. Reuse existing declarations in the code if doing so preserves functionality and removes a type."
  - `compactors` and `max_tool_iterations` leave the chunk's arguments for `loop_begin`'s captures, `is_message_list` leaves for `messages::is_list`, and `loop_begin` becomes the chunk's last argument, private in the same way `yield` is. Each call of it returns a step closure that owns that call's state, so Lua never holds a state handle. Owner's words: "can we make it so the call is a function with a userdata closure, so the Lua doesn't have to utter a handle".
  - The Rust step owns the whole local-tool handshake for the loop's calls: the trampoline only calls the handler under the raw `pcall` between the two depth captures, and the step does the cancel check, builds the `local_tool_done` request, and applies the re-raise rules; it owns the compactor's rules the same way. `tools.call` keeps the Lua `run_local_tool` and `dispatch_tool`, so the handshake exists twice, about fifteen lines, and the test-only `tools.call_as_model` hook keeps exercising the Lua copy. This delivers the one-language rules and VM-free tests the request asks for. Adopted on 2026-10-09; the owner was asked and raised no objection.
  - Phase 2, the drain fold, is adopted, after Phase 1. It needs no public API change: none of its types appears in `crates/promptforge/public-api.txt`. It is a deliberate reordering rather than a pure refactor: the `chat` of the chain that owns the model tasks is issued in the chain step that finished its batch, so round ids, effect ids, and event order change across chains whenever another chain is ready when that chain's batch ends, while notices still land in the same rounds. It stays deterministic and no run logs exist to migrate. Adopted on 2026-10-09; the owner was asked and raised no objection. Owner's request: "Engine-side improvements, landable separately after Phase 1."
  - Phase 3, the chat half of the opaque-answer change, is adopted, after Phase 2. It keeps reply text, the finish reason, the empty detail, the serving model, and the metrics out of Lua, a contained change whose benefit is one fewer copy of each reply and no per-round metrics table. Adopted on 2026-10-09; the owner was asked and raised no objection.
  - `quota.rs` stays, as a bound on the trampoline: its doc is rewritten, its floor becomes proof that the counting hook fires, and its ceiling is set at about three times the measured cost, the way the current ceiling was set (86 measured, 300 allowed; `quota.rs`, lines 21 to 27). It is the one test that would notice rules creeping back into Lua. Adopted on 2026-10-09; the owner was asked and raised no objection.
  - Stale `AGENTS.md` lines give way to the plan: a conflicting line is deleted, not reworded, so Phase 0 deletes the methodless-handle bullet from `crates/promptforge-internal/lua/AGENTS.md` and, by the same judgement, the same rule's bullet from the lua crate's `## Invariants` list. Owner's words: "When AGENTS.md conflicts with the plan, remove the conflicting lines from AGENTS.md. Prefer shrinking AGENTS.md over the alternatives. AGENTS.md can be stale, don't assume they are correct."
  - Phase 0's docs run as a vibe step across two repositories: the change is committed in `promptforge-docs`, and `promptforge` records only the plan marker. The owner's choice on 2026-10-09, over a follow-up outside the run.
  - One run covers every phase, Phase 0 through Phase 3. The owner's choice on 2026-10-09.
- Rejected alternatives:
  - Porting the landed guessing rule exactly to Rust. Reason: it keeps a rule that reads the second argument to decide what the first one is, and that rule needed two fixes on 2026-10-09. Revisit: never, once Phase 0 lands.
  - Taking any userdata other than a list as the handle. Reason: it breaks `models_loop-arguments.rs` line 113, where `models.loop(msgs[1])` must raise the list error, and gives a handle error for a lone tool handle or light userdata. Revisit: never.
  - `h:loop` without `h:infer`. Reason: authors who learn `h:loop` would try `h:infer` and get an error; moving both costs about the same as moving one. Revisit: never.
  - An options table, `models.loop(msgs, { model = h })`, which keeps handles methodless. Reason: it reads worse and changes how the compactor is passed. Revisit: if methods on handles cause trouble the docs cannot explain.
  - Keeping the handle-first forms beside the methods. Reason: the guessing rule would stay. Revisit: never; every shipped prompt already uses the list-first form.
  - Refusing an empty list before notices land, raised in review after commit `b5ea5d9b4`. Reason: the owner kept today's behavior, where notices can fill an empty list. Revisit: if an empty list with pending notices proves confusing in practice.
  - Design B for the local-tool handshake: the trampoline calls the shim's existing `run_local_tool(handler, args)` and `compact(compactor, reason)`. Reason: one copy, exact by construction, but those rules stay in Lua and out of the VM-free tests. Revisit: if the two copies of the handshake drift.
  - Deleting `quota.rs`, since no instruction budget limits a block (`crates/promptforge-internal/lua/src/lib.rs`, lines 90 to 94). Reason: it is the one test that notices rules moving back into Lua. Revisit: if the trampoline's cost stops mattering.
  - The earlier declaration set, with `Action`, `LoopState`, `Batch`, `Call`, `Round`, `Dispatched`, `Next`, `Raise`, and `Awaiting` beside the machine, and a `ChatAnswer` wrapper in Phase 3. Reason: each duplicated an existing type (`ChatResult`, `ToolCallEvent`, `Raised`, `MessageList`) or a projection the code already has (the action tags, the machine's `Phase`), so thirteen types became four. Revisit: never.
  - A machine with no type parameter, the adapter holding every Lua value. Reason: the handler's raised value must outlive the `local_tool_done` yield, so the adapter would need a side slot and the rules would split across two layers; an opaque `V` keeps every value in the machine's typed flow. Revisit: never.
  - One machine method per input in place of `Input` and `step`. Reason: six entry points for one, with the same phase check in each. Revisit: never.
  - Yielding from Rust with mlua's `async` feature and `yield_with`. Reason: it needs the feature and an async-driven VM, while the scheduler steps coroutines synchronously and owns no executor (`scheduler.rs`, lines 1 to 21). Revisit: if the Engine ever drives Lua through mlua's async API.
  - Calling handlers and the compactor from Rust. Reason: either may yield, and a yield across a Rust frame fails. Revisit: never.
  - Keeping the loop in Lua over the Rust list. Reason: the rules stay untyped and testable only through a VM, the opposite of the request. Revisit: if the port cannot be shown exact.
  - Raising loop errors as Rust callback errors. Reason: they reach Lua as opaque mlua error values, which changes what the guard stashes and what an author's `pcall` sees. Revisit: never.
- Assumptions, risks, and notes:
  - Verified at `b5ea5d9b4`: the chat parse gives the list error for every value other than a `MessageList`, a record view included (`protocol/parse/chat.rs`, lines 43 to 51), so one shared text serves the Phase 0 shim, the adapter, and the parse.
  - In Phase 1 the `ChatResult` the adapter reads back from the rendered table leaves `model` empty, `metrics` as `None`, and each call's `tool` as `None`, because the table omits `tool` and the loop reads none of the three; Phase 3 hands the machine the scheduler's own value with all three set.
  - The model-handle check reads a handle's type rather than a role's identity: `is::<LuaModelHandle>()` passes any model handle, frozen or not, as `call_handle` does today.
  - `h.loop` and `h.infer` read the registry table that `install_shim_prelude` fills. In production that runs only in `setup_section_vm`, right before `models.loop` is installed, so the methods exist exactly where the namespace functions do; a test VM that runs the prelude without the loop install still sees `h.loop`, which no test relies on.
  - `fail`'s branch for a value other than a table is reachable only through a hand-built envelope. The port calls the `tostring` global from Rust there, so an author `tostring` that raises or yields fails differently on that unreachable path.
  - Two reads may call author code from Rust: `compactors.fail` through an `__index` metamethod an author put on `compactors`, and the `tostring` global above. Such code cannot yield there, while the shim's reads could. Neither occurs in production prompts.
  - The port's argument checks ignore an author's rebinding of `type`, `select`, or `ipairs`, which today changes the shim. This is accepted as a fix; the trampoline still reads `error` as a global, as the shim does.
  - Unreachable answer shapes: the step reads answers only in the renderer's shapes. An empty tool-call batch, which the model client refuses (`model-client/src/client/tests.rs`, lines 98 to 103), would append an assistant record with no calls where the shim's record would fail validation. A model-issued bound result is always text (`scheduler/apply.rs`, lines 170 to 189).
  - An exhausted Lua heap inside a step surfaces as a callback error wrapping the memory error rather than as a bare memory error: the same `lua` kind, a different message.
  - After the port, the test-only `tools.call_as_model` hook exercises the Lua copy of the local-tool handshake, which `tools.call` still uses, rather than the loop's Rust copy. The loop's copy is covered by `local_tools.rs`, `batch_turn.rs`, `models_loop.rs`, and the new characterization tests.
  - The comment in `model_task_notices.rs` (lines 149 to 151) says the child ends "between its drain and its round-2 chat". By the scheduler, a spawned task is admitted only when the ready queue empties (`scheduler/drive.rs`, lines 143 to 148), which is after round 2 is issued; that is also why the test passes with or without the Phase 2 fold.
  - Phase 2's reordering may break multi-chain tests that hand scripted replies out in issue order; each such test is reviewed and its script change recorded in the change.

### Deferred and Out of Scope

- Deferred: model-issued tool answers as opaque handles, the half of Phase 3 that keeps bound-tool text out of Lua. Revisit when the `tools.call_as_model` test hook is retired or rebuilt over the step.
- Deferred: porting `tools.call`'s local-tool handshake to Rust, which would remove the second copy. Revisit if the two copies drift or `tools.call` needs typed state.
- Deferred: methods on tool handles. `tools.call(tool, args)` has nothing to guess. Revisit if authors expect `tool:call(args)` after `h:loop`.
- Out of scope: porting `models.infer` to Rust, and `call`, `tasks`, `fanout`, and the store shims. Phase 0 changes only how `models.infer` takes its handle.
- Out of scope: the messages refactor's own work (`vibe/2026-10-09-2-messages-userdata-refactor.md`), except that Phase 0's docs change carries its open chapter 11 and 17 follow-up (lines 21 and 779).

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <crate>`, such as `cargo build --locked -p promptforge-lua` or `-p promptforge-engine`. The workspace `default-members` is only `crates/gateway/app`, so a plain `cargo build` builds just the gateway (`cargo build --locked -p gateway`); the headless gateway is `cargo build --locked -p gateway --no-default-features` and the desktop app is `cargo build --locked -p workshop`. The gateway and Workshop build scripts bundle the UIs, so CI runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` before those builds. The Engine crates build with no npm step; their only build dependency is `build-ceiling`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>`, adding `--lib` for unit tests only or `--test <target>` for one integration binary. For example, `cargo nextest run --locked -p promptforge-engine --lib models_loop` runs the Engine's `models.loop` suites, and `cargo nextest run --locked -p promptforge-lua --lib <substring>` runs Lua-layer unit tests. CI also runs single named tests as `cargo test --locked -p <crate> --test it <test_name>`. One JS file runs with `node --test <path>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate>` runs a crate's unit and integration tests. The integration target is `suite` for `promptforge` and `harness`, and `it` for every other crate with a `tests/it/main.rs`; crates with flat integration files (`gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-cloud-providers`, `build-workshop`) use the file stem. Test-only features are `test-support` (`promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `promptforge-plugin`) and `test-fixtures` (gateway and Workshop crates); the Engine's dev-dependencies already enable `test-support` on `promptforge-lua` and `promptforge-parser`, so its unit tests need no flag. The two criterion benches run with `cargo bench -p promptforge-engine --features test-support --bench models_loop` and `cargo bench -p promptforge-lua --features test-support --bench surface`. A JS package runs with `npm test --workspace <ui|look|platform>` inside `crates/workshop`, or `npm test` inside `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then the Workshop crates separately with `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless`. The structural and boundary checks are `cargo test -p build-xtask` (part of the workspace run), and CI's `api-surface` job adds `cargo nextest run --locked -p build-xtask --run-ignored only` on the pinned nightly. The JS suites are `npm test --workspaces --if-present` inside `crates/workshop` and `npm test` inside `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and for Workshop `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. The one standalone check is the headless build shape `cargo check -p gateway --no-default-features`; `AGENTS.md` forbids a separate `cargo check --workspace` beside clippy. CI also runs `cargo deny check`, `cargo audit`, `cargo hakari verify`, and a check that `ring` stays out of the gateway's normal dependency closure; `.githooks/pre-push` runs the headless check, the main clippy run, and `cargo deny` when installed. TypeScript is checked with `npm run typecheck --workspaces --if-present` inside `crates/workshop` and `npm run typecheck` inside `crates/gateway/config-ui/ui`. The root `Cargo.toml` `[workspace.lints]` deny `clippy::all`, `pedantic`, `unwrap_used`, `expect_used`, `allow_attributes`, `allow_attributes_without_reason`, `exit`, and `unsafe_code`, and warn on `missing_docs`, `missing_debug_implementations`, and `unreachable_pub`; `clippy.toml` allows unwrap and expect in tests and bans process-global installers such as `std::panic::set_hook` and `tracing::subscriber::set_global_default`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`, and `.githooks/pre-commit` runs it). No JS formatter check is configured in CI.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, and for the facade with default features `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`. CI also builds `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`, all with `-D warnings`. The facade surface check is `cargo +nightly-2026-09-05 xtask api --check`; the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs` and the listing is compared against `crates/promptforge/public-api.txt`.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file `<stem>-tests.rs` wired from `<stem>.rs` with `#[cfg(test)]`, `#[path = "<stem>-tests.rs"]`, and `mod tests;` (190 such files), for example `crates/promptforge-internal/lua/src/models-tests.rs` beside `models.rs`. Extra files for one stem take a dash label, such as `messages-tests-list.rs` and `prelude-tests-environment.rs`.
  - Larger groups use a `tests/` subdirectory of topic files. The Engine's run tests live in `crates/promptforge-internal/engine/src/execute/tests/`, declared from `execute/tests.rs` (`mod models_loop;`, `mod models_loop_compactors;`), and a topic's extra files are dash-labeled siblings wired by path, as `models_loop.rs` does with `#[path = "models_loop-arguments.rs"] mod arguments;` and `#[path = "models_loop-author-shapes.rs"] mod author_shapes;`. The Lua layer has `crates/promptforge-internal/lua/src/tests/` and `protocol/tests/`.
  - Integration tests are one binary per crate at `tests/it/main.rs` (or `tests/suite/main.rs` for `promptforge` and `harness`) with sibling modules and `fixtures/` or `common/` directories; a few gateway crates and `build-workshop` use flat files under `tests/` instead. Integration binary roots carry `#![expect(clippy::expect_used, reason = "...")]`; unit tests rely on `clippy.toml`'s `allow-expect-in-tests` and `allow-unwrap-in-tests`.
  - Test names are full descriptive sentences in snake case, such as `models_loop_appends_the_terminal_assistant_record_and_returns_nil` and `models_loop_dispatches_local_and_bound_tools`; Engine run tests are `async fn` under the tokio test macro.
  - No doctests: the `build-xtask` `no_doctests` check fails any doc comment holding a code block rustdoc would compile, so examples go in `text`, `json`, or `toml` fences.
  - The benches (`models_loop` in `promptforge-engine`, `surface` in `promptforge-lua`) are criterion targets with `harness = false` and `required-features = ["test-support"]`; the Engine's `test-support` feature exists only for its bench and provides the serial `test_support::drive` and tokio `test_support::drive_tokio` drivers.
  - JS tests are `*.test.mjs` under `src/` or `*.mjs` under `test/`, run with `node --test`. `.config/nextest.toml` defines `default` and `ci` profiles with a 60 second slow timeout and a `heavy` test group for the STT packages.
- Directory map:
  - `crates/` holds every Rust crate and the TypeScript packages. Flat crates: `promptforge` (Engine facade), `promptforge-plugin` (Plugin contract), `plugin-mcp`, `plugin-web`, `plugin-user-input` (Plugins), `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `workspace-hack` (cargo-hakari), and the `build-*` tooling crates (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`).
  - Manifestless containers hold each family's private crates: `crates/promptforge-internal/` (types, vfs, model-client, lua, parser, engine), `crates/harness-internal/runner`, `crates/gateway/` (app, config, config-ui, local, logging, progress, protocol, routing, cloud-providers, web-search, and `stt/` with api, engine, backend-whisper, whisper-ffi), and `crates/workshop/` (desktop, server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace, run-log, agents, plus the npm workspaces `ui`, `look`, `platform`). `crates/shared-ui` is a TypeScript and CSS package, not a crate. The root `Cargo.toml` lists the container members explicitly because Cargo prunes excluded subtrees from member globs.
  - `crates/promptforge-internal/lua/src/` is the Lua VM layer: flat Rust modules plus pure-Lua shims pulled in with `include_str!` (`__impl_coro.lua` at 433 lines, `__impl_tasks.lua`, `__impl_fanout.lua`, `__impl_globals.lua`, `__impl_store.lua`), with `vm/`, `protocol/`, `tools/`, and `tests/` subdirectories. `models.rs` builds the `models` Engine global (`use`, `default`, `get`) over the shared `ModelSet` from `promptforge-model-client`, with `models-userdata.rs` (`LuaModelHandle`) beside it. `messages.rs` defines the Rust `MessageList` behind `messages.new()`, with `messages-view.rs` beside it. `compactors.rs` holds the overflow reasons, the one shipped compaction policy, and the pre-dispatch precheck.
  - `models.loop` today is the Lua function `models_loop` in `__impl_coro.lua` (signature `(handle?, messages, compactor?)`), built on the shim-local helpers `infer`, `dispatch_tool`, `tools_call_as_model`, `append_record`, `drain_task_notices`, and `compact`. Past the run's `max_tool_iterations` round cap it raises `tool_loop_exhausted`, and it emits no events. `coro.rs` compiles the shim, stashes `loop` under the registry key `promptforge.impl_coro.loop`, and `install_section_loop_shim` (re-exported from the crate root) sets it as `models.loop`; `crates/promptforge-internal/engine/src/execute/section_vm.rs` calls that per section.
  - `crates/promptforge-internal/engine/src/` steps runs: `execute/` (`scheduler.rs`, `section_vm.rs`, `section_context.rs`, `config.rs`, `run/effect.rs`, and the `tests/` directory with `models_loop*.rs`, `models_loop_compactors.rs`, and `tool_loop.rs`), plus `benches/models_loop.rs`.
  - `guide/` holds only mdBook configs (`books/gateway`, `books/language`, `books/workshop`) and the `chrome/` and `landing/` assets; the chapter sources live in a separate `promptforge-docs` checkout that `build-user-guide` assembles. `prompts/` holds example prompts, `tools/` holds `.mjs` sidecar staging and live-test scripts, `vibe/` holds tracked plan files, and `images/` holds README art.
  - `.github/workflows/` holds CI and release workflows, `.githooks/` holds `pre-commit` and `pre-push`, `.config/` holds `nextest.toml` and `hakari.toml`, `.cursor/rules/` holds `workshop-architecture.mdc` and `workshop-spa.mdc`, and `.cargo/config.toml` sets the Windows `rust-lld` linker with static CRT and defines the `xtask` and `workshop` aliases.
  - Root config: `Cargo.toml`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml` (cargo-dist), and `gateway.local.example.toml`. Ignored: `local/` (developer configs, prompts, and STT fixtures), `target/`, and `target-msrv/`; `cabinet/` is untracked.
- Component boundaries (normal dependency directions from `cargo metadata`, ignoring `workspace-hack`):
  - Engine: `promptforge-types` and `promptforge-vfs` depend on nothing in the workspace; `promptforge-model-client` depends on types; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; `promptforge-engine` on lua, model-client, parser, types, and vfs; the `promptforge` facade depends on all six. A change to `models.loop` stays inside `promptforge-lua` and `promptforge-engine` unless it changes a type the facade re-exports, which `cargo xtask api --check` and `public-api.txt` would catch.
  - Plugin contract: `promptforge-plugin` depends only on `promptforge-types` and `promptforge-vfs`. `plugin-mcp`, `plugin-web`, and `plugin-user-input` each depend only on `promptforge-plugin`.
  - Harness: `harness-runner` depends on `promptforge` and `promptforge-plugin`; `harness` on `harness-runner`, `promptforge`, and `promptforge-plugin`; `harness-gateway-client` on `harness`, `plugin-web`, and `promptforge`.
  - Gateway: `gateway-api-types` is the leaf; `gateway-config` and `gateway-progress` sit on it, `gateway-protocol` on api-types and config, `gateway-routing` on config and protocol, `gateway-local` on config, progress, protocol, and routing, and `gateway-stt` on config, local, progress, and the speech engine crates. The `gateway` app depends on its family (config-ui, local, stt, and web-search optional), `gateway-api-discovery`, and `shared-loopback`. No gateway crate depends on the Engine or the Harness.
  - Workshop (a Host): `workshop-protocol` and `workshop-support` are leaves and `workshop-registry` sits on protocol; `workshop-gateway`, `-menu`, `-status`, `-user-state`, and `-workspace` sit on that base; `workshop-agents` depends on `harness`, `plugin-user-input`, and `promptforge`; `workshop-run-log` on `harness`. `workshop-server` is the top, depending on the Harness, the three Plugins, `promptforge`, `promptforge-plugin`, and the `workshop-*` crates; `workshop-server-api` depends on `workshop-server`, and `workshop` on `workshop-server-api` and `gateway-api-discovery`.
  - `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, lint inheritance, and the Engine guards in `crates/build-xtask/src/engine_guards.rs`: a manifest guard, a `test-support` leak guard over the whole workspace, and a retired-symbol scan whose seed list includes `run_models_loop`, so live Engine source outside tests may not name that identifier.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain; every crate inherits the workspace lints with `[lints] workspace = true`, and `missing_docs` and `unreachable_pub` warn, so crates keep a minimal public surface with `pub(crate)` by default.
  - Every crate's `lib.rs` opens with `//!` crate docs ending in a `## Invariants` list, and each module opens with `//!` docs saying what it owns. The defined terms Engine, Harness, Host, and Plugin are capitalized and mean one thing each; Engine crates call their driver "the caller" and never mention the Host. `crates/workshop/ui/test/docs-claims.mjs` scans `AGENTS.md`, `## Invariants` docs, and `.cursor/rules` for these rules.
  - The Engine performs no I/O: every model reply, tool result, timer, or file is an effect the caller answers, and Lua-side suspensions are coroutine yields parsed by the `protocol/` modules.
  - Source directories are flat: a subdirectory needs at least three files, otherwise siblings are named `foo-bar.rs` and wired with `#[path = "foo-bar.rs"] mod bar;`.
  - Dependencies are declared once in `[workspace.dependencies]` with a comment explaining each pin or feature set, and crates use `workspace = true` plus `workspace-hack`.
  - Comments state a non-obvious constraint or cite an upstream issue URL for a workaround. Error messages are concise and self-contained for model consumption, naming required versus actual, as in `models.loop takes (handle?, messages, compactor?)`.
  - JSON that reaches a recorder or replay comparison round-trips exactly (`float_roundtrip`, sorted keys, finite numbers).
  - Behavior changes ship with tests in the same change; types and compiler checks come first, then behavior tests and deterministic fault injection.
  - Commits use short imperative summaries, such as `Fix models.loop compactor error and bound list pairs`, and a finished plan lands as `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

- Before starting:
  - Read `AGENTS.md` at the repository root and `crates/promptforge-internal/lua/AGENTS.md`. `AGENTS.md` files may be stale. Where a line in any `AGENTS.md` conflicts with this plan, the step that meets the conflict deletes that line, preferring deletion to rewording; a conflict whose resolution is unclear returns blocked for the owner.
  - Check that `git -C c:\Users\Vinnie\cursor\promptforge rev-parse --short HEAD` prints `b5ea5d9b4`, the commit every line number in this plan was checked against, and that `git -C c:\Users\Vinnie\cursor\promptforge-docs status --short` prints nothing. If HEAD differs, re-check the cited lines of every file a step names before editing, and correct the plan where they moved.
  - The messages refactor has landed, stopgap included (commits `d7a051f06` to `8ac23f411`, then `b5ea5d9b4`). Step 1 removes the stopgap's guessing rule and drain guard, and Step 4 removes the rest.
  - Line numbers are `master`'s at `b5ea5d9b4`, before Step 1, unless a step says otherwise. Each step moves code, so every later step finds a cited passage by its content.
  - Run commands from `c:\Users\Vinnie\cursor\promptforge`, except where Step 2 names `c:\Users\Vinnie\cursor\promptforge-docs`.
  - Coding, review, and fix sub-agents read only Technical Design, Project Survey, and their own step. Each step therefore restates, verbatim, each Testing Plan test it owns and each Functional Specification rule or text it relies on. Where a step says "as Technical Design declares", that declaration is the spec.
- Gates:
  - The verification set, run after every step that edits `promptforge`, is the `## Verification` list in `AGENTS.md`:
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`;
    - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny`, and the headless build shape `cargo check -p gateway --no-default-features`;
    - `cargo fmt --all --check`;
    - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps`, both with `RUSTDOCFLAGS="-D warnings"`;
    - `cargo +<pinned nightly> xtask api --check`, where `<pinned nightly>` is the toolchain named in `crates/build-xtask/src/api/toolchain.rs`. It passes with no diff in every step, because no step changes `crates/promptforge/public-api.txt`;
    - `cargo test -p build-xtask`, which enforces the boundaries, the `## Invariants` marker, and, with the build, the 500-line ceiling on every `.rs` file under `src`, `tests`, `benches`, and `examples` of the lua and engine crates (`crates/build-ceiling/src/lib.rs`, line 14), test files included.
  - Focused runs while coding: `cargo nextest run --locked -p promptforge-lua --lib <substring>` and `cargo nextest run --locked -p promptforge-engine --lib <substring>`, such as `--lib models_loop`.
  - Step 2's gate is the books build that step names.
- Components, in dependency order:
  1. Handle methods (Steps 1 and 2), Phase 0. First, because it is the one author-visible change, the contract tests pin the shim it leaves, and every later step keeps that behavior. Its two pieces are sequential: the Engine change, then the guide, which describes the shipped forms and depends only on Step 1.
  2. Loop port (Steps 3 and 4), Phase 1. After the handle methods, because it ports the shim Step 1 leaves. Its two pieces are sequential: the contract tests land first and pass on Step 1's shim before any port code exists, then the port passes them unchanged, which is the proof that it is exact. Inside Step 4 the machine, the adapter, the trampoline, and the chunk's new argument are built jointly, because none works without the others; the machine alone would also be dead code under `CARGO_BUILD_WARNINGS=deny`.
  3. Drain fold (Step 5), Phase 2. After the port, because it deletes the machine's drain phase and the `drain_task_notices` request the port still yields. One piece.
  4. Opaque chat answer (Step 6), Phase 3. After the drain fold: the two are independent in behavior, but both edit the adapter's `read_input`, so they land one after the other. One piece.
- One run, six steps, in this order. Each step is one commit holding its code and its tests under the subject the step names, and passes its gates before the next starts. Step 2's commit lives in `promptforge-docs`, and `promptforge` records only its plan marker, as Step 2 says.

<step-1>

### Step 1: Give model handles loop and infer methods [completed]

- Component: Handle methods

- Piece: the Engine change, the first of the component's two sequential pieces. Step 2 documents what it ships.
- Rules it implements, from Functional Specification, verbatim:
  - The forms:
    - `models.infer(prompt)` runs one round on the section's current model, as the one-argument form does today.
    - `h:infer(prompt)` runs one round on `h`'s frozen binding, as `models.infer(h, prompt)` does today.
    - `models.loop(messages, compactor?)` runs on the section's current model, as today's list-first form does.
    - `h:loop(messages, compactor?)` runs every round on `h`'s frozen binding, as `models.loop(h, messages, compactor?)` does today.
  - `h.infer` and `h.loop` read the shim's functions wherever the shim prelude ran; in production `models.loop` is installed right after it (`section_vm.rs`, lines 148 and 149), so both methods exist exactly where `models.infer` and `models.loop` do, and read nil elsewhere, as `models.loop` does.
  - A handle keeps its fields, and `type(h)` stays `'userdata'`. Tool handles keep their current form.
  - Each method sets the request's `handle` from its receiver, exactly as the handle-first call did, so every effect and event stays the same.
  - Arguments, checked before any yield in this order, each failure raised as a `lua`-kind error table, in the Phase 0 shim and from Phase 1 in Rust with the same texts. The rest of the plan calls the error for a handle passed to `models.loop` or `models.infer` the pointed error, and the error for a method called without a model handle as its receiver the colon error:
    - `models.infer`: a model handle first raises `models.infer takes (prompt); call handle:infer(prompt) to run on a model handle`; more than one argument otherwise raises `models.infer takes (prompt)`.
    - `h:infer`: a first argument other than a model handle, which is a call written with a dot or the function taken off the handle and called bare, raises `call infer on a model handle with a colon: handle:infer(prompt)`; more than two arguments, the handle included, raise `handle:infer takes (prompt)`.
    - `models.loop`: a model handle first raises `models.loop takes (messages, compactor?); call handle:loop(messages, compactor?) to run on a model handle`; more than two arguments otherwise, trailing nils counted, raise `models.loop takes (messages, compactor?)`.
    - `h:loop`: a first argument other than a model handle raises `call loop on a model handle with a colon: handle:loop(messages, compactor?)`; more than three arguments, the handle included and trailing nils counted, raise `handle:loop takes (messages, compactor?)`.
    - Both phases check model handles with `models::is_handle`, an exact `is::<LuaModelHandle>()` test that Phase 0 adds.
    - The compactor, second for `models.loop` and third for `h:loop`: nil selects `compactors.fail`, read through an ordinary index of the `compactors` table captured at install and not checked; a function is used as given; anything else raises `compactor must be a function, got {type}`, where `{type}` is `integer` for an integer and Lua's `type()` name otherwise, so a full or a light userdata reads `userdata`.
    - The list, first for `models.loop` and second for `h:loop`: a `messages.new()` list passes; anything else raises the parse's list text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace` (`protocol/parse/chat.rs`, lines 16 and 17).
  - The return value of either loop entry is exactly one nil.
- Changes in `crates/promptforge-internal/lua/src/`, as Technical Design's Phase 0 bullets declare:
  - `__impl_coro.lua`:
    - the first statement (lines 27 to 29) gains the `is_model_handle` capture after `is_message_list`, and the header (lines 1 to 26) says what it reports;
    - `infer` (lines 110 to 122) splits into `run_infer(handle, prompt)`, which keeps today's yield and failure path, and two entries: `infer(...)`, still installed as `models.infer`, and `handle_infer(...)`. The comment at lines 106 to 109 says handles carry `infer` and `loop`;
    - `models_loop` (lines 265 to 324) splits into `run_loop(handle, messages, compactor)`, which keeps the compactor default and check (lines 280 to 284), then the new list check, then the round loop (lines 287 to 323), and the entries `models_loop(...)` and `handle_loop(...)`, which hold their entry's handle check and arity check. The guessing rule (lines 267 to 269) goes, and the comment at lines 241 to 264 describes the two entries;
    - each entry raises its errors as `raise("lua", { message = ... })` before any yield, in this order: the handle check, then the arity check counting trailing nils, then for the loop entries the compactor check and the list check;
    - the chunk local `NOT_A_LIST` copies the parse's text (`protocol/parse/chat.rs`, lines 16 and 17) under a comment naming that source, and the list check is `if not is_message_list(messages) then raise("lua", { message = NOT_A_LIST }) end`;
    - the drain guard in `drain_task_notices` (line 211) and the comment sentences that explain it (lines 207 to 209) go, since only a list reaches the drain;
    - the return table (lines 411 to 433) gains `handle_methods = { infer = handle_infer, loop = handle_loop }`.
  - `coro.rs`: `install_shim_prelude` builds `is_model_handle` over `models::is_handle` beside `is_message_list` (lines 186 to 188), passes it after `is_message_list` in the chunk call (lines 190 to 206), and stashes the chunk's `handle_methods` table under one new private registry key, `HANDLE_METHODS_REGISTRY`, beside `LOOP_REGISTRY` (line 62). Add `pub(crate) fn handle_method(lua: &Lua, name: &str) -> mlua::Result<Value>` with the doc Technical Design declares. The doc of `install_shim_prelude` (lines 120 to 160) names the new capture and the stash.
  - `models-userdata.rs`: `add_fields` (lines 107 to 120) gains the `infer` and `loop` field getters over `crate::coro::handle_method`, and `pub(crate) fn is_handle(value: &Value) -> bool` joins `LuaModelHandle`, both as Technical Design declares. The module doc (lines 1 to 7) says the handle carries `loop` and `infer` and why they are Lua functions: each may suspend, and a Rust method cannot.
  - `models.rs`: re-export `is_handle` beside `LuaModelHandle` (line 27), as `messages::is_list` is for lists.
- Comments that say invocation is namespace-only or show a handle-first call, rewritten for the methods: `messages.rs` lines 3 to 6, `vm/install.rs` lines 54 to 56, `protocol.rs` line 4 in both the lua crate and `engine/src/execute/protocol.rs`, `protocol/request.rs` line 27, `protocol/parse.rs` lines 243 to 244 and 257, `tools/userdata.rs` line 8 (tool handles stay methodless, so only the cross-reference changes), `engine/src/execute/tools.rs` line 4, and `engine/src/execute/config.rs` line 186; also `protocol/parse/chat.rs` lines 31 to 40 (the `parse_chat` doc's leading-argument wording), `engine/src/execute/tests/model_and_reply.rs` line 3, and the module doc of `engine/src/lua/tests/shims.rs` at lines 4 and 5.
- Stale rules deleted whole, not reworded, because the methods contradict them: the methodless-handle bullet of `crates/promptforge-internal/lua/AGENTS.md` (line 5), and the same rule's bullet in the `## Invariants` list of `crates/promptforge-internal/lua/src/lib.rs` (lines 51 to 56). The Invariants list keeps its other bullets and the marker that `cargo test -p build-xtask` requires, and `crates/workshop/ui/test/docs-claims.mjs` checks only the defined terms, so neither gate is affected.
- Call sites migrated, each keeping its assertions:
  - to `h:loop`: `crates/harness/tests/suite/host.rs` line 55; `crates/workshop/server/tests/it/agents.rs` line 54 and `agents/revoke.rs` lines 29 and 33; `execute/tests/models_loop.rs` lines 347 and 349; `execute/tests/models_loop-arguments.rs` lines 27, 29, 70, and 111; `execute/tests/models_loop_compactors.rs` line 118, a call of the `refusal(...)` helper (defined at lines 109 to 114), which passes its arguments to `pcall(models.loop, ...)`;
  - to `h:infer`: `crates/harness/tests/suite/host.rs` line 43, `pcall(models.infer, writer, ...)`, which becomes `pcall(writer.infer, writer, ...)`; `engine/src/lua/tests/shims.rs` lines 368 and 387; `engine/src/lua/tests/errors.rs` line 174, whose assertion at line 184 then expects `handle:infer takes (prompt)`, and the comment at line 24; `errors.rs` lines 29 and 48, both `pcall(models.infer, models.get("fast"), "yo", { temperature = 0 })`: the first becomes `pcall(models.infer, "yo", { temperature = 0 })` and its assertion at line 31 expects `models.infer takes (prompt)`, with the test renamed to match, and the second binds `local h = models.get("fast")`, calls `pcall(h.infer, h, "yo", { temperature = 0 })`, and its assertion at line 54 expects `table|lua|handle:infer takes (prompt)`; `execute/tests/debug_and_counts.rs` lines 65 and 307 and the comment at 299; `debug_and_counts-infer-rounds.rs` line 93; `live_infer.rs` lines 13, 39, 253, 307, and 357; `scheduler/live_h1/pass.rs` line 18; `exec_flow.rs` lines 178, 222, and 243 and the comments at 169 and 211; `model_and_reply-handles.rs` lines 127 and 224.
  - Every prompt in `prompts/` and the Workshop chat agent (`crates/workshop/agents/agents/chat.md`, line 38) already use the list-first and prompt-first forms, so they need no change.
- Tests, from Testing Plan, verbatim:
  - `execute/tests/models_loop-arguments.rs` (146 lines today): `every_argument_form_resolves_the_list` runs `models.loop(msgs)`, `models.loop(msgs, compactors.fail)`, `other:loop(msgs)`, and `other:loop(msgs, compactors.fail)`, with its model and size assertions unchanged; the two plain-array tests call `other:loop(plain)` in place of `models.loop(models.get('other'), plain)`, with their texts unchanged. New cases: `models.loop(other, msgs)` raises the pointed error before any request; `other.loop(msgs)` raises the colon error; arity in both entries, one case with a trailing nil, checking `kind` and `tostring(err)`; a non-list in both entries raises before any request. If the file would pass 450 lines, the error cases move to a flat peer module registered in `execute/tests.rs`, as `models_loop_compactors.rs` is.
  - `execute/tests/models_loop_compactors.rs`: the compactor-type test (lines 98 to 146) keeps both entries, `models.loop(msgs, bad)` and `writer:loop(msgs, bad)`, with the same expected message for each.
  - `engine/src/lua/tests/errors.rs` and `shims.rs`: `h:infer(prompt)` yields an `infer` request whose handle is `h`, as `models.infer(h, prompt)` did; `models.infer(h, prompt)` raises the pointed error; `h.infer(prompt)` raises the colon error; `h:infer('a', 'b')` raises `handle:infer takes (prompt)`, replacing the three-argument case at `errors.rs` line 174; the two `pcall(models.infer, handle, ...)` cases at `errors.rs` lines 29 and 48 and `handles_reject_colon_methods` in `shims.rs` change as Technical Design lists.
  - `handles_reject_colon_methods` in `engine/src/lua/tests/shims.rs` (lines 399 to 418), which asserts that reading `infer` off a model handle fails, becomes a test that `type(h.infer)` and `type(h.loop)` are `"function"` on a model handle and that reading a field the handle does not have still fails.
  - Every migrated call site keeps its assertions.
- Size: estimates are `coro.rs` from 459 to about 471 lines, `models-userdata.rs` from 121 to about 130, and `__impl_coro.lua` from 433 to about 465. `engine/src/lua/tests/shims.rs` (418 lines) and `errors.rs` (387 lines) stay under 500.
- Done when: an `rg` for `models.loop(`, `models.infer(`, `pcall(models.loop,`, and `pcall(models.infer,` under `crates/` and `prompts/` finds every call starting with the list or the prompt, apart from the new test cases that pass a handle first on purpose to assert the pointed error; and every existing test passes after the migration.
- Verify: the Project Survey's commands at the run's chosen scope: the full-suite test command, which includes `cargo test -p build-xtask`; the linter command with the headless `cargo check -p gateway --no-default-features`; the formatter check; the workspace and facade docs builds; and the facade surface check `cargo +nightly-2026-09-05 xtask api --check`, which passes with no diff.
- Commit: `Give model handles loop and infer methods`.

</step-1>

<step-2>

### Step 2: Describe handle methods in the guide [completed]

- Component: Handle methods

- Piece: the guide, the component's second piece. It depends only on Step 1, whose forms and texts it describes.
- Repository: the separate repository `c:\Users\Vinnie\cursor\promptforge-docs`. This step runs as a vibe step across two repositories:
  - The coding, review, fix, and commit-message dispatches for this step name `c:\Users\Vinnie\cursor\promptforge-docs` as the repository and the active plan by its absolute path.
  - Its provisional `[WIP] Step 2:` commit, every fix amended into it, and its final message live in `promptforge-docs`. On resume, a `[WIP] Step 2:` commit at the `promptforge-docs` HEAD is discarded under the same rules as one in `promptforge` before the step reruns.
  - Its plan marker is committed in `promptforge` as a commit of its own that holds only the marker, with subject `Record the guide update for handle methods`, a body that names the `promptforge-docs` commit hash, and the `Plan:` trailer.
- House rules, from `promptforge-docs/CONTRIBUTING.md`: no em-dash or double-dash, four-backtick code fences, and one line per paragraph.
- What the guide says, from Functional Specification, verbatim:
  - The forms:
    - `models.infer(prompt)` runs one round on the section's current model, as the one-argument form does today.
    - `h:infer(prompt)` runs one round on `h`'s frozen binding, as `models.infer(h, prompt)` does today.
    - `models.loop(messages, compactor?)` runs on the section's current model, as today's list-first form does.
    - `h:loop(messages, compactor?)` runs every round on `h`'s frozen binding, as `models.loop(h, messages, compactor?)` does today.
  - A handle keeps its fields, and `type(h)` stays `'userdata'`. Tool handles keep their current form.
  - The argument errors, each a `lua`-kind error table raised before any yield:
    - `models.infer`: a model handle first raises `models.infer takes (prompt); call handle:infer(prompt) to run on a model handle`; more than one argument otherwise raises `models.infer takes (prompt)`.
    - `h:infer`: a first argument other than a model handle, which is a call written with a dot or the function taken off the handle and called bare, raises `call infer on a model handle with a colon: handle:infer(prompt)`; more than two arguments, the handle included, raise `handle:infer takes (prompt)`.
    - `models.loop`: a model handle first raises `models.loop takes (messages, compactor?); call handle:loop(messages, compactor?) to run on a model handle`; more than two arguments otherwise, trailing nils counted, raise `models.loop takes (messages, compactor?)`.
    - `h:loop`: a first argument other than a model handle raises `call loop on a model handle with a colon: handle:loop(messages, compactor?)`; more than three arguments, the handle included and trailing nils counted, raise `handle:loop takes (messages, compactor?)`.
    - The compactor, second for `models.loop` and third for `h:loop`: nil selects `compactors.fail`, read through an ordinary index of the `compactors` table captured at install and not checked; a function is used as given; anything else raises `compactor must be a function, got {type}`, where `{type}` is `integer` for an integer and Lua's `type()` name otherwise, so a full or a light userdata reads `userdata`.
    - The list, first for `models.loop` and second for `h:loop`: a `messages.new()` list passes; anything else raises the parse's list text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace`.
  - Emptiness: `messages must not be empty` stays the chat parse's check at every round, after that round's drain, so pending notices can fill an empty list and `h:loop(messages.new())` runs one round over them.
- Changes for the handle methods, in `src/language/`, each passage found by its content:
  - `05-lua-environment.md` line 208, which spells out the optional-leading-handle API and its `handle?` error text;
  - `10-models.md` lines 232 to 297, 409, 416, and 429, the last an error bullet with the `handle?` text. Line 232 states the rule that model handles have no methods, which the methods overturn for model handles only; tool handles keep it, and the message list stays the other object with methods;
  - `11-conversations.md` lines 221, 250, 255, 416, and 564, the last the `handle?` loop error text. Line 255 has said "a userdata first argument is taken as the handle" since before lists became userdata;
  - `17-quick-reference.md` line 69, which says a model handle has no methods, and lines 74 to 76.
- Changes from the messages refactor's open follow-up (`c:\Users\Vinnie\cursor\promptforge\vibe\2026-10-09-2-messages-userdata-refactor.md`, lines 21 and 779 to 795, whose Functional Specification is the source for these behaviors), restated verbatim:
  - `src/language/11-conversations.md`:
    - lines 36 and 52: the list is a `messages.new()` list owned by the Engine, not a plain Lua table;
    - line 118: `models.loop` refuses a hand-written array, with its error text, and `list:append` validates the record and drops fields beyond the four instead of appending it unchanged;
    - line 173: extra fields are dropped when the record is added, so they cannot be read back from the list;
    - line 217: `msgs[#msgs] = nil` becomes `msgs:replace(#msgs, #msgs)`;
    - line 307: the late-system error is raised by the edit itself;
    - the Checking the list section (lines 315 to 382): per-record checks and system placement run at the edit, the rows for the list as a whole (lines 336 to 342) change because a plain table is refused outright, and cross-record checks such as tool-call pairing still run at the `models.loop` call;
    - a passage on `replace(first, last, records...)` and its bounds, and on record views: a record read from a list is read-only, its `type()` is `userdata`, and changes go through `replace`.
  - `src/language/17-quick-reference.md`, the `messages` table (lines 83 to 97): add `list:replace`, `#list`, and `list[i]` rows; `list:append` validates the record and drops extra fields instead of appending it unchanged; the `record.*` rows read a read-only view.
  - Also, as Technical Design adds, chapter 11 line 129 and the list's `pairs` behavior, which commit `b5ea5d9b4` set: a `pairs` loop over a list stops at the record count it started with, so a loop that appends to the list ends, while `ipairs` reads the live list, so the two visit different records when the loop edits the list.
- Tests: the books build, run as written: `Push-Location c:\Users\Vinnie\cursor\promptforge; $env:PROMPTFORGE_DOCS = 'c:\Users\Vinnie\cursor\promptforge-docs'; cargo xtask site --books-only; Pop-Location`.
- Verify: the books build above rebuilds cleanly (`promptforge-docs/CONTRIBUTING.md`, line 7). The `promptforge` code is unchanged since Step 1, so the Project Survey's commands are not rerun for this step, and the marker commit changes only the plan.
- Commit, in `promptforge-docs`: `Describe handle methods and the Rust-backed messages list`.

</step-2>

<step-3>

### Step 3: Pin the loop's behavior with contract tests [completed]

- Component: Loop port

- Piece: contract tests, the first of the component's two sequential pieces. They pin the shim as Step 1 leaves it, before any port code exists, and Step 4 passes them unchanged.
- Rules for every test here, from Testing Plan and Execution Instructions, verbatim:
  - The end-to-end ones go in `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`, driven with `TokioDriver` over `ScriptedChat` or `OverflowClient`, or with the serial `Run` for the dropped-call case, reusing the loop fixtures in `execute/tests/models_loop.rs` (lines 29 to 125) and the model-task fixtures in `execute/tests/model_tasks.rs` and `model_task_notices.rs`. The one shape no real round produces, an answer with no empty detail, goes in `crates/promptforge-internal/engine/src/lua/tests/models_loop_contract.rs`, with canned answers rendered through `Answer::into_envelope`. Every one of them uses only `messages.new()` lists and their builders and asserts only what an author or the caller sees: the list, the returned or raised value, the effects, and the events. Phase 1 keeps every argument behavior, so every characterization case passes on both sides of it.
  - A light userdata for the compactor gap comes from a JSON null in a structured tool's result (`StructuredFixtureTool`, `execute/tests/fixtures.rs`, lines 83 to 94), which mlua turns into a null light userdata.
  - Through the public Lua surface only. A failing test is a wrong test, unless the shim contradicts the guide (the chapters in `c:\Users\Vinnie\cursor\promptforge-docs\src\language\`, as Step 2 left them), which is reported rather than fixed: the test pins what the shim does, neither the shim nor the guide changes, and the step's report names the contradiction for the owner.
- Changes:
  - New `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`, registered in `execute/tests.rs` with `mod models_loop_contract;` beside `mod models_loop;` and `mod models_loop_compactors;` (lines 395 and 396). It is a flat peer rather than a third `models_loop-*` sibling of `execute/tests/models_loop.rs`, which already has two (lines 20 to 24), because a third would force the group into a `models_loop/` directory (`AGENTS.md`, line 63).
  - New `crates/promptforge-internal/engine/src/lua/tests/models_loop_contract.rs`, registered in `engine/src/lua/tests.rs` (lines 15 to 19).
  - The fixtures are already `pub(super)`: `loop_models`, `loop_context`, `loop_context_observed`, `always_tool`, `echo_tools`, `loop_events`, and `loop_prompt` in `execute/tests/models_loop.rs`, imported as `models_loop_compactors.rs` does with `use super::models_loop::{...}`; `task`, `model_task_context_with`, `owner_prompt`, and `PARKED_CHILD` in `model_tasks.rs`; and `loop_owner` and its recorder in `model_task_notices.rs`.
  - Each new file stays under 400 lines. If the Engine file would pass that, one group of its cases moves to a dash-labeled sibling, `models_loop_contract-<label>.rs`, wired from it with `#[path]`, which keeps the flat-directory rule.
- Tests: the 18 gap tests, each with the inventory behavior it pins, from Testing Plan, verbatim. Line numbers in this step, `__impl_coro.lua`'s included, are `master`'s at `b5ea5d9b4`, before Step 1, which moved the loop's body into `run_loop` and kept the rules below the argument checks as they are. Tests 1 to 17 go in the Engine file and test 18 in the Lua-layer file.
  1. The default compactor. Behavior: a nil compactor selects `compactors.fail`, read from the captured `compactors` table at each call and never checked (lines 280 and 281). Already pinned: `execute/tests/models_loop_compactors.rs`, lines 13 to 43 and 67 to 97. Gap test: an author who replaces `compactors.fail` changes the default, and a non-function put there fails only at an overflow round, with Lua's own call error raised as a string.
  2. A bad compactor. Behavior: a non-function raises `compactor must be a function, got {type}`, with `integer`, `number`, and otherwise Lua's `type()` name (lines 282 to 284 and 44 to 47). Already pinned: `models_loop_compactors.rs`, lines 98 to 146, in both entries after Phase 0. Gap test: a light userdata in `h:loop`'s compactor position reads `got userdata`, Lua's name, rather than mlua's `lightuserdata`.
  3. A bad list. Behavior: since Phase 0, anything but a `messages.new()` list raises the parse's `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace` before any yield, in both entries, so the call raises before any request and a pending notice waits for the next valid call. The parse keeps its own check (`protocol/parse/chat.rs`, lines 43 to 51), unreachable through the API. Already pinned: `models_loop-arguments.rs`, lines 63 to 146, for a plain array in both entries, a record view, and a waiting notice, as Step 1 migrated them. Gap test: `42`, no argument at all, and `other:loop(other)`.
  4. An empty list. Behavior: the chat parse raises `messages must not be empty` at each round, after that round's drain, so pending notices can fill an empty list (`protocol/parse/chat.rs`, lines 52 to 54). Already pinned: `protocol/tests/parse_chat.rs`, lines 190 to 196, through a hand-built table only. Gap test: an empty list with nothing pending raises it through `models.loop`, and an empty list with one pending notice runs one round over that notice.
  5. An uncaught argument error. Behavior: a `lua`-kind raise maps to `Error::LuaRuntime` whose text names the author's prompt line, because it is a Lua `error` above the author's frame (`vm/run.rs`, lines 283 to 293). Already pinned: for `models.infer` only (`engine/src/lua/tests/errors.rs`, lines 165 to 194). Gap test: an uncaught `models.loop` arity error.
  6. The drain. Behavior: before every round, the first included: one `drain_task_notices` yield, then one user record per notice in queue order, then the `chat` yield (lines 210 to 217 and 289 to 290; `scheduler/notices.rs`, lines 77 to 93). Already pinned: the yield order in `engine/src/lua/tests/shims.rs`, lines 252 to 308, and in `quota.rs`; one notice in `execute/tests/model_task_notices.rs`, lines 147 to 202. Gap test: two notices drained into one round land as two user records, in the order their tasks ended.
  7. The reply. Behavior: a reply appends `{ role = "assistant", content = reply }` and returns nil (lines 310 to 312). Already pinned: `execute/tests/models_loop.rs`, lines 126 to 157; `execute/tests/exit_rules.rs`, lines 193 to 224; `chat_arm.rs`, lines 144 to 191. Gap test: the call returns exactly one value.
  8. The clean exit, counting a failed tool. Behavior: no reply, no calls, `finish_reason` `"stop"`, and at least one answered call in this loop call append `{ role = "assistant", content = "" }` and return nil (lines 313 to 315). A batch adds all its calls to the count, failed tools included (line 309), and the count starts at zero for each call (line 287). Already pinned: `exit_rules.rs`, lines 226 to 253 for the clean exit, and lines 255 to 278 and 297 to 325 for the cases that fall outside it. Gap test: a failing bound tool still counts.
  9. The clean exit, counting per call. Behavior: as in test 8. Gap test: a second `models.loop` whose first round is an empty `"stop"` raises, even though the list already holds tool records from the first.
  10. The batch rule. Behavior: results are buffered, and the assistant record and the tool records are appended only once every call has its result (lines 295 to 309). Already pinned: `models_loop.rs`, lines 371 to 421, where handlers mid-batch see no partial batch; `models_loop.rs` lines 158 to 203 and `tool_loop.rs` lines 69 to 93 pin only that whole exchanges are appended round by round. Gap test: a handler that raises its own table on the second call of a batch, under `pcall(models.loop, msgs)`, leaves only the user record in the list, and `err` is that same table. It also covers the raised-handler gap: `local_tool_done` reports the failure, so `TOOL_CALL_FAILED` fires and `ToolResult` stays silent, and then the handler's own value is raised unchanged, before the answer is looked at (lines 153 and 154), whose identity through `models.loop` no test pinned.
  11. A failed tool answer. Behavior: a failed `tool_call` answer raises its error at the call site and appends nothing (line 163). Already pinned: for a script call only, `run/tests-drops.rs`, lines 206 to 225. Gap test, driven with the serial `Run`: a dropped model-issued call raises `cancelled` at the `models.loop` call site, and the list holds only the user record.
  12. A local tool's return. Behavior: the first return value under the scalar-return rule, with nil read as `""` (`protocol/parse.rs`, lines 415 to 428). Already pinned: string returns. Gap test: a handler that returns nothing, a number, or two values gives `""`, the number's text, or the first value.
  13. A local tool under cancellation. Behavior: a handler that fails under cancellation has its raw value raised at once, with no `local_tool_done` yield (line 149). Already pinned: `local_tools.rs`, lines 141 to 171, where the run ends `Interrupted`. Gap test: that call reports no `TOOL_CALL_FAILED`.
  14. The compactor call. Behavior: `raw_pcall(compactor, overflow_reason)`, inside the block, so the compactor may yield (line 230). Already pinned: `chat_arm.rs`, lines 382 to 449, for the reason tags. Gap test: the compactor gets exactly one argument, and one that calls `models.infer` before raising suspends and resumes normally.
  15. A returning compactor. Behavior: it raises `the selected compactor returned without raising: replacement compactors are deferred; compactors.fail is the only shipped policy` (lines 231 to 236). Already pinned: `models_loop_compactors.rs`, lines 147 to 186, by substring only. Gap test: the exact text.
  16. A raising compactor. Behavior: under cancellation the raw value (line 237); otherwise `normalize_failure(value)` (line 238). Already pinned: `models_loop_compactors.rs`, lines 13 to 66 for typed exhaustion from `compactors.fail`, lines 187 to 221 for cancellation, and lines 222 to 253 for a string passing through. Gap test: an author's own table passes through as the same table.
  17. Events come from the scheduler. Behavior: the scheduler reports every round and call, and the shim reports none of its own (`scheduler/chat.rs`, lines 11 to 19). Already pinned in part by the filtered sequences in `exit_rules.rs` and `tool_loop.rs`. Gap test: the full ordered trace of observations and content reports for one bound-tool round, one local-tool round, and a reply.
  18. The empty raise, in `engine/src/lua/tests/models_loop_contract.rs`. Behavior: otherwise `empty_model_reply`, with the answer's `empty_detail`, else `empty model reply`, as its message, and `finish_reason` when present (lines 316 to 321 and 221). Already pinned: `exit_rules.rs`, lines 280 to 295 and 327 to 381; `chat_arm.rs`, lines 346 to 380; the kind mapping in `errors.rs`, lines 304 to 323. Gap test: the fallback text, which only a rendered answer with no empty detail produces, since the model client refuses an empty text first. It drives the loop's yields by hand, a `drain_task_notices` step and then the `chat` step, with each answer rendered through `Answer::into_envelope`.
- Done when: all 18 pass on Step 1's shim, and every existing test still passes.
- Verify: the Project Survey's commands at the run's chosen scope: the full-suite test command, which includes `cargo test -p build-xtask`; the linter command with the headless `cargo check -p gateway --no-default-features`; the formatter check; the workspace and facade docs builds; and the facade surface check `cargo +nightly-2026-09-05 xtask api --check`, which passes with no diff.
- Commit: `Pin models.loop behavior with contract tests`.

</step-3>

<step-4>

### Step 4: Run the models.loop rules in a Rust state machine

- Component: Loop port

- Piece: machine and trampoline, the component's second piece. It needs Step 3's tests and passes them unchanged. The new module, the chunk's new argument, and the trampoline are built jointly, because they only work together. Line numbers are `master`'s at `b5ea5d9b4`, before Step 1, unless this step says otherwise; earlier steps moved code, so find each cited passage by its content.
- Rules it implements, from Functional Specification, verbatim:
  - From Phase 1, both loop entries are thin Lua functions over one trampoline. Each hands its arguments to `loop_begin` behind a flag that names the entry, then the trampoline performs each action the Rust step returns: yield a request and pass the resume values back, call a handler or the compactor under the raw `pcall` and pass the outcome back, raise a value, or return nil.
  - The scheduler receives `drain_task_notices`, `chat`, `tool_call`, and `local_tool_done` in the same order and shapes through Phase 0 and Phase 1, until Phase 2 changes them. Each method sets the request's `handle` from its receiver, exactly as the handle-first call did, so every effect and event stays the same.
  - The loop entries' arguments, checked before any yield in this order, each failure raised as a `lua`-kind error table, from Phase 1 in Rust with the Phase 0 shim's texts:
    - `models.loop`: a model handle first raises `models.loop takes (messages, compactor?); call handle:loop(messages, compactor?) to run on a model handle`; more than two arguments otherwise, trailing nils counted, raise `models.loop takes (messages, compactor?)`.
    - `h:loop`: a first argument other than a model handle raises `call loop on a model handle with a colon: handle:loop(messages, compactor?)`; more than three arguments, the handle included and trailing nils counted, raise `handle:loop takes (messages, compactor?)`.
    - Both phases check model handles with `models::is_handle`, an exact `is::<LuaModelHandle>()` test that Phase 0 adds.
    - The compactor, second for `models.loop` and third for `h:loop`: nil selects `compactors.fail`, read through an ordinary index of the `compactors` table captured at install and not checked; a function is used as given; anything else raises `compactor must be a function, got {type}`, where `{type}` is `integer` for an integer and Lua's `type()` name otherwise, so a full or a light userdata reads `userdata`.
    - The list, first for `models.loop` and second for `h:loop`: a `messages.new()` list passes; anything else raises the parse's list text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace` (`protocol/parse/chat.rs`, lines 16 and 17).
  - The return value of either loop entry is exactly one nil.
  - Records the loop adds, each a `MessageRecord` pushed onto the list in Rust from Phase 1:
    - each drained notice: `{ role = "user", content = notice }`;
    - a finished batch: one `{ role = "assistant", content = "", tool_calls = { { id, name, arguments }, ... } }`, then one `{ role = "tool", content = result, tool_call_id = id }` per call, in call order;
    - a reply: `{ role = "assistant", content = reply }`;
    - the clean exit: `{ role = "assistant", content = "" }`.
  - Requests yielded in Phase 1, built in Rust exactly as the shim builds them:
    - `{ op = "drain_task_notices" }`;
    - `{ op = "chat", messages = list, handle = handle }`, with `handle` the receiver of `h:loop` and absent for `models.loop`;
    - `{ op = "tool_call", alias = name, args = arguments, call_id = id, turn = turn }`, with the call's arguments as a table converted from the call's JSON, which the parse reads back to the same value the chat answer's table held, and the round's `turn`;
    - `{ op = "local_tool_done", ok = ok, value = value }`, with `value` set only when the handler returned.
  - States and validation:
    - Rounds: at most `max_tool_iterations` rounds, the terminal one included. Each starts with the drain, then the `chat` yield. After the last allowed round's batch is appended, the loop raises `tool_loop_exhausted` instead of starting another round.
    - Emptiness: `messages must not be empty` stays the chat parse's check at every round, after that round's drain, so pending notices can fill an empty list and `h:loop(messages.new())` runs one round over them.
    - A round's answer is judged in this order: a failed answer raises its value; an overflow goes to the compactor; tool calls start a batch; a reply is appended and the loop returns; an empty reply whose `finish_reason` is `"stop"`, after at least one answered call in this loop call, appends the empty record and returns; anything else raises `empty_model_reply`.
    - The answered count includes every call of every finished batch in this loop call, a tool's own failure included, and starts at zero for each call.
    - A batch dispatches its calls one at a time, in order, and buffers each result; its records are appended only once every call has its result.
    - A local tool's call runs its handler between the depth captures, reports the outcome with `local_tool_done`, and then raises the handler's own value if the handler raised, raises the answer's error if the answer failed, and otherwise takes the answer's text as the result.
    - The compactor runs under the raw `pcall` with the overflow reason as its only argument. A compactor that returns raises the deferred-replacement error.
  - Errors and recovery:
    - Every raise happens in a Lua frame as `error(value, 0)`, from Phase 1 the trampoline's, with the value the Phase 0 shim raises. So `tostring(err)` is the message, the block guard's handler stashes the same value through `stash_failure` (`coro.rs`, lines 272 to 284), and a failed answer's retained typed error is substituted as it is today (`vm/run.rs`, lines 226 to 243 and 315 to 328).
    - The values raised: a failed answer's error table, unchanged; a handler's own raised value, unchanged; under cancellation, a failed handler's or compactor's raw value; any other compactor failure through `normalize_failure`; new error tables for `tool_loop_exhausted` (`tool-call loop did not converge`, no fields), `empty_model_reply` (the answer's empty detail, else `empty model reply`, with `finish_reason` when the answer has one), the deferred compactor (`the selected compactor returned without raising: replacement compactors are deferred; compactors.fail is the only shipped policy`), and the argument errors.
    - Phase 1 keeps every raised value and every raise point: the argument texts and their timing are Phase 0's.
  - Security and privacy behavior:
    - No trust rule moves. The Engine wraps untrusted tool output before it answers (`scheduler/apply.rs`, lines 170 to 189), a handler's text stays trusted, and a notice's task result stays in the untrusted envelope (`scheduler/notices.rs`, lines 60 to 70).
    - `loop_begin` is reachable only as the shim chunk's upvalue, and each call's step closure only as the trampoline's local; the call's state lives inside that closure. The sandbox never loads `debug` (`hardening.rs`, lines 12 to 14), so no author code can call a step or reach a state.
- Success criteria it meets, from Product Requirements, verbatim:
  - Every existing test passes unchanged after Phase 1, except `crates/promptforge-internal/engine/src/lua/tests/quota.rs`, which measures the shim's own Lua instructions and is rewritten as Testing Plan describes.
  - In Phase 1 the scheduler receives exactly the requests it receives after Phase 0, in the same order and shapes, and resumes them with the answers it renders today.
  - The trampoline holds no rule of the loop: it only yields, calls, raises, or returns as the Rust step says.
  - Every rule, including the three that no prompt can observe, has a unit test that runs without a Lua VM.
  - Every record the loop adds reaches the list through `MessageList::push` in Rust.
- Changes, each as Technical Design's Phase 1 declarations and bullets give it:
  - New in `crates/promptforge-internal/lua/src/`:
    - `models_loop.rs`, the adapter: its module doc; the constants `HANDLE_FIRST`, `LOOP_ARITY`, `NO_RECEIVER`, and `METHOD_ARITY`; `pub(crate) fn loop_begin(lua, max_tool_iterations, compactors, budget)`; the private `begin`, `read_input`, `act`, `envelope_failure`, and `lua_type_name`; `#[path = "models_loop-machine.rs"] mod machine;`; and the `#[cfg(test)]` `#[path = "models_loop-tests.rs"] mod tests;`. The step closure comes from `create_function_mut`, owns the call's `Machine`, and must be `Send` under mlua's `send` feature. Every loop error is a `"raise"` action, never an `Err`.
    - `models_loop-machine.rs`, the machine: `EMPTY_MODEL_REPLY`, `Machine<V>`, `Phase<V>`, `Input<V>`, and `Then<V>`, with `Machine::begin`, `phase`, and `step` for `V: Clone`. It reuses `ChatResult`, `ToolCallEvent`, `ToolCallRecord`, `OverflowReason`, `Raised`, `MessageList`, `MessageRecord`, `MessageContent`, and `MessageRole`, declares no other type, needs no VM, and returns `Then::RaiseNew` with an `internal`-kind `Raised` for an input other than the one its phase names.
    - `models_loop-tests.rs`, the tests below.
  - Edited in `crates/promptforge-internal/lua/src/`:
    - `lib.rs`: `mod models_loop;` beside `mod models;` (line 149).
    - `coro.rs`: `install_shim_prelude` keeps its `compactors` read (line 176), drops the `is_message_list` capture and its construction (lines 186 to 188), builds `loop_begin` from `crate::models_loop::loop_begin(lua, max_tool_iterations, compactors, instruction_budget)`, and calls the chunk with the argument list Technical Design gives, ending in `is_model_handle, loop_begin`. Its doc says the round cap and the `compactors` table now go to `loop_begin`, and the doc of `MODEL_TOOL_CALL_REGISTRY` (lines 64 to 70) drops "in production the loop shim reaches the function directly".
    - `__impl_coro.lua`: the first statement becomes Technical Design's capture list ending in `cancel_requested, is_model_handle, loop_begin`; `drive`, `models_loop`, and `handle_loop` become Technical Design's Lua, each entry a tail call `return drive(loop_begin(false, ...))` or `return drive(loop_begin(true, ...))`. Removed: `append_record`, `drain_task_notices`, `EMPTY_MODEL_REPLY`, `NOT_A_LIST`, `compact`, and Step 1's `run_loop`, with their comments. Kept: `raise`, `fail`, and `engine_type`, which the `tasks` and `fanout` chunks use through `helpers` (line 416); `run_local_tool`, `dispatch_tool`, and `tools_call`, which `tools.call` uses; `tools_call_as_model`, which only the test-only `tools.call_as_model` hook uses after the port; and Step 1's `infer`, `handle_infer`, and `run_infer`. The return table and the registry stashes stay as Step 1 leaves them. Comments: `drive`'s says the loop's rules run in Rust behind `loop_begin` and the step closure it returns, that it only performs the action each step returns, and why; the entries' says each names itself to `loop_begin`, so the checks know the form without inspecting the arguments; the chunk header (lines 1 to 26) drops `compactors`, `max_tool_iterations`, and `is_message_list` from its captures and adds `loop_begin`; `tools_call_as_model`'s comment (lines 177 to 185) says only the test-only hook calls it now.
    - `error-value.rs`: `pub(crate) fn normalized(lua: &Lua, value: Value) -> mlua::Result<Value>` beside `install_normalize_failure`, which becomes `lua.create_function(normalized)`; no behavior change.
    - `protocol/parse/chat.rs`: `NOT_A_LIST` (lines 16 and 17) becomes `pub(crate)` with the doc Technical Design gives, re-exported through `protocol/parse.rs` and from `protocol.rs` as `pub(crate)`; no behavior change.
  - Edited elsewhere: `crates/promptforge-internal/engine/src/lua/tests/quota.rs`, rewritten as Tests says; the doc comment of `crates/promptforge-internal/engine/benches/models_loop.rs` (lines 1 to 6), which says the loop runs inside `__impl_coro.lua`, says its rules run in Rust behind the trampoline.
  - Exactness: follow Technical Design's "Exactness of the port, helper by helper" list for each shim helper the port replaces: the entry checks, `raise`, `fail`, `normalize_failure`, `cancel_requested()`, `run_local_tool`, `dispatch_tool`, `compact`, the block guard and retained errors, every Rust frame returning before a suspension, and the globals the trampoline still reads.
  - Unchanged: the scheduler, the protocol parse, dispatch, and answer rendering, apart from the shared helper and constant above; the guide, because Phase 1 is invisible to authors. "The loop shim" keeps naming `models.loop`'s implementation, so only comments that say a rule runs in Lua change.
- Tests, from Testing Plan, verbatim:
  - `models_loop-tests.rs`, machine tests with a plain test type for `V`, a `MessageList::default()` read back through `records()`, and no VM: every `Phase` transition and every `Then`; the records each step pushes; the order a round's answer is judged in; the answered count across batches and its reset per call; the cap raised after the last allowed batch and before another drain; the empty-detail fallback; a local call's three outcomes and the re-raise order; the cancel branches for handlers and compactors, including a returning compactor winning over cancellation; an input of the wrong kind raising `internal`. Three of these rules are invisible to a prompt, because every cancelled path ends the run as `Interrupted` (`vm/run.rs`, lines 298 to 304) and a drain reports nothing: the cap before another drain, the compactor's cancel branch, and its returned-before-cancel order. These unit tests are their only pin.
  - `models_loop-tests.rs`, adapter tests with a VM: `lua_type_name` for every `Value` kind against Lua's own `type`; both entries' argument checks, their order, and their texts; `envelope_failure` for an error table and for a string; the chat-table reader against the table `Answer::into_envelope` renders for an overflow, a reply, an empty round, and a batch, matching every field the loop reads; each `Then`'s action tag and values from `act`.
  - `quota.rs` (lines 77 to 172) is the only test that depends on Lua instruction counts or hooks spent inside the shim. Its counting hook will see only the trampoline: by estimate about nine instructions per yield, so about 27 for its measured round of three yields, which is under the 300 ceiling and just over the 20 floor. Its doc (lines 1 to 6 and 20 to 37) says that every instruction the shim spends per round is counted and that a round under the floor means "the loop runs outside the shim", which is now true by design. It is rewritten: the doc says it bounds the trampoline's cost; the floor becomes one instruction per yield in the span (three), which only proves the hook fires on the loop thread; and the ceiling is re-measured and set at about three times the measured cost, the way the current ceiling was set (86 measured, 300 allowed; `quota.rs`, lines 21 to 27).
  - Every other hook test loops in author code rather than counting shim instructions, and passes unchanged: `engine/src/lua/tests/coroutine.rs`, `lua/src/tests/cancellation.rs`, and `lua/src/tests/budgets.rs`.
  - Every Step 1 and Step 3 test, and every other existing test, passes unchanged.
  - The `models_loop` bench (`engine/benches/models_loop.rs`) should keep its speed: run `cargo bench -p promptforge-engine --features test-support --bench models_loop` on Step 3's commit and on this step, and report a clear slowdown.
- Size: estimates are `models_loop.rs` about 300 lines, `models_loop-machine.rs` about 260, `models_loop-tests.rs` about 400, `coro.rs` about 470, `error-value.rs` from 462 to about 470, `protocol/parse.rs` from 428 to about 429, and `__impl_coro.lua` about 360. If `models_loop-tests.rs` would pass 450 lines, the machine's VM-free tests move to a file of their own, and the three-file `models_loop-*` group becomes a `models_loop/` directory under the flat-directory rule.
- Verify: the Project Survey's commands at the run's chosen scope: the full-suite test command, which includes `cargo test -p build-xtask`; the linter command with the headless `cargo check -p gateway --no-default-features`; the formatter check; the workspace and facade docs builds; and the facade surface check `cargo +nightly-2026-09-05 xtask api --check`, which passes with no diff.
- Commit: `Run models.loop rules in a Rust state machine`.

</step-4>

<step-5>

### Step 5: Fold the task-notice drain into the chat dispatch

- Component: Drain fold

- Piece: the component's only piece. It needs Step 4, whose machine loses its drain phase here. Line numbers are `master`'s at `b5ea5d9b4`, before Step 1, unless this step says otherwise; earlier steps moved code, so find each cited passage by its content.
- Rules it moves, each quoted verbatim from Functional Specification and followed by where it runs from this step:
  - Records: "each drained notice: `{ role = "user", content = notice }`". From this step `prepare_chat` pushes these records, in queue order, before the round's records are read.
  - "Emptiness: `messages must not be empty` stays the chat parse's check at every round, after that round's drain, so pending notices can fill an empty list and `h:loop(messages.new())` runs one round over them." From this step the check runs in `prepare_chat` right after the notice push, with the same text and no event, which keeps that rule: the refusal still reaches the `models.loop` call site, and Step 3's test 4 passes unchanged.
  - "Rounds: at most `max_tool_iterations` rounds, the terminal one included. Each starts with the drain, then the `chat` yield. After the last allowed round's batch is appended, the loop raises `tool_loop_exhausted` instead of starting another round." From this step the drain runs inside the chat dispatch, so a round starts with the `chat` yield.
- What changes for a run, from Technical Design, verbatim: the `chat` of the chain that owns the model tasks is issued in the chain step that finished its batch, rather than after one more trip through the ready queue (`answer_inline`, `scheduler.rs`, lines 337 to 341). Notices land in the same rounds as today, because the queue is read at the same point of that chain's own sequence, so the guide's rule that a notice joins the first round that gathers notices after its task ends (`promptforge-docs/src/language/15-tasks.md`, line 992) stays true. Across chains, the order effects are issued in, and so round ids, effect ids, and event order, changes whenever another chain is ready when the owning chain's batch ends. Chains that move in step keep their order, as the three looping fanout arms of `execute/tests/fanout_acceptance.rs` (lines 351 to 410) do.
- Changes, as Technical Design's Phase 2 bullets give them:
  - `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`: `prepare_chat` (lines 104 to 184) starts by taking `self.drain_task_notices(id)` (`scheduler/notices.rs`, line 85), which also joins each task (line 90). When that returns notices, it pushes one user record per notice onto the list with `MessageList::push`, before it reads `list.records()` (line 110) and so before the list's commit ahead of the issue (lines 172 to 182), which records the wire request the Chat effect's `after` and `keep` refer to. Then it refuses an empty list by returning the Engine's `Error::Lua` with `messages must not be empty`, the error the parse's refusal becomes in the Engine, so the answer at the call site is unchanged. The rest of `prepare_chat` stays.
  - `crates/promptforge-internal/lua/src/messages.rs`: `MessageList::push` (line 103) becomes `pub`, so the Engine can call it. The Engine already imports `MessageList` through `engine/src/lua.rs` (line 18), and `MessageList` is absent from `crates/promptforge/public-api.txt`, so the facade surface stays the same.
  - `crates/promptforge-internal/lua/src/protocol/parse/chat.rs`: the empty-list refusal (lines 52 to 54) leaves `parse_chat`.
  - The machine (`models_loop-machine.rs`) drops `Phase::Draining`, `Then::Drain`, and `Input::Drained`, and a round starts with `Then::Chat`; the adapter (`models_loop.rs`) drops their reads and its `drain_task_notices` request. The trampoline stays as it is.
  - Deleted: `Request::DrainTaskNotices` (`protocol/request.rs`, lines 136 to 142); `Answer::DrainTaskNotices` (`protocol/answer.rs`, lines 238 to 241) and its `map_error` arm (line 272); the parse arm (`protocol/parse.rs`, line 219); the render arms (`protocol/render.rs`, lines 171 to 175 and line 203); `blocked_on`'s arm and the dispatch arm (`scheduler/dispatch.rs`, line 97 and lines 187 to 190); `dispatch_drain_task_notices` (`scheduler/notices.rs`, lines 96 to 101).
  - Docs: `scheduler/notices.rs` (lines 14 to 18), `scheduler/dispatch.rs` (lines 8 to 10), and the docs of `parse_chat` and `Request::Chat`, which stop describing a drain request and the parse's emptiness check.
  - Public API: none.
- Tests:
  - From Technical Design, verbatim: the two drain tests in `protocol/tests/answer.rs` (lines 283 to 326) and the one in `protocol/tests/parse_tasks.rs` (lines 156 to 165) go; the drain steps of `lua/tests/shims.rs` (lines 262 to 267 and 305 to 308), of `quota.rs` (lines 106 to 147), and of the yield-level characterization test go. The yield-level characterization test is Step 3's test 18, in `engine/src/lua/tests/models_loop_contract.rs`.
  - `an_empty_list_is_the_calls_error` in `crates/promptforge-internal/lua/src/protocol/tests/parse_chat.rs` (lines 189 to 197) goes, because the parse no longer refuses an empty list. Step 3's test 4 pins the refusal through `models.loop`.
  - In `models_loop-tests.rs`, the machine and adapter tests of `Phase::Draining`, `Then::Drain`, and `Input::Drained` go, and the cap test asserts the cap is raised after the last allowed batch and before another `chat`.
  - `quota.rs`, from Testing Plan: after Phase 2 a round has two yields, so it is measured again. The floor becomes two, one instruction per yield in the span, and the ceiling is re-measured and set at about three times the measured cost.
  - From the execution plan, verbatim: every multi-chain test that fails is reviewed for scripted replies handed out in issue order, and each script change is recorded in the change, as a line of the commit body naming the test and the reordering. A failure that a scripted-reply order does not explain is a regression to fix, not a script to change.
  - Every other test, Step 3's included, passes unchanged.
- Cost and risk, from Technical Design: about twelve files with a net deletion, plus a review of every multi-chain test whose scripted replies are handed out in issue order; the reordering is the risk its decision weighs. `scheduler/chat.rs` (408 lines) stays under 500.
- Verify: the Project Survey's commands at the run's chosen scope: the full-suite test command, which includes `cargo test -p build-xtask`; the linter command with the headless `cargo check -p gateway --no-default-features`; the formatter check; the workspace and facade docs builds; and the facade surface check `cargo +nightly-2026-09-05 xtask api --check`, which passes with no diff.
- Commit: `Fold the task-notice drain into the chat dispatch`, with each script change listed in its body.

</step-5>

<step-6>

### Step 6: Resume the chat answer as an opaque ChatResult

- Component: Opaque chat answer

- Piece: the component's only piece. It lands after Step 5, because both edit `read_input`. Line numbers are `master`'s at `b5ea5d9b4`, before Step 1, unless this step says otherwise; earlier steps moved code, so find each cited passage by its content.
- Changes, as Technical Design's Phase 3 bullets give them:
  - `crates/promptforge-internal/lua/src/protocol/answer.rs`: `ChatResult` itself becomes the userdata, with `impl mlua::UserData for ChatResult {}` and no methods beside its declaration, so no wrapper type is needed. It is absent from `crates/promptforge/public-api.txt`, so the facade stays the same. `ChatResult`'s doc (lines 92 to 107) says it resumes the loop as an opaque value.
  - `protocol/render.rs`: the `Answer::Chat(Ok(result))` arm of `into_envelope` (line 190) resumes `(true, result)` as that userdata, and `chat_result_table` (lines 49 to 96) is deleted.
  - `models_loop.rs`: `read_input` takes the result out with `AnyUserData::take::<ChatResult>()` and hands the machine the scheduler's own value, with `model`, `metrics`, and each call's `tool` set, so the adapter's table reader goes, and `read_input`'s doc stops describing it. The machine stays as it is: it already reads an empty reply as no reply, the renderer's rule (line 70), and builds each `tool_call` request's table from the call's JSON.
  - Public API: none. Risk: low, because the chat answer is never author-visible.
- Tests, from Technical Design, verbatim:
  - The eight table-shape tests of `protocol/tests/answer_chat.rs` (lines 8 to 300 and 321 to 395) give way to one test that a chat answer resumes as a `ChatResult` userdata the step takes, beside the machine's typed tests; the error test (lines 301 to 320) stays, as does the adapter's table-reader test until the reader goes. The reader goes in this step, so its test in `models_loop-tests.rs` goes too.
  - `shims.rs` and `quota.rs` render their answers through `into_envelope`, so they keep working, as does Step 3's test 18.
  - Every other test passes unchanged.
- Its limit, from Technical Design: a model-issued bound tool's text arrives through the `tool_call` answer, rendered as a Lua string (`scheduler/apply.rs`, lines 170 to 189; `protocol/render.rs`, lines 147 to 151), and the `tools.call_as_model` test hook and the Lua `dispatch_tool` read that same answer. Keeping that text out of Lua needs the hook retired or rebuilt over the step, so it is Deferred and not part of this step.
- Verify: the Project Survey's commands at the run's chosen scope: the full-suite test command, which includes `cargo test -p build-xtask`; the linter command with the headless `cargo check -p gateway --no-default-features`; the formatter check; the workspace and facade docs builds; and the facade surface check `cargo +nightly-2026-09-05 xtask api --check`, which passes with no diff. This is the last step, so every Exit criterion holds here: the verification commands pass, the facade surface check shows no diff, and the books rebuilt cleanly in Step 2.
- Commit: `Resume the chat answer as an opaque ChatResult`.

</step-6>

</execution-plan>
