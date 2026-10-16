---
name: Require replaces lua shared
overview: Replace the H1's `lua shared` block with a Runvil `require` that works like `call` but runs a reachable section in the caller's own environment, once, and returns its value, so shared library code runs only where an author asks for it. Lands before the late-bound Plugin offering plan.
todos: []
isProject: false
---

# Require replaces lua shared

<product-contract>

## Product Requirements

Prompt authors need a way to write library code once and reuse it across sections. Today a single `lua shared` block under the H1 does this by replaying itself into every section, so shared code runs everywhere, whether or not a section asked for it. That automatic replay also collided with the late-bound Plugin offering design, which limited prompt-wide offers to the H1. This plan replaces `lua shared` with `require`, which runs a section as a library only where an author asks for it. Each section requires what it uses, and child sections start without their parent's libraries.

- Problem and users:
  - Prompt authors write reusable library code and reusable prompt text, and want it available to any section that needs it, including nested task workers.
  - Today a prompt can hold one `lua shared` block, placed under the H1 (`crates/runvil-internal/parser/src/fence.rs` lines 30-77). Runvil replays it as the first chunk of every section's Lua environment, before the section's own code (`crates/runvil-internal/engine/src/execute/section_vm.rs` lines 96-105 and 145-161). The H1 and task sections started by `tasks.spawn` or `tasks.fanout` take the same setup path (`crates/runvil-internal/engine/src/execute/scheduler/tasks.rs` lines 1-9; `crates/runvil-internal/engine/src/execute/tests/scheduler/fanout/arms.rs` lines 73-112).
  - The late-bound Plugin offering design, as written, makes its `always_` forms H1-only, so an `always_` call in `lua shared` would run, and raise, in every section (`C:\Users\Vinnie\.cursor\plans\late-bound_plugin_offering_d2575252.plan.md`, Behavior). Inheritance, where child sections take their parent's model choice and tool offers, was considered as a replacement for the `always_` forms, and it is not a clear win: its problems have no settled answers, and the answers on hand are not clearly better. The offering plan now retires the `always_` forms instead, and each section gets shared offers by requiring a shared section.
- Goals:
  - Library code lives in an ordinary section and runs in each section that requires it.
  - A library can hand back a value, such as a table of functions, so helpers stay out of the global namespace.
  - Prompt text becomes reusable: a library can return its prose.
  - The mechanism reads like ordinary Lua.
- Non-goals:
  - Changing how far `call` and `jump` reach.
  - Loading Lua files or native modules.
  - Checking `require` calls at parse time.
- Success criteria:
  - Any section, including a nested task worker, can require a reachable library and use what it defines or returns.
  - Prompts, tests, and docs in this repository use `require` for shared code, and `lua shared` is gone.
  - Shared setup, such as offers and a model choice, can come from a library that the H1 requires, with no library code replayed into other sections.
- Constraints:
  - This plan lands before the late-bound Plugin offering plan: `C:\Users\Vinnie\.cursor\plans\late-bound_plugin_offering_d2575252.plan.md`.
  - Runvil's Lua environment loads only the string, table, and math libraries, so `require` is free for Runvil to define (`crates/runvil-internal/lua/src/vm.rs` lines 262-264).
  - Library code must be able to do anything a section can, including suspending calls.
  - Breaking existing prompts is acceptable.
  - The prompt keeps exactly one H1 (`crates/runvil-internal/parser/src/parse.rs` line 118), and the reach rules assume it.
- Open questions: None

## Functional Specification

`require '## heading'` runs a reachable section as a library in the caller's own environment, once, and returns its value. It works like `call`, except for where the section runs and how often. A library's prose, substitutions, and suspending calls work as in any section. Errors surface at the call. A library stays an ordinary section, so its placement relative to the walk is the author's choice.

- Actors and workflows:
  - A prompt author puts reusable Lua in a section and requires it from the sections that need it. For example, a Lua block in the `## Research` section:

    ```lua
    local shared = require '## shared_stuff'
    shared.search_all(args.topic)
    ```

    And the Lua block in the `## shared_stuff` section, a sibling of `## Research`:

    ```lua
    local M = {}
    function M.search_all(topic)
      -- code
    end
    return M
    ```

  - A library shares prompt text by returning it. For example, a `## house_style` section holds shared instructions as prose and ends with `return prose`, and a section reads them with `local style = require '## house_style'`.
  - The H1 and every section can call `require`. Requiring a library from the H1 runs shared setup in the H1, such as `models.default` and, until the offering plan retires it, `tools.always_offer`. Inheritance of offers and model choice by child sections was considered and is not a clear win, so a section that needs shared offers or helper functions requires the library itself.
- Inputs and outputs:
  - `require` is an ordinary Lua call, so it can appear anywhere in a block, and the calling block's `local` variables stay as they are.
  - `require` names a section by heading string, the same way `call` does, for example `require '## shared_stuff'`.
  - A library's blocks run in order. A block that returns a value stops the library there, and `require` hands back that value, which can be anything, including a table or a function. A block with no `return` lets the next block run, and `return nil` counts as no `return`. If no block returns anything, `require` hands back `true`.
  - Each first `require` of a library records library-load events that name the library, in place of today's shared-library load events.
- States and validation:
  - Reach: `require` reaches a section's direct children, its siblings, and the siblings of each of its ancestors, up to the top-level sections. The H1 reaches every top-level section. A nested worker can therefore require a top-level library.
  - `call` and `jump` keep their narrower reach: from the H1, every top-level section; from any other section, its siblings and its direct children (`crates/runvil-internal/engine/src/execute/walk_target.rs` lines 4-11 and 71-75). Workers that `tasks.fanout` starts are often nested sections, such as the `### Worker` under `## Research` in `crates/runvil-internal/engine/tests/prompts/execution/fanout-basic.md` lines 9-18.
  - A `require` made while a library loads looks the heading up from the library's own position, so a library can pull in its own nested helpers wherever it's required from.
  - Every other section-addressing call in library code resolves from the calling section's position: `call`, `jump`, `prompt.section`, `prompt.list`, `tasks.spawn`, and `tasks.fanout` (`crates/runvil-internal/engine/src/execute/walk_target.rs` lines 4-11). So does a `require` that a library function makes after the load. A library's helper functions therefore address the sections the caller can reach.
  - `prompt.section` and `prompt.list` keep the narrower reach of `call`, so from a nested worker `require '## house_style'` works where `prompt.section '## house_style'` raises.
  - The library runs as a section at the point of the call, in the caller's environment. It sees the caller's globals, `var`, `args`, and other state, and globals it defines stay visible to the caller after the call. Its `local` variables stay inside the library.
  - Prose: in any section, each Lua block gets a `prose` value built from the text before it, starting at the top of the section or the last thematic break, with `{{ }}` substitutions filled in on first read (`crates/runvil-internal/engine/src/execute/scheduler/step.rs` lines 93-98; `crates/runvil-internal/engine/src/execute/section_context.rs` lines 163-172). A library's blocks get `prose` built from the library's own text, with substitutions filled in from the caller's environment. After the call, the caller's own `prose` is just as it was, including text the caller has already read.
  - Library code can do anything a section can, including suspending calls such as `store`, model, and tool calls.
  - Library code runs after the Plugin preludes, so it can use what they define, as `lua shared` code can today (`crates/runvil-internal/engine/src/execute/section_vm.rs` lines 145-153).
  - Caching: the first `require` of a section in an environment runs it. Later calls return the saved value, `false` included, and so do calls made from inside other libraries.
  - A library that raises while loading stays out of the cache: the error reaches the caller, and the next `require` runs the library again. Globals it defined before the error stay defined, as in standard Lua.
  - Code that runs while a library loads, such as a model call, runs once in each section that requires the library.
  - A required section stays an ordinary section: the walk runs it when it reaches it, and `call` and `jump` can target it. Placement is the author's choice.
  - Requires are not inherited. A child section's environment starts without its parent's libraries, and each section requires what it uses.
- Errors and recovery:
  - Requiring a section out of reach raises at the call. The parser leaves `require` calls alone, so a misspelled heading surfaces when the call runs.
  - A `require` cycle raises an error that names the sections in the cycle.
  - Errors raised in library code, while it loads or later when the caller invokes a library function, report the library's location and the line in the prompt file.
  - Calling `jump` while a library loads raises, as it does in `lua shared` today (`crates/runvil-internal/lua/src/vm/run.rs` lines 22-27). A library function that calls `jump` later, when the caller invokes it, acts on the caller's section as usual.
  - `require` raises when a Plugin prelude calls it, because preludes run before any block starts.
  - When the walk reaches a library, the library runs under the section rules, where a section's blocks may return only scalar values and a returned scalar ends the chain (`crates/runvil-internal/lua/src/vm/run.rs` lines 140-143 and 261-266; `crates/runvil-internal/engine/src/execute/scheduler/step.rs` lines 281-284). A library that only defines globals runs harmlessly. A library that returns a table fails the run, and a library that returns a scalar ends the chain with that value, so a walked `## house_style` would make its text the run's output.
  - Two placements keep a returning library away from the walk: after the section that returns, or nested under the section that uses it, since the walk reaches nested sections only through `jump` (`crates/runvil-internal/engine/src/execute/scheduler/walk.rs` lines 1-10).
  - `lua shared` is removed. A `lua shared` fence becomes a parse error that points to `require`. The error for the already-removed `lua prompt` fence, which today suggests `lua shared`, points to `lua` and `require` instead (`crates/runvil-internal/parser/src/fence.rs` lines 46-50).
- Security and privacy behavior:
  - `require` loads sections only. It has no access to files or native modules.
- Acceptance criteria:
  - A section requires a sibling library and uses the table it returns.
  - A nested worker requires a top-level library.
  - A second `require` of the same library in a section returns the saved value without running it again, including a saved `false`.
  - A library's prose, with substitutions from the caller's environment, reaches its own blocks, and the caller's `prose` is unchanged afterward.
  - Library code makes a suspending call, such as a `store` or model call, while it loads.
  - A cycle, an out-of-reach heading, a `jump` during load, and a `require` from a Plugin prelude each raise.
  - A library that raises while loading runs again on the next `require`.
  - An error in library code shows the library's location and its line in the prompt file, both during loading and when a library function runs later.
  - A library's first load records library-load events that name it.
  - A child section starts without its parent's libraries and loads them with its own `require`.
  - A `lua shared` fence fails to parse with an error that points to `require`.

</product-contract>
<implementation-contract>

## Technical Design

Runvil defines its own `require` in every section's Lua environment, the H1 included, and keeps the standard package library unloaded. `require` loads the target section's already-compiled blocks into the caller's environment and runs them inside the calling block's coroutine, which is what lets library code suspend. Section setup stops replaying shared code, and the parser rejects the `lua shared` fence. The engine and the Lua crate coordinate through two engine-supplied closures, one that finds a library and one that renders library prose, following the pattern prompt reads already use. Names below that don't exist yet are marked as proposals; every other name is today's code.

- Architecture:
  - Standard Lua 5.5 `require` lives in the package library, runs a module's loader through a call that can't suspend, and has no cycle detection (vendored Lua 5.5.0 `loadlib.c`, `ll_require`, lines 650-677, from the `lua-src` 550.1.1 crate pinned in `Cargo.lock`; the `lua55` feature is set in `Cargo.toml` line 120). With mlua's safe mode, loading the package library blocks native modules but still lets `require` load Lua files from disk (mlua 0.12.0, pinned in `Cargo.lock`: `src/state/raw.rs` lines 303-308 and `src/state.rs` lines 379-380 and 2412-2438). Runvil's own `require` runs library code from Lua inside the caller's coroutine, loads sections only, and detects cycles.
  - Every compiled Lua block, a `LuaProgram`, already holds its source, its bytecode, and a location name, and `LuaProgram::load` loads it into any section's environment without running it (`crates/runvil-internal/lua/src/program.rs` lines 53-58 and 170-178).
  - Runvil's hardening removes the standard `require` today, and sandbox tests check that it's gone (`crates/runvil-internal/lua/src/hardening.rs` lines 12-46). Runvil's `require` installs after hardening and joins `RESERVED_NAMES` with the other Engine globals (`crates/runvil-internal/lua/src/globals.rs` lines 155-218), so `check_collision` rejects a Plugin prelude that defines its own (`crates/runvil-internal/lua/src/prelude.rs` lines 177-203).
- Modules and interfaces:
  - Parser to engine, removed in one step: today `fence::split_h1` compiles the shared fence into `Prompt.replay`, the engine reads it through `detail::replay` into `RunState.shared`, and `RunState::vm_setup` passes it as `SectionVmSetup.shared` to `setup_section_vm`, which calls `SectionVm::replay_shared` and then checks for a store conflict (`crates/runvil-internal/parser/src/fence.rs` lines 39-77; `parser/src/parse.rs` lines 146-169; `parser/src/prompt.rs` lines 138-165; `parser/src/detail.rs` lines 18-21; `engine/src/execute/run.rs` lines 256-259; `engine/src/execute/context.rs` lines 89-91, 146, 178, and 377-393; `engine/src/execute/section_vm.rs` lines 54-78 and 116-161; `lua/src/vm/run.rs` lines 15-51). `split_h1` reports a `lua shared` fence as a parse error that points to `require`, its `lua prompt` removal error stops suggesting `lua shared`, and every link in that chain is removed together, since each crate's change depends on the next.
  - Install point: `setup_section_vm` installs `require` where the shared replay ran, after `install_preludes` and before `install_store_shims` (`crates/runvil-internal/engine/src/execute/section_vm.rs` lines 134-169). Preludes run during setup, outside any block's coroutine and before `require` exists, which is how a prelude's call to `require` raises.
  - Library lookup, engine to Lua crate: prompt reads already cross this boundary as a closure. `install_scheduler_control_globals` takes `Fn(String) -> Result<Option<SectionText>, E>`, and the engine builds it from `read_visible` over `Visible::of` for the section's `SlicePath`, returning `SectionText` (`crates/runvil-internal/lua/src/vm/install.rs` lines 198-280; `engine/src/execute/section_context-construct.rs` lines 96-103; `engine/src/execute/walk_target.rs` lines 26 and 85-95; `engine/src/heading_address.rs` lines 124-138; `lua/src/handles.rs` lines 144-149). `require` gets a second closure of the same kind, proposed as `LibraryResolver`, taking a heading and an optional `LibraryKey` and returning a `Library`. `Library` (proposed, beside `SectionText`) carries an opaque `LibraryKey`, the heading, and the section's blocks in order, as compiled `LuaProgram`s and prose templates. The Lua crate can't name the engine's `SlicePath`, so while a library loads, the Lua side passes that library's `LibraryKey` back for a nested `require`, and the engine maps the key to the library's position. Every other lookup passes no key and resolves from the calling section.
  - Reach: the engine adds a lookup beside `find_visible` for `require`'s wider reach, proposed as `heading_address::find_reachable`, which searches direct children, siblings, and the siblings of each ancestor. `call`, `jump`, prompt reads, and task calls keep `find_visible`.
  - Library prose, engine to Lua crate: today `Scheduler::start_lua` takes `Chain.pending_prose` and calls `SectionContext::install_lazy_prose`, which builds `subst::Sources` from the run's `args` and `argv` and the frame's `item`, then calls `SectionVm::install_lazy_prose`, `prose::install`, and `globals::install_prose`, which writes the `prose` slot in the `_G` guard's state (`crates/runvil-internal/engine/src/execute/scheduler/step.rs` lines 144-160; `engine/src/execute/section_context.rs` lines 163-189; `lua/src/vm/install.rs` lines 336-353; `lua/src/prose.rs` lines 35-135; `lua/src/globals.rs` lines 48-50 and 110-116). Substitution lives in the engine, so the engine also hands the Lua crate a renderer for library prose templates, proposed as `ProseRenderer`, built from the same sources. Nothing reads or restores the `prose` slot today, and each install starts empty, so the prose module gains a save and a restore that keep the memoized text, proposed as `prose::swap` and `prose::restore`. A library block runs between the two, and the restore runs on every exit, errors included.
  - The `require` helper: the Lua-side helpers live in the embedded `__impl_coro.lua`, compiled once and installed per environment by `install_shim_prelude`, with Rust helpers passed in as chunk arguments (`crates/runvil-internal/lua/src/coro.rs` lines 37-40 and 168-215; `lua/src/__impl_coro.lua` lines 13-29). `require` follows the same pattern: a Lua function, so library blocks run inside the caller's coroutine and can suspend, backed by Rust helpers that resolve a library, load its blocks with `LuaProgram::load`, track the loading library's `LibraryKey`, and swap prose. It keeps a per-environment cache table with a loading marker, which is how a cycle raises with the sections named.
  - `jump` refusal: `jump` already refuses while `local_handler_depth` is above zero, a counter that `enter_local_handler` and `leave_local_handler` maintain (`crates/runvil-internal/lua/src/vm/install.rs` lines 225-245; `lua/src/__impl_coro.lua` lines 168-170). Library loading adds the same kind of counter. Outside such counters, `jump` records its target and raises an unwinding error, and the recorded target wins even if author code catches that error (`lua/src/vm/install.rs`; `lua/src/vm/run.rs`).
  - Return values: `require` reads its library blocks' return values in Lua, so `scalar_return`, the section block's scalar-return rule, stays on section blocks only (`crates/runvil-internal/lua/src/hardening.rs` line 204).
  - Error mapping: `SectionVm::block_failure` and `SectionVm::map_chunk_failure` map an error through `LuaProgram::map_runtime_error` of the started block only, leaving other chunks' markers as they are (`crates/runvil-internal/lua/src/vm/run.rs` lines 280-301; `lua/src/program.rs` lines 194-207 and 233-292). `SectionVm` keeps the programs of the libraries it has loaded, and the mapping tries each of them too (`lua/src/vm.rs` line 59).
  - Events: lifecycle events today are payload-free constructors, `Lifecycle = fn(String, String, Provenance) -> Event`, reported with `Emitter::report(section, boundary)` (`crates/runvil-internal/types/src/event-lifecycle.rs` lines 15-17; `types/src/emitter.rs` lines 231-234). Library-load events name the library, so they become `Event` variants with a `library` field, proposed as `LuaLibraryLoadStarted`, `LuaLibraryLoadSucceeded`, and `LuaLibraryLoadFailed`, which the `require` helpers emit through a new `Emitter` method.
- File and public API changes:
  - Lua API: adds `require`.
  - Prompt syntax: removes the `lua shared` fence form.
  - No shipped prompt uses `lua shared`. A repository-wide search, excluding build output, finds it in 37 files, covering its own implementation, tests, and docs, and none under `prompts/` or `crates/workshop/agents/agents/`. Tests and docs in this repository move to `require`.
  - Some tests rely on the replay, for example the `models.default` call inside a shared block in `crates/runvil-internal/engine/src/execute/tests/model_and_reply-globals.rs` lines 45-46.
  - Code comments that describe the shared-library replay get updated, such as the notes that make `tools.always_offer` and `models.default` safe to repeat under replay (`crates/runvil-internal/lua/src/tools.rs` lines 271-273; `crates/runvil-internal/lua/src/models.rs` lines 226-230).
  - Lifecycle events: `Event::LuaSharedLoadStarted`, `LuaSharedLoadSucceeded`, and `LuaSharedLoadFailed`, with their lifecycle constants, are emitted only by the shared replay and listed in `crates/runvil/public-api.txt` lines 874-885 (`crates/runvil-internal/types/src/event.rs` lines 206-210; `types/src/event-lifecycle.rs` lines 58-60; `lua/src/vm/run.rs` lines 34-37). They give way to the library-load variants. Today only tests and recording helpers consume them, so the change also reaches `types/src/event-tests.rs` lines 75-77, `engine/src/test_support/recording/observation.rs` lines 52-54, 129-133, and 261-263, `engine/src/test_support/recording/forward.rs` lines 50-52, and `engine/src/test_support/recording/forward-tests-variants.rs` lines 73-75.
- Data, persistence, failure, security, and privacy constraints:
  - Persisted formats stay as they are. Library code issues its effects as part of the calling section, so run records and replays treat them like the section's own.
  - Errors raised inside library code report the library's location and prompt-file line.
  - `require` reaches sections only, so prompts gain no access to files or native modules.

</implementation-contract>
<verification-contract>

## Testing Plan

Tests pin every rule in the Functional Specification and the removal of `lua shared`. Lua-level tests cover return values, caching, cycles, reach, and errors. Whole-prompt tests cover what only the scheduler shows: suspending calls inside libraries, prose and substitutions, nested workers, and the walk reaching a library. The existing suite, moved off `lua shared`, is the regression check.

- Unit:
  - Return values: a block that returns a value stops the library and `require` hands it back, whatever its type; a block with no `return`, or with `return nil`, lets the next block run; and `require` hands back `true` when no block returns anything.
  - Caching: a second `require` returns the saved value without running the library again, `false` included, and so does a `require` from inside another library.
  - A cycle raises an error naming the sections in it.
  - Reach: direct children, siblings, siblings of each ancestor, and every top-level section from the H1. An out-of-reach heading raises at the call.
  - A `require` made while a library loads resolves from the library's position; a later `require` from a library function and every other section-addressing call resolve from the calling section.
  - A library that raises while loading stays out of the cache and runs again on the next `require`.
  - `jump` during library load raises.
  - `require` raises when a Plugin prelude calls it, and a prelude can't define a global named `require`.
  - A `lua shared` fence fails to parse with an error that points to `require`.
- Integration and end-to-end:
  - Library prose and substitutions from the caller's environment, with the caller's `prose` restored afterward, including after a library error and including text the caller had already read.
  - Suspending calls inside library code: `store`, model, and tool calls.
  - A nested `tasks.fanout` worker requiring a top-level library.
  - The H1 requiring a library that sets prompt-wide state.
  - The walk reaching a library: a globals-only library runs harmlessly, a returned table fails the run, and a returned scalar ends the chain.
  - An error inside library code reports the library's location and prompt-file line, both during loading and when a library function runs later.
  - A library's first load records library-load events that name it.
  - A child section entered from a parent that required a library starts without that library, and its own `require` loads it.
- Regression, security, and performance:
  - Every existing test that used `lua shared` passes after moving to `require`.
  - Sandbox tests that check `require` is absent move to checking Runvil's `require`, and tests that read the shared-library load events move to the library-load events.
  - `require` reaches sections only and loads no files or native modules.
  - Load-time library code runs once per requiring section.
- Exit criteria:
  - The whole suite passes, and no prompt, test, or doc in this repository uses `lua shared` apart from the parse error that rejects it and that error's test.

</verification-contract>
<decision-record>

## Decision Record

Every decision serves one aim: reusable library code that runs only where an author asks for it. Each entry records the call, the reason for it, and the user's own words or chosen option. Rejected alternatives record why they lost and what would reopen them.

- Decisions:
  - `require` replaces `lua shared`, which is removed, and the H1 gives up its `lua shared` role. Rationale: shared code exists for reuse, so it should run only where an author asks for it; the automatic replay into every section collided with the H1-only `always_` forms the offering design proposed; and `require` is the name Lua programmers already know. User: "So, the whole purpose of the shared section is to allow you to write library code that you could reuse." Also: 'Can we get rid of "lua shared" and just do something like this', and 'no, I meant "require" not "import." Lua has the "require" keyword. Would this be better?'
  - Runvil defines its own `require`, which loads sections and nothing else. Rationale: the environment has no package library, so the name is free; the standard `require` would let prompts load Lua files from disk and would block suspending calls in library code (Technical Design, Architecture). User: "It should be able to do everything."
  - `require` works like `call`, but runs the section in the caller's own environment, and once. Rationale: the user wanted include behavior. A literal text insert breaks in two places: a section's prose sits between Lua blocks while `require` runs inside one, and a library's `return` would end the caller's section. Running the library as a section in the caller's environment keeps everything else include-like. User: "I want it to work just like an include. Like, if you say require, and then you put a section, it would be as if the text of that section was inserted into that spot." The user then approved this rule.
  - A library's prose and substitutions work as in any section, for the library's own blocks. Rationale: a library section behaves like a normal section. User: "If the, if the library section has pros [prose], then the Lua gets the pros [prose] variable and it gets the accumulated state." Also: "the substitutions have to happen."
  - Library code can do anything a section can, including suspending calls. Rationale: library code is ordinary section code running in another place. User: "Of course it can make suspending calls. It should be able to do everything."
  - A library's blocks run in order. A block that returns a value stops the library, and `require` hands back that value, whatever its type. A block with no `return` lets the next block run, and if no block returns anything, `require` hands back `true`. Rationale: module tables need any-value returns, a block with no `return` moving on matches how a section moves to its next block, and `true` matches standard Lua. The user was unsure ("what require returns for a library with several Lua blocks? I'm not sure.") and approved this rule.
  - A library runs once per environment, and later calls return its saved value, `false` included. Rationale: this is Lua's own `require` behavior, so a library pulled in by two others runs once, and a library that returns `false` still counts as loaded, where standard Lua would run it again. User chose: "It runs once, and the second call returns the saved value, like Lua's require."
  - A `require` cycle raises an error that names the sections in it. Rationale: standard Lua 5.5 has no cycle detection and recurses until the stack overflows (Technical Design, Architecture). Approved with the switch to `require`.
  - `require` reaches direct children, siblings, and the siblings of every ancestor, while `call` and `jump` keep siblings and direct children. Rationale: workers that `tasks.fanout` starts are often nested sections, `lua shared` reaches them today (Functional Specification, States and validation; Product Requirements, Problem and users), and the narrower reach would force a copy of each library under every parent; control flow needs less reach than reuse. User's first call: "require uses teh same scoping rules. siblings, or direct descendants". The user then chose: "Yes: require also reaches anything the section's parents could reach; call and jump stay strict".
  - A `require` made while a library loads resolves from the library's position; every other section-addressing call in library code, and any `require` a library function makes later, resolves from the calling section. Rationale: a library function runs in the caller's code path, and `call`, `jump`, prompt reads, and task calls already resolve from the running section, so one rule covers everything outside loading. `prompt.section` and `prompt.list` keep the narrower reach, which the docs state. User chose: "A require made while a library loads resolves from the library's position; everything else, including later calls from library functions, resolves from the calling section".
  - `jump` raises while a library loads, and a library function that calls `jump` later acts on the caller. Rationale: control transfer during a load has no coherent meaning, which is also today's `lua shared` rule (Functional Specification, Errors and recovery). The refusal happens at the `jump` call, with the depth-counter check that already refuses `jump` in local tool handlers, because a caught jump still wins at the end of a block today (Technical Design, Modules and interfaces). User approved.
  - A library that raises while loading stays out of the cache, and the next `require` runs it again. Rationale: standard Lua does the same, and a retry can succeed once whatever failed is fixed. User chose: "Nothing is cached; the error propagates, and the next require runs it again, as in standard Lua".
  - Library-load events that name the library replace the shared-library load events. Rationale: a library's first load stays visible in the run's events, and the shared-library events lose their only emitter. User chose: "Library-load events naming the library, emitted on each first require".
  - Error mapping covers every library loaded into a section. Rationale: today only the started block's lines are rewritten, so library errors would otherwise show block-relative lines (Technical Design, Modules and interfaces). User approved after a review of how `require` interacts with every Runvil Lua feature.
  - Restoring the caller's `prose` puts back its saved slot, including text already read. Rationale: a fresh install renders again from newer state, so the caller could see different text within one block (Technical Design, Modules and interfaces). User approved after a review of how `require` interacts with every Runvil Lua feature.
  - `require` joins the reserved global names and raises when a Plugin prelude calls it. Rationale: preludes run outside any block's coroutine, so suspending library code has no way to run there, and reserving the name keeps a prelude from replacing it (Technical Design, Architecture). User approved after a review of how `require` interacts with every Runvil Lua feature.
  - The parser leaves `require` calls alone, so errors surface at the call. Rationale: each Lua block compiles without exposing a syntax tree, and a text search would misfire on strings and comments. User: "The parser can't check require. That's too much." Also: "We don't have access to the Lua parser, like the, we, that doesn't get exposed to us. And I don't want to grep for it. I think that would be annoying."
  - A required section stays an ordinary section, keeping a returning library away from the walk is up to the author, and the plan states the sharp edge with two safe placements. Rationale: the user preferred author responsibility over a special library marker. User chose: "Author's responsibility, for example placing libraries after a section that returns", and later: "State the sharp edge, and suggest nesting a library under the section that uses it".
  - A `lua shared` fence becomes a parse error that points to `require`. Rationale: no shipped prompt uses `lua shared` (Technical Design, File and public API changes), and breaking existing prompts is acceptable, so a clear error is enough to guide the rest. The error for the already-removed `lua prompt` fence stops suggesting `lua shared` too. User: "breaking existing prompts is ok".
  - This plan is separate and lands before the late-bound Plugin offering plan. Rationale: removing the automatic replay frees that plan from shared code running in every section, whether it keeps H1-only `always_` forms or moves to inheritance. User chose: "A separate small plan that lands before this one".
  - Requires are not inherited by child sections, and each section requires what it uses. The prompt keeps a single H1. Rationale: inheriting requires would bring back implicit replay; a library table held in a parent's `local` variable wouldn't pass down anyway; and a replay during a child's setup would run outside its coroutine, where suspending calls fail. Inheriting a setup library's effects, its offers and model choice, was considered too, and it is not a clear win, so a section that needs those effects requires the library itself. User: "single H1. leave requires out. breaking existing prompts is ok." Later: "we considered inheritance but there are problems with it which have not been solved and the solutions are not necessarily an improvement."
  - The separate `runvil-docs` repository is out of scope for this plan and gets its own update. Thirteen of its language guide files under `src/language/` mention `lua shared`, for example `src/language/05-lua-environment.md` and `src/language/10-models.md`. Rationale: this plan changes the Runvil repository only. User chose: "No: runvil-docs is out of scope here and gets its own update".
  - Every repository fact in this plan carries a repository-relative path, and code questions are settled through bounded evidence lookups whose accepted facts and locations enter the plan. Rationale: a plan that outlives its conversation needs every code fact traceable to its source. The user chose this rule set over the earlier maintenance rules, which kept file names, line numbers, and sub-agents out of the plan.
  - Technical Design names types and functions only at the points where modules must coordinate: the parser-to-engine removal, the install point, the library lookup and prose closures between the engine and the Lua crate, the reach lookup, the `require` helper, the `jump` counter, error mapping, and the event types. New names there are proposals. Rationale: those points need agreement across crates before work starts, and the rest is better decided while implementing. User: "do a light review of the source code to get a broad-strokes idea of how it will be implemented. name types and functions. but dont design the whole thing in-place, just the parts that need coordination".
- Rejected alternatives:
  - An `import shared_stuff` line at the top of a Lua block, found and removed by the parser. Reason: the user meant `require`, which is ordinary Lua, works anywhere in a block, and needs no parser rewrite. Revisit if the parser ever needs a list of libraries ahead of time.
  - An `import 'shared_stuff'` Lua call. Reason: `require` is the Lua name for the same idea. Revisit: none.
  - A literal text include of the section. Reason: a section's prose sits between Lua blocks while `require` runs inside one, and a library's `return` would end the caller's section. Revisit if a block-level include for prompt text becomes worth a separate form.
  - Running a library again on every `require`. Reason: the user chose run-once, which matches Lua's `require` and keeps load-time side effects from repeating. Revisit if authors need the same prose inserted twice in one section.
  - Required sections skipping the walk automatically. Reason: it needs every `require` found at parse time, which the user ruled out, and the user preferred author responsibility. Revisit if walked libraries cause real failures.
  - A heading marker that the walk skips. Reason: the user chose to state the sharp edge instead. Revisit if walked libraries cause real failures.
  - Siblings and direct children only, for `require`. Reason: nested workers couldn't reach top-level libraries. This was the user's first call, replaced by the wider reach. Revisit if the wider reach makes lookups confusing.
  - Dotted names for nested sections, such as `require 'tools.search'`. Reason: `require` uses the same heading strings as `call`. Revisit: none.
  - Checking `require` calls that name a literal heading at parse time. Reason: the same as the no-parse-check decision. Revisit if the Lua compiler ever exposes a syntax tree.
  - Loading Lua's package library and using the standard `require`. Reason: it can load Lua files from disk and blocks suspending calls in library code. Revisit: none.
  - Keeping file names, line numbers, and sub-agent research out of the plan until the design is approved. Reason: the user chose cited repository facts and bounded evidence lookups from now on. Revisit: none.
  - Every section-addressing call made while a library loads resolving from the library's position. Reason: `call`, `jump`, prompt reads, and task calls resolve through the running section's view of the prompt, and the user chose the narrower rule. Revisit if library helpers need to address the library's own nested sections.
  - Every `require` written in library code resolving from the library's position, even when a library function runs it later. Reason: each library function would have to remember the library it came from. Revisit if lazy loading from library functions becomes common.
  - Caching a failed load so later calls raise the same error. Reason: standard Lua retries, and the user chose that. Revisit: none.
  - Removing the shared-library load events with no replacement, or keeping them unused in the public API. Reason: library loads should show up in the run's events, and unused events would mislead readers of the public API. Revisit: none.
  - Child sections inheriting their parent's requires. Reason: it brings back implicit replay, misses libraries held in `local` variables, and runs library code outside the child's coroutine. Revisit if requiring setup libraries in every section proves burdensome.
  - Child sections inheriting their parent's model choice and tool offers. Reason: considered as the way shared setup reaches children, and it is not a clear win: its problems have no settled answers, and the answers on hand are not clearly better. User: "we considered inheritance but there are problems with it which have not been solved and the solutions are not necessarily an improvement." Revisit if those problems get settled answers.
  - Several H1 sections, each passing settings to its own children. Reason: the parser requires exactly one H1 (`crates/runvil-internal/parser/src/parse.rs` line 118), the H1 carries roles beyond offers, such as being the one place `argv` is writable (`crates/runvil-internal/lua/src/argv.rs` line 47), and H2 sections under different H1s couldn't call each other under today's reach rules (`crates/runvil-internal/engine/src/execute/walk_target.rs` lines 4-11). Revisit: none.
- Assumptions, risks, and notes:
  - Plan maintenance: research happens in the chat, with the user. When an open question needs code knowledge, gather the facts through a bounded evidence lookup, agree on a concrete answer, and only then update the plan.
  - Plan maintenance: plan updates stay at the level of behavior and ideas, apart from Technical Design, which names types and functions only where modules must coordinate. Repository facts carry repository-relative paths.
  - Plan maintenance: when the user answers an open question, move the answer to Decisions and remove the question.
  - Plan maintenance: keep the plan lean. Implementation detail beyond the coordination points waits until the user says the design is right, and the rest belongs to implementation.
  - Note: a library that does expensive work while it loads, such as a model call, repeats that work in each section that requires it.
  - Note: the late-bound Plugin offering plan retires the `always_` forms, and each section gets shared offers by requiring a shared section. Inheritance was considered for that job and is not a clear win. Until the offering plan lands, `tools.always_offer` keeps today's behavior and works from any section (`crates/runvil-internal/lua/src/tools.rs` lines 10-12).

### Deferred and Out of Scope

- Deferred: None
- Out of scope:
  - Inheritance of model choice and tool offers by child sections, which was considered and is not a clear win.
  - Changes to `tools.allow_tasks`, which stays as it is. Its rule that a later call replaces the allowlist still lets section code narrow a grant that a library makes, since the library now runs inside the section that requires it (`crates/runvil-internal/lua/src/tools.rs` lines 320-324).

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the gateway, the workspace's one default member (`crates/gateway/app`); the desktop app is `cargo build --locked -p workshop`. Crate build scripts bundle the TypeScript UIs, so a fresh checkout first needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`. Every crate's build script runs `build_ceiling::check()`, which fails the build when any Rust file under the crate's `src`, `tests`, `benches`, or `examples` exceeds 500 lines.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`; drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file: `node --test <path-to-file.mjs>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, with the same `--all-features` exception for the three workshop crates. Engine package names are `runvil` (facade), `runvil-plugin`, `runvil-types`, `runvil-vfs`, `runvil-model-client`, `runvil-lua`, `runvil-parser`, and `runvil-engine`. Workshop UI packages: `npm test --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs`. The workspace run includes `build-xtask`, which holds the boundary and structural checks (`cargo xtask tidy` prints the same report on demand).
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features` for the headless build shape. On PowerShell set `$env:CARGO_BUILD_WARNINGS='deny'` first. Never run a standalone `cargo check --workspace`.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: with `RUSTDOCFLAGS="-D warnings"`, run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` for the facades with default features, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. When the `runvil` facade's public surface changes, also run `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed on this machine), which compares against `crates/runvil/public-api.txt`; `--bless` rewrites the listing.
- Test placement and naming conventions:
  - Unit tests live in a sibling file wired from the module as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `prelude.rs` beside `prelude-tests.rs`); extra topic files take `<stem>-tests-<topic>.rs` (for example `messages-tests-view.rs`, `prelude-tests-environment.rs`). A module with three or more test files uses a `tests/` subdirectory instead (for example `runvil-internal/lua/src/protocol/tests/`, `runvil-internal/engine/src/lua/tests/`). Some older files keep an inline `mod tests { }`.
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` for the `runvil` and `harness` facades, `tests/it/main.rs` for about eighteen others (gateway app, workshop crates, Plugin crates, `runvil-plugin`, `harness-runner`, `gateway-api-discovery`, `gateway-stt`, `build-ui`). Each topic is a `mod` file; shared helpers sit in `support.rs`. Prompt fixtures live under `crates/runvil/tests/prompts/` (currently only `invalid/`, including `removed-lua-prompt.md`).
  - Test functions are snake_case sentences stating the behavior; a `test_` prefix is rare.
  - Test-only hooks sit behind a `test-fixtures` Cargo feature (gateway and workshop crates) or `test-support` (`runvil-lua`, `runvil-parser`, `runvil-engine`, `runvil-plugin`); `build-xtask` checks they do not leak into normal builds.
  - Doctests are banned: `build-xtask` fails on any compiled code block in a doc comment, so examples belong in tests. Benches use criterion with `harness = false` (for example `runvil-internal/lua/benches/surface.rs`).
  - JS tests are `*.test.mjs` beside the source or `*.mjs` under `test/`, run by `node --test`.
- Directory map:
  - `crates/`: all code. Top-level public crates (`runvil`, `runvil-plugin`, `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`), Plugins (`plugin-web`, `plugin-mcp`, `plugin-user-input`), shared utilities (`shared-error-source`, `shared-loopback`, and `shared-ui`, a TypeScript and CSS package rather than a crate), tooling (`build-xtask` for structural checks and `cargo xtask`, `build-workshop` behind the `cargo workshop` alias, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`), and `workspace-hack` for cargo-hakari.
  - `crates/runvil-internal/`: private Engine crates `types`, `vfs`, `model-client`, `lua`, `parser`, `engine`. The `lua` crate holds the Lua runtime: Rust modules plus embedded Lua sources `__impl_globals.lua`, `__impl_coro.lua`, `__impl_tasks.lua`, `__impl_fanout.lua`, and `__impl_store.lua`, each loaded with `include_str!` under an `@crates/...` chunk name. A prompt's shared Lua is a `LuaProgram` held as `shared` in `engine/src/execute/context.rs` and loaded in `lua/src/vm/run.rs`, which reports the `LuaSharedLoadStarted`, `LuaSharedLoadSucceeded`, and `LuaSharedLoadFailed` lifecycle events defined in `types/src/event-lifecycle.rs`.
  - `crates/harness-internal/runner`: the private Harness crate `harness-runner`.
  - `crates/gateway/`: private gateway crates; `app` is the `runvil-gateway` binary, beside `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and the `stt/` speech subsystem whose public member is `api` (`gateway-stt`).
  - `crates/workshop/`: the Workshop Host; `desktop` (Tauri app, package `workshop`), `server` and `server-api`, tier crates `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, `workspace`, and the npm workspaces `ui`, `look`, `platform`.
  - `guide/`: user guide and docs site sources (`books`, `chrome`, `landing`). `prompts/`: example prompts. `tools/`: Node scripts for gateway sidecar staging and live TTS checks, each with a `.test.mjs`. `vibe/`: dated plan and run records. `local/`: gitignored machine-local gateway config and fixtures.
  - `.github/workflows/`: CI (`ci.yml` gates on `ci-green`), release, and nightly jobs. `.githooks/`: pre-commit runs fmt; pre-push runs docs-claims, the headless check, workspace clippy, and cargo-deny. `.config/`: nextest and hakari config. `.cargo/config.toml`: the `workshop` and `xtask` aliases and the Windows `rust-lld` linker. `.cursor/rules/`: Workshop architecture and SPA rules. Several crates carry their own `AGENTS.md`, including `runvil-internal/lua`, `engine`, `types`, and `vfs`.
- Component boundaries (enforced by `cargo test -p build-xtask`):
  - Engine internal graph: `types` and `vfs` depend on no Engine crate; `model-client` on `types`; `lua` on `types`, `vfs`, and `model-client`; `parser` on `lua` and `types`; `engine` on all five. The `runvil` facade re-exports from all six, and `runvil-plugin` depends on `types` and `vfs`.
  - Engine crates (`runvil*`) depend on no gateway, workshop, Harness, or Plugin crate. Other families reach the Engine only through `runvil` and `runvil-plugin`; `crates/runvil-internal/` is private to those two.
  - Harness crates (`harness*`) depend outside their family only on `runvil`, `runvil-plugin`, and `workspace-hack`. Others reach the Harness only through `harness` and `harness-gateway-client`; `harness-runner` is private to `harness`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*`, `workspace-hack`, and outside libraries. The only product crate allowed to depend on a Plugin is `harness-gateway-client`, which implements `plugin-web`'s `SearchProvider`.
  - Gateway crates depend on no runvil, workshop, or Harness crate. Their container crates are private to the family, and Workshop sees the gateway only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop tiers flow one way: server, then features, then services, then vocabulary. Same-tier crates do not depend on each other, except `workshop-registry` on `workshop-protocol`. Each workshop crate root names its tier. The desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`.
  - `shared-*` crates depend on no product crate.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized terms with exactly one meaning each (AGENTS.md Definitions). Engine crates say "the caller" and never mention the Host. `docs-claims.mjs` enforces the wording in every AGENTS.md, `## Invariants` crate doc, and `.cursor/rules` file.
  - Every member inherits strict workspace lints: clippy `all` and `pedantic` at deny, `unwrap_used` and `expect_used` denied, `#[allow]` banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` and `unreachable_pub` warned, broken and private intra-doc links denied. `clippy.toml` bans installing process-global state outside binary entry points.
  - Crate roots open with a `//!` doc, many with a `## Invariants` section.
  - Source directories are flat: a subdirectory needs three or more files, otherwise use `parent-label.rs` siblings wired with `#[path]`; convert in either direction when touching a group on the wrong side of the line. No Rust file may exceed 500 lines; split into private child modules.
  - A removed Engine symbol is added to `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`, and the retired-symbol scan then fails if live Engine source names it again.
  - Dependencies are declared once in `[workspace.dependencies]`, with comments explaining each pin and disabled feature.
  - Error and status messages are written for model consumption: concise, naming what is missing with required versus actual. Comments explain only non-obvious constraints, and platform workarounds cite the upstream issue URL.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder must round-trip exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers).
  - Long-running gateway work reports through `gateway-progress`. Workshop UI CSS uses only `@workshop/look` tokens, and persisted UI state goes through `ui-storage` to `ui-state.json` or the workspace file.

</project-survey>
<execution-plan>

## Execution Instructions

Seven steps in three components take Runvil from `lua shared` to `require`. This plan lands before the late-bound Plugin offering plan (`C:\Users\Vinnie\.cursor\plans\late-bound_plugin_offering_d2575252.plan.md`). The original three work items map to Steps 1-5 (add `require` alongside `lua shared`), Step 6 (move tests to `require`), and Step 7 (remove `lua shared`). Each of Steps 5, 6, and 7 ends a work item and runs the whole suite, and Step 7 checks the Testing Plan's exit criteria. Every other step leaves its crates' tests passing.

Components, in dependency order:

1. `require` runtime (Steps 1-3): the library-load events in `runvil-types` and the Lua-side `require` in `runvil-lua`, driven by a resolver and a prose renderer that the caller supplies. It comes first because the engine wiring consumes its `Library`, `LibraryKey`, resolver, renderer, and install call, and its Lua-level tests need no engine. Its pieces run in sequence: the events exist before the loader emits them, and library prose and error mapping build on the loader's load path.
2. `require` in prompts (Steps 4-5): the engine's wider reach lookup, library construction from parsed sections, and the install in section setup, with whole-prompt tests. It follows the runtime because it plugs engine closures into it, and it precedes the retirement because migrated tests need a working `require`. Its pieces run in sequence: the setup install calls the lookup.
3. `lua shared` retirement (Steps 6-7): tests move to `require`, then the replay chain, its events, and the fence go. It comes last because removal is safe only once nothing depends on the replay. Its pieces run in sequence: the migration keeps coverage alive before the removal deletes the replay's own tests.

<step-1>

### Step 1: Library-load event vocabulary [completed]

- Component: `require` runtime
- Tests: `cargo nextest run --locked -p runvil-types -p runvil-lua -p runvil-engine --all-features`; `cargo +nightly-2026-09-05 xtask api --check`; `cargo fmt --all --check`

Add the three events that name a loaded library. Today's shared-library load events stay until Step 7.

- Artifacts:
  - `crates/runvil-internal/types/src/event.rs`: new `Event` variants `LuaLibraryLoadStarted`, `LuaLibraryLoadSucceeded`, and `LuaLibraryLoadFailed`, each with a `library: String` payload field, in the "Lifecycle: the section VM" group. They carry a payload, so they get no entry in the `lifecycle_constants!` list in `types/src/event-lifecycle.rs`, whose `Lifecycle` constructors take only the three coordinates.
  - `crates/runvil-internal/types/src/emitter.rs`: a new `Emitter` method, proposed `library_load`, that builds these variants through `Emitter::emit` with the run's coordinates, given the section, the library name, and the phase.
  - `crates/runvil-internal/types/src/event-tests.rs`: map the new variants in `lifecycle_of` as payload variants (`None`) and add samples to `payload_samples`, so the round-trip and one-line checks cover them.
  - Recording helpers: `crates/runvil-internal/engine/src/test_support/recording/observation.rs`, `forward.rs`, and `forward-tests-variants.rs` gain observations for the new variants that keep the library name, so suites can assert which library loaded. Update `crates/runvil-internal/lua/src/tests-recording.rs` the same way if it matches events by variant.
  - `crates/runvil/public-api.txt`: regenerate with `cargo +nightly-2026-09-05 xtask api --bless`.
- Behavior:
  - The `library` field holds the library section's heading with its markers, for example `## shared_stuff`. The `section` coordinate is the section whose environment loads the library.
  - Nothing emits the events yet; Step 2's loader does.
- Test cases:
  - Each variant serializes to one JSON line tagged `lua_library_load_started`, `lua_library_load_succeeded`, or `lua_library_load_failed`, with `library` beside the coordinates, and round-trips exactly.
  - The emitter method stamps the run's coordinates and the library name.
  - The recording adapter forwards each variant to its observation with the library name.

</step-1>

<step-2>

### Step 2: Lua-side `require` loader [completed]

- Component: `require` runtime
- Tests: `cargo nextest run --locked -p runvil-lua -p runvil-engine --all-features`; `cargo fmt --all --check`

Add Runvil's `require` to the Lua crate as an installable global driven by a resolver that the caller supplies. The engine installs it in Step 5, so prompts see no change yet.

- Artifacts:
  - Library types beside `SectionText` in `crates/runvil-internal/lua/src/handles.rs`, exported from `lib.rs` (names proposed in the Technical Design): `LibraryKey`, opaque to this crate, cloneable and `Send + Sync`, with a stable string identity that the cache keys on, and a public constructor from that string plus an accessor, which Step 4's engine code uses to encode a section's position; `Library`, holding its `LibraryKey`, the section heading, and its blocks in order; and a block type holding one compiled `LuaProgram` and its prose template string.
  - The resolver contract, proposed `LibraryResolver`: a closure of the same kind as the prompt-read closure that `SectionVm::install_scheduler_control_globals` takes (`lua/src/vm/install.rs` lines 198-206). It takes a heading `String` and an `Option<LibraryKey>` and returns `Result<Library, E>`. An out-of-reach or malformed heading is the resolver's error.
  - New module `crates/runvil-internal/lua/src/require.rs` with the Rust helpers and the install, and a new embedded chunk `crates/runvil-internal/lua/src/__impl_require.lua`, loaded with `include_str!` under the chunk name `@crates/runvil-internal/lua/src/__impl_require.lua` and compiled once, like the chunks in `coro.rs`. `coro.rs` is at 486 of the 500-line ceiling, so the new code stays out of it.
  - A `SectionVm` method, proposed `install_require(resolver, emitter, section)`, that installs `require` as a global. Step 3 adds the prose renderer to it.
  - A library-load depth counter on `SectionVm` beside `local_handler_depth` (`lua/src/vm.rs`), stepped by the helper and read by the `jump` closure in `install_jump_global` (`lua/src/vm/install.rs` lines 225-245).
  - Tests in a new `crates/runvil-internal/lua/src/tests/require.rs`, wired in `lua/src/tests.rs`, built on the `shim_vm` helper and `SectionVm::start_block_coro` and `resume_block_coro`, with a fake resolver over blocks compiled by the `program` helper. Add the new chunk to the chunk-name checks with `assert_chunk_name_resolves`.
  - Sandbox tests in `lua/src/tests/sandbox.rs` (line 54) and `lua/src/globals-tests.rs` (line 208): `package` and the standard `require` stay absent, and on a VM with `install_require`, `require` is Runvil's, so `require 'os'` raises a heading error instead of loading anything.
- Behavior (from the Functional Specification):
  - `require` is a Lua function, so library blocks run inside the calling block's coroutine and can make suspending calls (`store`, model, and tool calls), which yield through the caller's coroutine.
  - `require '## heading'` asks the resolver for the library, loads each block with `LuaProgram::load`, and runs the blocks in order in the caller's environment. Globals a library defines stay visible to the caller after the call, and its `local` variables stay inside the library.
  - Return values: a block that returns a value stops the library there, and `require` hands back that first value, whatever its type, including a table, a function, a string, or `false`. A block with no `return`, or with `return nil`, lets the next block run. If no block returns anything, `require` hands back `true`.
  - Caching: a per-environment cache table keyed by the resolved library's `LibraryKey` identity, never by the heading text, so one heading resolved from two positions never collides. The first `require` of a library runs it, and later calls return the saved value, `false` included, and so do calls made from inside other libraries.
  - Failure: a library that raises while loading stays out of the cache, the error reaches the caller unchanged, and the next `require` runs it again. Globals it defined before the error stay defined, as in standard Lua.
  - Cycles: a loading marker in the cache detects a `require` cycle, which raises an error naming the sections in the cycle in order, for example `require cycle: ## a -> ## b -> ## a`.
  - Position: while a library loads, a `require` call passes that library's `LibraryKey` to the resolver, so a library pulls in its own nested helpers from its own position wherever it's required from. Every other `require`, including one a library function makes after the load, passes no key and resolves from the calling section.
  - `jump`: while any library loads, `jump` raises at the call with a message that names library loading, through the same kind of depth check that refuses `jump` in local tool handlers. A library function that calls `jump` after the load records the jump for the caller's section as usual.
  - The depth counter and the loading state are restored on every exit, errors and cancellation included, and the raised value passes through unchanged, so its position prefix still names the library block.
  - Events: each first load of a library emits `LuaLibraryLoadStarted`, then `LuaLibraryLoadSucceeded` or `LuaLibraryLoadFailed`, naming the library (Step 1). A cached return emits nothing, and a retry after a failure emits again.
  - Section blocks keep `scalar_return` (`lua/src/hardening.rs` line 204); `require` reads library return values in Lua.
  - `require` loads sections only, through the resolver. It never loads files or native modules, and the package library stays unloaded.
- Test cases:
  - Returns: a table, a function, a string, and `false` each come back; a first block with no `return` and a second that returns; `return nil` continuing to the next block; `true` when no block returns.
  - Caching: a second `require` doesn't rerun the library (a counter global stays at 1); a saved `false` comes back without a rerun; library B requiring already-loaded library A gets the saved value; two libraries whose nested `### helpers` share a heading each get their own helper.
  - A two-library cycle raises, naming both sections in order.
  - A library that raises on its first load runs again on the next `require` and succeeds once the fault is cleared, for example through a global flag.
  - Resolver calls: a `require` inside a loading library passes that library's key; a `require` from a library function after the load passes none.
  - `jump` during a load raises; a library function that calls `jump` later produces `LuaBlockResult::Jump` for the caller.
  - A library block's suspending call (for example the `call` shim) yields a request from the caller's block coroutine, and resuming it finishes the load.
  - Events: one Started and one Succeeded on a first load, naming the library; Started and Failed on a failing load; nothing on a cached call.

</step-2>

<step-3>

### Step 3: Library prose and error mapping [completed]

- Component: `require` runtime
- Tests: `cargo nextest run --locked -p runvil-lua -p runvil-parser -p runvil-engine --all-features`; `cargo fmt --all --check`

Give library blocks their own `prose`, and make library errors report the library's location and prompt-file line.

- Artifacts:
  - The renderer contract, proposed `ProseRenderer`: a closure taking a prose template and the `ProseState` snapshot and returning the rendered string, the same shape `SectionVm::install_lazy_prose` takes today, with the template as an argument. `SectionVm::install_require` gains it.
  - `crates/runvil-internal/lua/src/prose.rs`: a save and a restore of the `prose` slot in the `_G` guard's state (`globals::install_prose`, `lua/src/globals.rs` lines 48-50 and 110-116), proposed `prose::swap` and `prose::restore`, that keep the caller's memoized text. Nothing reads or restores the slot today, and each install starts empty.
  - `SectionVm` keeps the `LuaProgram`s of every library loaded into it (`lua/src/vm.rs`), and `SectionVm::block_failure` and `SectionVm::map_chunk_failure` (`lua/src/vm/run.rs` lines 280-301) map an error through the started block's program and those library programs.
  - Tests in `lua/src/tests/require.rs` or a topic sibling; once the `require` tests reach three files, they move into a `tests/require/` directory.
- Behavior:
  - Each library block gets a lazy `prose` built from the library's own text before it (the block's prose template from its `Library`), rendered on first read through the renderer, with `{{ }}` substitutions filled in from the caller's environment: the caller's `var`, `sys`, and globals at read time. Assigning `prose` still raises, and `{{ prose }}` is still rejected as recursive.
  - Each library block runs between `prose::swap` and `prose::restore`, and the restore runs on every exit, errors included. After `require` returns or raises, the caller's `prose` is exactly as it was, including text the caller already read: the memo survives, and nothing renders again.
  - Errors raised in library code, during the load or later when the caller invokes a library function, report the library's location and the line in the prompt file through `LuaProgram::map_runtime_error`.
  - `map_chunk_line_to_absolute` (`lua/src/program.rs` lines 243-295) rewrites every `[string "<location>"]:N:` marker for one location and prefixes a `location:line:` tag. Each marker must be rewritten exactly once, by the program that owns it: mapping through several programs must not rewrite an already mapped marker again or stack tags.
  - The middle blocks of one section share the location ``section `X` lua`` (`lua_block_location`, `parser/src/fence.rs` lines 321-336), so two library blocks can share a location with different base lines. Resolve that so each block's error maps to its own line, for example by giving every section block a distinct location and updating the tests that pin the old strings.
- Test cases:
  - A library block reads `prose` and gets its own text, with a `{{ var.x }}` substitution from the caller's `var`; the caller's `prose` after the call is unchanged.
  - The caller reads its `prose`, changes `var`, requires a library, and reads `prose` again: the same memoized text.
  - A library that raises during its load still restores the caller's `prose`.
  - An error during the load and an error from a library function called later both report the library block's location and its prompt-file line.
  - An error in the second of two middle library blocks maps to its own prompt line.
  - An error message holding markers from both a library block and the caller's block maps each marker once, under one leading tag.

</step-3>

<step-4>

### Step 4: Reach lookup and library construction [completed]

- Component: `require` in prompts
- Tests: `cargo nextest run --locked -p runvil-engine --all-features`; `cargo fmt --all --check`

Add the engine's lookup for `require`'s wider reach and the code that turns a parsed section into a `Library`. Section setup starts calling it in Step 5.

- Artifacts:
  - `crates/runvil-internal/engine/src/heading_address.rs`: a reach view beside `Visible` and a lookup beside `find_visible`, proposed `find_reachable`, plus a variant like `resolve_visible` whose not-found error lists only the reachable sections. Update the module doc to name `require` and its reach.
  - `crates/runvil-internal/engine/src/execute/walk_target.rs`: proposed `resolve_library`, which resolves a heading from an origin (a walked section's `SlicePath` and index, or the H1) and builds the `Library`, plus the mapping from a `LibraryKey` back to its section's position. Update the module doc, which says a running section addresses only its visible set.
  - Unit tests in `engine/src/heading_address-tests.rs` and a `walk_target` test file wired the same way.
- Behavior:
  - Reach: from a walked section, `require` reaches its direct children, its siblings, and the siblings of each of its ancestors, up to the top-level sections. From the H1, it reaches every top-level section. A nested worker can therefore require a top-level library. Siblings exclude the section itself, so the section and its ancestors are out of reach.
  - Uniqueness: a heading resolves by its exact `(level, name)` address, as `find_visible` does (`engine/src/heading_address.rs` lines 113-128). Each scope sits at its own level: direct children deeper than the section, siblings at its level, and each ancestor's siblings at that ancestor's shallower level. Sibling names are unique within a slice (`check_unique_sibling`, `parser/src/build.rs` line 271), so a heading matches at most one reachable section and needs no tie-break.
  - An unknown or out-of-reach heading is an error that lists the reachable sections and nothing else, and a malformed heading raises through `parse_heading_address`.
  - `call`, `jump`, `prompt.section`, `prompt.list`, `tasks.spawn`, and `tasks.fanout` keep `find_visible` and their narrower reach: from the H1, every top-level section; from any other section, its siblings and its direct children.
  - Library construction: the library's blocks are its section's `Block::Lua` programs in order, each paired with the `Block::Prose` text right before it, or with an empty template when no prose block comes right before it. The parser already accumulates prose and resets it at thematic breaks, as the scheduler relies on (`engine/src/execute/scheduler/step.rs` lines 93-98). The `Library` heading is the section's heading with its markers.
  - Keys: each `Library` gets a `LibraryKey` that identifies its section's position uniquely, and a lookup given a key resolves from that library's position instead of the caller's.
- Test cases:
  - From a nested section, a direct child, a sibling, a sibling of the parent, a sibling of a grandparent, and a top-level section all resolve.
  - A grandchild, a niece or nephew, the section itself, and its parent don't resolve.
  - The H1 reaches every top-level section and no nested one.
  - A heading whose name matches a reachable section at a different level doesn't resolve.
  - A not-found error lists only the reachable sections.
  - A library built from a section laid out as prose, Lua, Lua, prose, Lua pairs each program with the right template, the second Lua block getting an empty one.
  - A key maps back to its section's position, and a keyed lookup resolves from the library's reach.

</step-4>

<step-5>

### Step 5: Install `require` in section setup [completed]

- Component: `require` in prompts
- Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; `node --test crates/workshop/ui/test/docs-claims.mjs`; `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`; `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`; `cargo fmt --all --check`

Make `require` available in every section's Lua environment, the H1 included, and pin its behavior with whole-prompt tests. This ends the first work item: `require` exists alongside `lua shared`.

- Artifacts:
  - `crates/runvil-internal/engine/src/execute/section_vm.rs`: `SectionVmSetup` and `setup_section_vm` take the library resolver and the prose renderer, and setup calls `SectionVm::install_require` right after `install_preludes`, before the shared replay (which Step 7 removes) and before `install_store_shims`.
  - `crates/runvil-internal/engine/src/execute/section_context-construct.rs`: `SectionContext::new` builds the resolver over its section's origin (its `SlicePath` and index) with Step 4's `resolve_library`, and `SectionContext::new_live_h1` over the H1's. Both build the renderer from the inputs `SectionContext::install_lazy_prose` feeds `subst::Sources` (`engine/src/execute/section_context.rs` lines 172-191): the run's `args` and `argv` and the frame's `item`.
  - `crates/runvil-internal/engine/src/lua.rs`: re-export the new Lua crate items the engine names.
  - Remove Step 4's temporary `cfg_attr(not(test), expect(dead_code, ...))` markers once setup calls the code: in `engine/src/heading_address.rs` on `Reachable`, `Reached`, `impl Reachable`, `find_reachable`, and `resolve_reachable`; in `engine/src/execute/walk_target.rs` on `RequireOrigin`, `impl RequireOrigin`, `library_key`, `resolve_library`, and `build_library`. Any marker whose item stays unused outside tests points to missing wiring.
  - `crates/runvil-internal/lua/src/globals.rs`: add `("require", Reserved::EngineGlobal)` to `RESERVED_NAMES`, whose array length becomes 61. The engine tests in `engine/src/lua/tests/globals.rs` compare the list with a fully set-up VM in both directions, so their `fully_bound_vm` helper passes a resolver and a renderer.
  - Whole-prompt tests in new files under `crates/runvil-internal/engine/src/execute/tests/suite/`, for example `require.rs` with topic siblings, or a `suite/require/` directory at three or more files, wired in `suite.rs`. They use `run_fixture` from `suite/support.rs`, plus the scripted gateway and fixture tools other suite tests use.
- Behavior:
  - Every section VM gets `require` with its own reach (Step 4) and its own cache: the H1, walked sections at any depth, spawned tasks, and `tasks.fanout` arms. Requires aren't inherited, so a child section's environment starts without its parent's libraries.
  - A Plugin prelude can't define a global named `require`, since `check_collision` (`lua/src/prelude.rs` lines 177-203) rejects reserved names. A prelude that calls `require` while it installs raises with a message naming `require` and the prelude phase, since preludes run before any block starts.
  - Library code runs after the Plugin preludes, so it can use what they define.
  - The parser leaves `require` calls alone, so a misspelled heading parses and raises when the call runs.
  - A required section stays an ordinary section. When the walk reaches a library, it runs under the section rules: a globals-only library runs harmlessly, a returned table fails the run, and a returned scalar ends the chain with that value. Two placements keep a returning library away from the walk: after the section that returns, or nested under the section that uses it.
- Test cases (each a whole prompt):
  - A section requires a sibling library and uses the table of functions it returns: `local shared = require '## shared_stuff'`, then `shared.search_all(args.topic)`.
  - A nested `tasks.fanout` worker, such as a `### Worker` under `## Research`, requires a top-level library.
  - A section requires a library twice, and the second call returns the saved value without rerunning it, including a saved `false`.
  - A `## house_style` library whose prose ends in `return prose` hands its text to the caller, with `{{ }}` substitutions from the caller's environment, and the caller's `prose` is unchanged afterward, including after a library error and including text the caller already read.
  - Library code makes `store`, model, and tool calls while it loads.
  - The H1 requires a library that sets prompt-wide state (`tools.always_offer` and `models.default`), and later sections see it without requiring the library.
  - The walk reaching a library: a globals-only library runs harmlessly, a returned table fails the run, and a returned scalar ends the chain.
  - Errors in library code report the library's location and prompt-file line, both during the load and when a library function runs later.
  - A library's first load records `LuaLibraryLoadStarted` and `LuaLibraryLoadSucceeded` naming it, and a failing load records `LuaLibraryLoadFailed`.
  - A child section entered from a parent that required a library starts without it, and its own `require` loads it.
  - A cycle, an out-of-reach or misspelled heading, a `jump` during a load, and a `require` from a Plugin prelude each raise; a library that raises while loading runs again on the next `require`.
  - A library's nested `require` during its load finds the library's own child helper, while a library function's later `call`, `prompt.section`, and `require` resolve from the calling section.
  - From a nested worker, `require '## house_style'` works where `prompt.section '## house_style'` raises.
  - Load-time library code runs once in each requiring section: two sections that each require one library each run its load-time code once.

</step-5>

<step-6>

### Step 6: Move tests and fixtures to `require` [completed]

- Component: `lua shared` retirement
- Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; `node --test crates/workshop/ui/test/docs-claims.mjs`; `cargo fmt --all --check`

Move every test and fixture that uses the `lua shared` fence for some other purpose onto `require`. This ends the second work item. Tests whose subject is the replay mechanism itself, which call `SectionVm::replay_shared` or read the prompt's replay program directly, stay for Step 7 to remove. No shipped prompt under `prompts/` or `crates/workshop/agents/agents/` uses `lua shared`. The separate `runvil-docs` repository is out of scope and gets its own update.

- Rewrite rules:
  - Each H1 `lua shared` block becomes a library section, required by each section that uses it, or by the H1. Place it where the walk can't end the run: a globals-only library anywhere, and a returning library after the section that returns or nested under the section that uses it.
  - Shared setup that set prompt-wide state, such as `models.default` or `tools.always_offer`, moves to a library the H1 requires.
  - A test named for the replay that has a `require` counterpart is rewritten as that counterpart and renamed:
    - the shared chunk seeing a prelude global becomes a library seeing it;
    - the shared replay seeing a fanout arm's `item` becomes a library required in an arm seeing `item`;
    - a shared replay failure in the H1 keeping its Lua kind becomes a failing library required from the H1 that ends the run as `RequirementsUnmet`, with the library's message as the notice. The library runs inside the H1's chunk, and an uncaught H1 chunk error is the failed assertion (`engine/src/execute/scheduler/step.rs` lines 192-207); only the replay ran outside the chunk. Production error classification stays as it is;
    - the shared library calling Engine globals at load becomes a library doing so;
    - prompt reads in the shared library become prompt reads in a library, resolving from the calling section;
    - the shared library's `_G` metatable and prose-assignment cases become library cases;
    - a shared-library store write across two arms becomes a library write across two arms, which still ends the run with a determinism violation.
  - Doc comments in these tests that describe the replay move to `require`.
- Artifacts, all engine tests under `crates/runvil-internal/engine/src/execute/` unless noted: `tests.rs`, `tests/context.rs`, `tests/debug_and_counts.rs`, `tests/debug_and_counts-infer-rounds.rs`, `tests/live_infer.rs`, `tests/model_and_reply.rs`, `tests/model_and_reply-globals.rs` (the `models.default` call at lines 45-46), `tests/preludes.rs`, `tests/scheduler/fanout/arms.rs`, `tests/scheduler/live_h1/pass.rs`, `tests/suite/exec_flow/prompt_reads.rs`, `tests/suite/exec_flow/store_failures.rs`, `tests/suite/global_metatable.rs`, `tests/suite/lazy_prose.rs`, and `tests/suite/parsing.rs`; plus the fixture `crates/runvil-internal/engine/tests/prompts/valid/shared-library.md`, which becomes a `require` fixture renamed with `git mv` (for example to `valid/require-library.md`), with `verify_shared_library` in `tests/suite/parsing.rs` following it.
- Done when: a search for `lua shared` outside `vibe/` hits only the replay implementation and its own tests: the parser and its shared-fence tests under `parser/src/tests/`, `lua/src/tests/shared_replay.rs`, implementation doc comments, and engine tests that drive the replay directly. Step 7 removes all of them. `vibe/` holds dated plan records, including this plan's own seed copy, and stays as written.

</step-6>

<step-7>

### Step 7: Remove `lua shared` [completed]

- Component: `lua shared` retirement
- Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; `node --test crates/workshop/ui/test/docs-claims.mjs`; `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`; `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`; `cargo check -p gateway --no-default-features`; `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`; `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`; `cargo +nightly-2026-09-05 xtask api --check`; `cargo fmt --all --check`; `rg -n --hidden -g '!target' -g '!**/node_modules/**' -g '!.git' -g '!vibe/**' 'lua shared'`

Remove the shared replay chain, its events, and the fence, and reject the fence with a parse error that points to `require`. Every link in the parser-to-engine chain goes in this one step, since each crate's change depends on the next. This ends the third work item and the plan's work.

- Parser:
  - `fence::split_h1` (`parser/src/fence.rs` lines 30-77) stops extracting a shared fence and stops returning a replay program.
  - `parse.rs` lines 131-145 replace the "at most one" and "allowed only in H1" errors with one error for any exact `lua shared` opening anywhere in the prompt, located at the fence as today's `Error::parse_at` call is. Proposed text: "the `lua shared` fence form was removed; put shared code in a section and load it with `require '## heading'`".
  - The `lua prompt` removal error (`fence.rs` line 49) stops suggesting `lua shared`. Proposed text: "the `lua prompt` fence form was removed; use `lua` for a live H1 block and `require` for shared code". Keep the fragment "`lua prompt` fence form was removed", which `crates/runvil/tests/suite/parsing.rs` checks against `crates/runvil/tests/prompts/invalid/removed-lua-prompt.md`.
  - Remove `Prompt.replay` and its accessor (`parser/src/prompt.rs` lines 138-165) and `detail::replay` (`parser/src/detail.rs` lines 18-21). Replace the shared-fence tests in `parser/src/tests.rs`, `tests/h1_fences.rs`, `tests/breaks.rs`, and `tests/line_mapping.rs` with tests of the new error, and update the docs in `parser/src/lib.rs`, `parser/src/error.rs`, and `parse.rs`.
- Engine:
  - Remove `RunState.shared` and its read through `detail::replay` (`engine/src/execute/run.rs` lines 256-259; `engine/src/execute/context.rs` lines 89-91, 146, 178, and 377-393), `SectionVmSetup.shared`, and the `replay_shared` call in `setup_section_vm`.
  - Keep the `take_store_conflict` check after `install_preludes`: a prelude's store call while it installs still reaches the direct store closures. Update its comment and the store-shim ordering comment, which explain both by the replay.
  - Remove the empty shared library from test helpers (`tests/effects.rs`, `tests/models_loop.rs`, `tests/scheduler.rs`, `engine/src/lua/tests/globals.rs`, and others the compiler finds), and delete the tests that drive the replay directly.
  - Update the docs that describe the replay: `execute/section_vm.rs`, `execute/section_context-construct.rs`, `execute/config.rs` line 99, `execute/environment.rs` line 71, `engine/src/parser.rs` line 4, and the H1 error-remap comment in `execute/scheduler/step.rs` (lines 192-201), which names "the shared replay" as machinery outside the chunk without the literal `lua shared`.
- Lua crate:
  - Remove `SectionVm::replay_shared` (`lua/src/vm/run.rs` lines 15-51) and the test helper `section_vm_with_shared` (`lua/src/tests.rs`).
  - Delete `lua/src/tests/shared_replay.rs`, moving its tests that aren't about the replay, such as `call` with a non-string target, to `tests/section_vm.rs`. Tests that used the replay to run code (`tests/section_vm.rs`, `store_reports.rs`, `store_errors.rs`, `budgets.rs`, `sandbox.rs`, `tool_scoping.rs`, `models-tests.rs`, and `globals-tests.rs`) switch to `run_chunk` or a block coroutine.
  - Update the comments that describe the replay: `tools.rs` lines 271-273 and `models.rs` lines 226-230 (which call `tools.always_offer` and `models.default` safe to repeat under the replay), `coro.rs`, `__impl_coro.lua`, `__impl_store.lua`, `engine_globals.rs`, `engine_globals-store.rs`, `error-value-store.rs`, `vm.rs`, `vm/install.rs`, `program.rs`, and `lib.rs`.
- Events:
  - Remove `Event::LuaSharedLoadStarted`, `LuaSharedLoadSucceeded`, and `LuaSharedLoadFailed` (`types/src/event.rs` lines 206-210) and their constants (`types/src/event-lifecycle.rs` lines 58-60), with their uses in `types/src/event-tests.rs`, `engine/src/test_support/recording/observation.rs`, `forward.rs`, `forward-tests-variants.rs`, and `lua/src/tests-recording.rs`.
  - Every section emitted the shared-load pair, even for an empty program, so observation sequences that list it (for example in `engine/src/execute/tests/observations.rs`) drop the pair; a test that checked the load events themselves moves to a `require` prompt and the library-load events.
  - Regenerate `crates/runvil/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`.
- `crates/build-xtask/src/engine_guards.rs`: add the removed Engine symbols to `RETIRED_SEEDS` and adjust its array length, so the retired-symbol scan fails if live Engine source names them again: `replay_shared`, `section_vm_with_shared`, `LuaSharedLoadStarted`, `LuaSharedLoadSucceeded`, `LuaSharedLoadFailed`, `LUA_SHARED_LOAD_STARTED`, `LUA_SHARED_LOAD_SUCCEEDED`, and `LUA_SHARED_LOAD_FAILED`. Add distinctive names only, since a generic word such as `replay` would match unrelated code.
- Test cases:
  - A `lua shared` fence in the H1 fails to parse with an error that points to `require`, and so does one in an H2.
  - The `lua prompt` fence error points to `lua` and `require` and no longer mentions `lua shared`.
  - Every test that used `lua shared` passes on `require`, and tests that read the shared-library load events read the library-load events.
- Exit criteria (Testing Plan): the whole suite passes, and the final search for `lua shared` outside `vibe/` hits only the parse error that rejects the fence and that error's tests.

</step-7>

</execution-plan>
