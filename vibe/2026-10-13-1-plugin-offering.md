---
name: Plugin offering
overview: Stage 1 of the late-bound Plugin offering design. Adds `plugins.offer`, which offers a whole Plugin to the section it's called in; retires `tools.always_offer` and the offer description override, since shared offers now come from a section each section requires, and with them the lock on the run's tool set, which nothing writes anymore; and makes every usable installed Plugin visible to Lua, with an offer of an unavailable one raising its reason. Rendering, listed tools, and name clashes stay for stage 2.
todos:
  - id: plugin-offer
    content: "Step 1: plugins.offer over a name, a Plugin object, or a list of either; ToolRuntime deriving Default and gaining plugins; current_tool_bindings expands offered Plugins each round; an absent Plugin raises; tests"
    status: completed
  - id: retire
    content: "Step 2: retire tools.always_offer, ToolSet.always, and the description override; with no writer left, the tool set becomes one read-only Arc<ToolSet>, deleting ToolView, the lock helpers, from_parts, for_test, and tool_set_snapshot, with RunState::with_tool_set for test fixtures; Engine test fixtures lose their run-wide list and their tests offer explicitly; move every other test and the Workshop chat agent onto plugins.offer, tools.offer, or a required offers section; docs; RETIRED_SEEDS; full suite"
    status: completed
  - id: roster
    content: "Step 3: the run's Plugin roster reaches Lua: Environment::plugins folds usable and unavailable Plugins into one map that Environment, RunContext, and ToolSet hold; the Harness passes it from its snapshot; a Plugin object for every usable Plugin; plugins.offer of an unavailable Plugin raises its reason; public API blessed; full suite"
    status: completed
  - id: close
    content: "Close, owned by the run and not a step: mark the vibe/ copy completed, delete vibe/ACTIVE, and commit Close plan: plugin-offering; nothing is pushed"
    status: completed
isProject: false
---

# Plugin offering

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\runvil` (git). Paths are relative to it unless they start with `../` or a drive letter.
- Baseline: commit `ba4a63beb` (Fix broken and redundant doc links in the Lua crate), after `Close plan: require-run-debt-removal` (`fb84079f1`). Citations were checked at `ba4a63beb`; search by name if a line has moved.
- This plan is stage 1 of the late-bound Plugin offering design (`C:\Users\Vinnie\.cursor\plans\late-bound_plugin_offering_d2575252.plan.md`), which covers stage 2: model-shaped rendering, listed and unlisted tools, and name clashes. This plan ships only the offering part, built so stage 2 adds to it without undoing any of it.
- It follows the `require` plan, which landed (`vibe/2026-10-12-1-require-replaces-lua-shared.md`): a required section runs in the caller's own environment, so a `plugins.offer` inside it applies to the section that requires it.
- Shell: PowerShell on Windows. Set an environment variable for one command with `$env:NAME='value'; <command>`.
- Before editing, read the root `AGENTS.md` and the `AGENTS.md` of each crate a step touches; the Engine crates `crates/runvil-internal/lua`, `engine`, and `types` each have one.
- Terms from `AGENTS.md`: the Engine runs prompts and performs no I/O; the Harness steps it and performs its effects; the Host is the application running prompts; a Plugin is a package of tools installed under a local name such as `web`, and every tool it offers sits under that name (`web/fetch`).
- Terms used here:
  - *section entry*: one entry into a section, by falling into it, `jump`, `call`, or a task worker starting there, and the H1 pass too. Each entry gets a fresh Lua VM, and with it a fresh section scope.
  - *wire name*: the name the model sees and calls: a tool id with `/` and `.` replaced by `_` (`web/fetch` becomes `web_fetch`).
  - *round*: one model request inside `models.loop`. The tools a round *advertises* are the ones that request shows the model, recorded in `EffectRecord::Chat.tools`.
  - *the run's roster*: what the Harness's snapshot says about each installed Plugin for this run: usable, or unavailable with a reason (`crates/harness-internal/runner/src/host-run.rs` lines 34-46).
  - *a Plugin in the run*: a Plugin the frontmatter declares, or a Plugin with a tool in the run's offering, as today, and from Step 3 also every usable Plugin on the roster.
- Plan record and commits:
  - When execution starts, copy this plan to `vibe/` as `2026-10-13-1-plugin-offering.md`, or the next free number for the day the run starts. Create `vibe/ACTIVE` naming it. Mark each step `[completed]` in that copy when it lands.
  - Each commit message has a one-line summary, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer.
  - Close with a commit `Close plan: plugin-offering` with the same trailer. Nothing is pushed.

<product-contract>

## Product Requirements

Authors can offer a Plugin's tools only one at a time or through its object's `tools` list, prompt-wide offers come from `tools.always_offer`, and a Plugin with no tools that the prompt doesn't declare is invisible to Lua. This plan adds `plugins.offer`, which offers a whole Plugin to the section it's called in, retires `tools.always_offer` and the offer description override, and makes every usable installed Plugin visible to Lua. Shared offers come from a shared section that each section requires.

- Problem and users:
  - Prompt authors offer a Plugin with `tools.offer(plugins.get('web').tools)` or by naming each id. There is no way to say "this Plugin, whole".
  - Prompt-wide offers today come from `tools.always_offer`, which writes a run-wide list from any section (`crates/runvil-internal/lua/src/tools.rs` lines 253-282). With `require` landed, a section gets shared offers by requiring a shared section that offers them, so the run-wide list has no remaining job.
  - A Plugin that ships no tool descriptors, such as the native `runvil/subagents` package Workshop installs, gets a Lua object only when the prompt declares it (`crates/runvil-internal/lua/src/plugins.rs` lines 1-9 and 134-150). Stage 2's subagents Plugin needs authors to reach it with `plugins.offer('subagents')`.
  - An installed Plugin can be unavailable for a run, such as an MCP Plugin whose server failed to start. A declared one fails the run at start (`crates/harness-internal/runner/src/host-run.rs` lines 130-153); an undeclared one is silently absent today.
  - Engine maintainers: stage 2 needs Plugin offers recorded by Plugin name and expanded each round, so it can render each offered Plugin's catalog at request time.
- Non-goals:
  - Model-shaped rendering, listed and unlisted tools, and name-clash numbering, which stage 2 adds.
  - Withholding a Plugin or a tool.
  - Prompt-wide offers of any kind, and inheritance of offers by child sections.
  - Method forms on Plugin objects, such as `plugins.get('web').offer()`.
  - Offer options, such as `{ write = false }`.
  - Any change to `tools.offer`'s errors, or to how local tools from `tools.offer_local` shadow catalog tools; stage 2 changes both.
- Success criteria:
  - `plugins.offer('web')` in a section makes that section's rounds advertise every `web` tool, and leaves every other section entry's rounds alone.
  - A shared section that calls `plugins.offer('web')`, required by two sections, makes both of their rounds advertise `web`'s tools, and a third section that doesn't require it advertises none.
  - `tools.always_offer` is no longer installed, and `tools.offer(tool, 'text')` raises.
  - The Workshop chat agent offers web tools with `plugins.offer('web')` in its `## Conversation` section.
  - `plugins.get('subagents')` returns a Plugin object in a Workshop run whose prompt doesn't declare it, and `plugins.offer('subagents')` succeeds and adds nothing to a round, since the stub has no tools.
  - `plugins.offer` of an unavailable Plugin raises an error naming the Plugin and its reason.
  - Every exit gate in the Testing Plan passes.
- Constraints:
  - The Engine stays sans-IO and deterministic. The order of a round's tools is recorded in `EffectRecord::Chat.tools`, so scope resolution walks ordered lists only.
  - The Engine crates don't depend on `runvil-plugin` or any Harness crate; the roster reaches the Engine as plain Plugin names and reasons through `Environment`.
  - Every `.rs` file stays under 500 lines (`build_ceiling::check()`). `crates/runvil-internal/engine/src/execute/context.rs` is at 453 lines, `lua/src/plugins.rs` at 154, `lua/src/plugins-tests.rs` at 216, `lua/src/tools.rs` at 383, and `engine/src/execute/tests/offering.rs` at 380.
  - Error text is written for a model to read: concise and self-contained, naming what was required and what was found.
  - Engine crates name the code that steps a run "the caller" and never mention a Host. Plugin is capitalized, and Plugin-facing docs say installed, snapshotted, or declared.
  - Breaking existing prompts is acceptable. The only shipped prompt that changes is the Workshop chat agent (`crates/workshop/agents/agents/chat.md` line 23).
- Open questions: None

## Functional Specification

`plugins.offer` takes one Plugin or a list and returns nothing. It offers each Plugin whole to the section entry it's called in, so a call inside a library applies to the section that requires the library. Each round's tools are resolved when the round is built, from the section entry's tool offers and Plugin offers, with each offered Plugin standing for all of its tools. `tools.always_offer` and the offer description override are gone. Every usable installed Plugin has a Lua object, and an unavailable one raises at the offer with its reason.

- Actors and workflows:
  - A prompt author offers a Plugin whole in one section: in `## Research`, `plugins.offer('kit')`, and that section's rounds advertise every `kit` tool.
  - Shared offers live in a shared section: `## offers` calls `plugins.offer('web')`, and `## Research` and `## Draft` each run `require '## offers'`, so both advertise `web`'s tools, while `## Summary`, which doesn't require it, advertises none of them.
  - In the H1, `plugins.offer('web')` covers the H1's own rounds, and the sections after it start with no offers.
  - An optional Plugin, missing or unavailable: `if plugins.get('mcp') then plugins.offer('mcp') end`.
  - Per-tool offers keep working: `tools.offer('web/fetch')`, `tools.offer(plugins.get('web').tools)`, `tools.offer(tools.extras())`.
- Inputs and outputs:
  - `plugins.offer(plugin)`, where `plugin` is a local name such as `'web'`, a Plugin object (from `plugins.get`, `plugins.required()`, `plugins.extras()`, or a tool object's `plugin`), or an array of names and objects, such as `plugins.required()`. It returns nothing and takes no options.
  - `tools.offer(tool)` takes one tool id or tool object, or an array of them, and no second argument.
  - `tools.always_offer` is no longer installed.
  - `plugins.get(name)` returns an object for every Plugin in the run, and `nil` for one that's absent or unavailable. `plugins.required()` lists the declared Plugins and `plugins.extras()` every other Plugin in the run, both in name order as today, so `plugins.extras()` now includes usable undeclared Plugins with zero tools.
  - Unchanged: `tools.offer_local`, `tools.call`, `tools.calls`, `tools.allow_tasks`, `tools.get`, `tools.required()`, `tools.extras()`, and tool descriptors.
- States and validation:
  - A round's bound tools, resolved each time a round is built:
    - first the section entry's `tools.offer` tools, in first-offer order
    - then the tools of each `plugins.offer` Plugin, Plugins in first-offer order and each Plugin's tools in id order
    - each tool appears once, at its first place
    - local tools and task built-ins follow as today, and a local alias still wins over a bound tool with the same wire name
  - `plugins.offer` lasts for the section entry, as `tools.offer` does, and offering a Plugin again is a no-op. A section `call`ed, a task spawned, a child section, and the next section each start with no offers.
  - A Plugin with no tools, such as one that only has a prelude or the `runvil/subagents` stub, is accepted and adds nothing to a round.
  - `plugins` is already a reserved Engine global, so the reserved-global list stays at 61 names (`crates/runvil-internal/lua/src/globals.rs` line 168).
- Errors and recovery:
  - A wrong argument (nil, no argument, a number, a tool object, an array holding one, or a string that isn't a valid Plugin name) raises: `plugins.offer takes a Plugin name, a Plugin object, or a list of them, got nil`, or `... got "web/fetch"`.
  - An extra argument raises: `plugins.offer takes one Plugin or a list of them, got extra string`, so `plugins.offer('web', 'kit')` can't silently offer only `web`.
  - A name with no Plugin in the run raises: `plugins.offer: no Plugin named "mcp" in this run; declare it under plugins: to require it, or offer it only when plugins.get("mcp") returns it`.
  - From Step 3, an unavailable Plugin raises with its reason: `plugins.offer: the mcp Plugin is unavailable in this run: <reason>`.
  - Every entry of a list is checked before any is recorded, as `tools.offer` already does.
  - `tools.offer` with a second argument raises: `tools.offer takes one tool or a list of them, got extra string`.
  - `tools.offer` of an id the run can't offer keeps today's `is not a catalog tool in this run`, and a script `tools.call` keeps today's errors.
  - A call to `tools.always_offer` fails as a call to any missing field does.
- Security and privacy behavior:
  - Offering a Plugin grants nothing a per-tool offer of each of its tools couldn't: every tool still runs under the same rules, and its call and result are reported and recorded the same way. The existing round gate in `crates/runvil-internal/engine/src/execute/scheduler/chat.rs` still refuses a model call to a name its round didn't advertise.
  - Undeclared usable Plugins' tools are already in the catalog and offerable one by one today, with no frontmatter consent gate. The roster adds Lua objects for Plugins with zero tools and the reasons of unavailable ones, which are already written for the model that reads them (`crates/runvil-internal/engine/src/execute/requirements.rs` lines 150-158).
- Acceptance criteria:
  - Every behavior above has a test in the Testing Plan, and every exit gate passes.

</product-contract>
<implementation-contract>

## Technical Design

Plugin offers are recorded as Plugin names, never expanded into tool names when they're made, so stage 2 can render each offered Plugin's catalog when a round is built. The scope resolution that already runs as each round is built expands the offered Plugins. Once `tools.always_offer` is gone, nothing writes the run's tool set, so it becomes one read-only shared value and its lock, its view trait, and the copy every read makes all go. The Harness then passes the run's roster to the Engine through `Environment`, the same way it passes the catalog and preludes, and the tool set holds it as a plain field.

- Architecture:

  ```mermaid
  flowchart LR
    harness["Harness snapshot"] -->|"roster"| env["Environment"]
    env -->|"prepare"| ctx["RunContext"]
    ctx --> toolset["ToolSet"]
    toolset --> objects["plugins table"]
    sectionTools["section tools"] --> scope["round scope"]
    sectionPlugins["section Plugins"] -->|"all tools"| scope
    scope --> locals["plus local tools"]
  ```

  - Where the records live: the section entry's Plugin offers sit on the section VM's `ToolRuntime` beside `added`, and the roster sits on the run's `ToolSet` beside `declared` and `offered`. Every section entry builds a fresh VM with a fresh `ToolRuntime`, which gives an offer its section-entry reach with no extra rule. After Step 2, no run-wide offer list remains.
  - Why per round: `current_tool_bindings` (`crates/runvil-internal/lua/src/vm/state.rs` lines 183-196) already rebuilds the bound half of the scope for every round and every tool call, so Plugin expansion added there reaches both the round's schemas and the section's call counts with no other change.
  - Why the tool set becomes read-only: `tools.always_offer` is the set's only writer (`crates/runvil-internal/lua/src/handles.rs` lines 278-284). The set sits behind a lock and a read-only view trait, `ToolView`, and every read clones it whole, every binding and schema included (`crates/runvil-internal/engine/src/execute/context.rs` lines 279-291), on each round (`scheduler/chat.rs` line 147), each name check of a model's tool call (`chat.rs` line 270), and each tool call (`scheduler/tool_call.rs` lines 210 and 291). Once the writer goes, one `Arc<ToolSet>` serves every reader with no lock and no copy.
  - Why through `Environment`: the Engine learns the catalog and the preludes from `Environment::tools` and `Environment::preludes`, which `prepare` copies onto the `RunContext` (`crates/runvil-internal/engine/src/execute/environment.rs` lines 51-84 and 107-114), and the Harness builds that environment from its per-run snapshot in `begin_run` (`crates/harness-internal/runner/src/host.rs` lines 197-200). The snapshot already knows each installed Plugin's state (`crates/harness-internal/runner/src/host-run.rs` lines 34-46 and 56-89).
- Modules and interfaces, by step. Names marked *proposed* don't exist yet.
  - Step 1, `plugins.offer`:
    - `crates/runvil-internal/lua/src/scope.rs`: `ToolRuntime` (lines 106-119) derives `Default` and gains `plugins: Vec<PluginId>`. Its six struct literals, which hold only default values, become `ToolRuntime::default()`: `lua/src/vm.rs` lines 281-285, `lua/src/tools/tests.rs` lines 207-211, `lua/src/plugins-tests.rs` lines 56-60, `lua/src/protocol/tests/answer.rs` lines 171-175, and the Engine's `execute/tests/tool_scoping.rs` lines 27-31 and 51-55. Each `BTreeMap` import that becomes unused goes too, or clippy fails.
    - `crates/runvil-internal/lua/src/plugins.rs`: `install_plugins` (lines 82-132) takes the section's `ToolRuntime`, which its call in `lua/src/vm/install.rs` (line 90) passes from `self.tool_runtime`, and installs `plugins.offer`. One decode, *proposed* `plugin_names`, raises on an extra argument, walks an array, reads a string through `PluginId::parse` and `plugin_object` (lines 64-72), reads a `LuaPluginHandle` (lines 26-31) as its name, and checks every entry before anything is recorded. The verb pushes each Plugin not already in `runtime.plugins`. If `plugins.rs` passes about 450 lines, the verb moves to a sibling `plugins-offer.rs` wired with `#[path]`.
    - `crates/runvil-internal/lua/src/vm/state.rs`: `current_tool_bindings` appends, after the section's `added` names, the offered bindings of each runtime `plugins` Plugin, filtering `offered` by each binding's Plugin, so Plugins come in first-offer order and each Plugin's bindings in id order, each wire name once at its first place. Its doc (lines 171-182) states the order. Until Step 2, the run-wide `always` names still come first.
  - Step 2, retirement and the read-only tool set:
    - `crates/runvil-internal/lua/src/tools.rs`: `install_always_offer` (lines 253-282) and its call in `install_tools` (lines 199-211) go. `install_offer` (lines 214-251) drops the override and the check that skips a tool already offered prompt-wide. `lock_tools` (lines 73-76) goes. The module doc (lines 1-19) and `install_offer`'s doc follow.
    - `crates/runvil-internal/lua/src/tools/decode.rs`: `OfferEntry` (lines 46-49) loses `description_override`, or the decode returns names, and `collect_offer_entries` (lines 71-117) raises the extra-argument text on any second argument in place of today's three override errors. Its docs (lines 3-4 and 66-69) follow, and so does `lua/src/tools/userdata.rs` lines 6-8.
    - `crates/runvil-internal/lua/src/scope.rs`: `ToolRuntime` loses `description_overrides`.
    - `crates/runvil-internal/lua/src/handles.rs`: `ToolSet` (lines 195-206) loses `always` and its accessor (lines 244-248). `for_test` (lines 208-221) and `from_parts` (lines 223-236) go, since every field is public and the set derives `Default`; their callers build `ToolSet { declared, offered, ..ToolSet::default() }`: `from_parts` at `engine/src/execute/context-bound.rs` line 23, `context.rs` line 286, and `lua/src/handles-tests.rs` line 57, and `for_test`'s 12 calls in `lua/src/tools/tests.rs`, `lua/src/tools/tests-offering.rs`, `lua/src/tests.rs`, `lua/src/tests/tool_scoping.rs`, `lua/src/plugins-tests.rs`, `lua/src/handles-tests.rs`, and the Engine's `engine/src/lua/tests.rs`, `engine/src/lua/tests/globals.rs`, `engine/src/execute/tests.rs`, `engine/src/execute/tests/context-tools.rs`, and `engine/src/execute/tests/tool_call_arm.rs`. `ToolView` (lines 278-305), `lock_tool_set` (lines 307-312), and the trait's impl (lines 314-326) go, and `lua/src/lib.rs` line 170 and the Engine's `engine/src/lua.rs` line 20 stop exporting `ToolView`. `ToolBinding` loses `model_description` (line 44), its getter (lines 108-109), and its `None` in `from_descriptor` (line 63).
    - The set's holders take `Arc<ToolSet>` in place of `Arc<Mutex<ToolSet>>`:
      - production: `RunState::new` builds the set (`engine/src/execute/context.rs` line 139); section setup hands `ctx.tool_set()` to each section VM and to the H1's (`engine/src/execute/section_context-construct.rs` lines 99 and 191); `SectionVm`'s `bound_tools` (`lua/src/vm.rs` line 63, its default at line 279, and `new_for_section` at lines 331-337); `install_tools` (`lua/src/tools.rs` lines 199-211) and `install_offer` (line 221); `install_tool_lists` (`lua/src/tools/objects.rs` lines 62-72); `install_plugins`; and the calls in `lua/src/vm/install.rs` lines 83-90. `install_tool_objects` (`objects.rs` line 42) already takes `&ToolSet`. `install_tool_call_counts` (`lua/src/vm/state.rs` lines 102-109) and the test-only `tool_bag_handles` (lines 119-127) read the set without a lock.
      - tests and benches: `shared_set` and `section_vm_with_set` (`lua/src/tests.rs` lines 243 and 248), `lua/src/protocol/tests/answer.rs` line 170, `lua/benches/surface.rs` line 43, `engine/src/lua/tests.rs` line 96, `engine/src/lua/tests/globals.rs` line 93, `engine/src/execute/tests.rs` lines 58 and 130, and `engine/src/execute/tests/tool_scoping.rs` line 103.
    - Two test helpers replace the whole set through the lock after `RunState::new`: `FixtureTools::install` (`engine/src/execute/tests/context-tools.rs` lines 60-65) and `engine/src/execute/tests/model_tasks.rs` lines 69-71. They switch to a test-support method, *proposed* `RunState::with_tool_set(self, set: ToolSet) -> RunState`, which replaces the shared set before any section VM exists, and the fixtures' callers, such as `loop_context_observed` (`engine/src/execute/tests/models_loop.rs` lines 55-73) and `arm_tools_scoped` (`context-tools.rs` lines 95-102), build the state with it.
    - Engine test fixtures lose the run-wide list: `FixtureTools::new` (`context-tools.rs` lines 35-51) drops its `always` parameter, `arm_tools` and `arm_tools_scoped` (lines 77-102) become one helper, and `always_tool` and `echo_tools` (`models_loop.rs` lines 83-95) stop putting their tool in scope. The Testing Plan lists the tests that then offer for themselves.
    - `crates/runvil-internal/lua/src/vm/state.rs`: `current_tool_bindings` drops the `always` prefix, and `binding_for_scope` (lines 231-242) returns the offered binding unchanged or is folded into its caller.
    - `crates/runvil-internal/engine/src/execute/context.rs`: the `tools` and `tool_set` fields (lines 88-95) become one `Arc<ToolSet>`, which `tool_set()` (lines 272-277) hands out. `tool_set_snapshot` (lines 279-291) goes, and its callers read the shared set: `scheduler/chat.rs` lines 147 and 270, `scheduler/tool_call.rs` lines 210 and 291, and the tests `execute/tests/full_id_calls.rs` line 131 and `execute/tests/offering.rs` lines 92 and 131. The `Debug` impl's `"<dyn ToolView>"` field (line 429) follows.
    - `crates/runvil-internal/engine/src/execute/scope.rs`: `prepare_scoped_tools` (lines 59-110) advertises `binding.description()`, and the precedence comment (lines 74-79) goes.
    - The remaining test helpers that hold `Arc<Mutex<ToolSet>>` take `Arc<ToolSet>`: the `lua_over` helpers (`lua/src/plugins-tests.rs` line 52 and `lua/src/tools/tests.rs` line 203), `lua/src/tools/tests-offering.rs` line 34, and `shared_tools` (`engine/src/model/tests.rs` line 65).
    - `crates/plugin-user-input/src/lib.rs` lines 60-64: the opt-in doc names `plugins.offer('user-input')` or `tools.offer("user-input/ask")` in place of `tools.always_offer`.
    - `crates/workshop/agents/agents/chat.md`: the H1 keeps `models.default("chat")` and drops `tools.always_offer({"web/fetch", "web/search"})` (line 23), and `## Conversation` gains `plugins.offer('web')` before `tools.offer(tools.extras())` (line 29), since its model loop runs there.
    - `crates/build-xtask/src/engine_guards.rs`: `RETIRED_SEEDS` gains the distinctive removed names `install_always_offer`, `description_overrides`, `model_description`, `ToolView`, `lock_tool_set`, and `tool_set_snapshot`, after a search shows no other Engine code uses them, with the array length adjusted.
  - Step 3, the roster:
    - `crates/runvil-internal/engine/src/execute/environment.rs`: `Environment` (lines 27-38) gains the roster as one map from Plugin name to an optional reason, with no reason for a usable Plugin, set by a new builder, *proposed* `Environment::plugins(self, usable: Vec<PluginId>, unavailable: Vec<UnavailablePlugin>) -> Environment`, which folds both lists into the map. `prepare` (lines 107-114) copies the map onto the context beside the catalog and the preludes, and the `Debug` impl (lines 122-136) lists it.
    - `crates/runvil-internal/engine/src/execute/config.rs`: `RunContext` (line 42) gains the same map as a `pub(super)` field beside `tools` (line 95) and `preludes` (line 101), empty by default.
    - `crates/runvil-internal/engine/src/execute/requirements.rs`: the `UnavailablePlugin` docs (lines 150-170) widen from a required Plugin to any installed Plugin that can't serve.
    - `crates/runvil-internal/engine/src/execute/context-bound.rs`: `bound_tool_set` (lines 22-28) copies the context's map into the set.
    - `crates/runvil-internal/lua/src/handles.rs`: `ToolSet` gains `roster: BTreeMap<PluginId, Option<String>>`, so one lookup tells whether a Plugin is missing, usable, or unavailable.
    - `crates/runvil-internal/lua/src/plugins.rs`: `plugin_tools` (lines 134-150) seeds every usable roster entry with an empty tool list beside every declared Plugin, so each gets an object, and the module doc (lines 1-9) says so. `plugin_names` raises the unavailable text for a name whose roster entry holds a reason.
    - `crates/harness-internal/runner/src/host-run.rs`: `HostRunContext` gains a method, *proposed* `roster`, returning the usable Plugins' names and an `UnavailablePlugin` for each `Unavailable` and `NeedsUnmet` entry. A `NeedsUnmet` reason names the missing services, for example `the run lacks the services it needs: operator`.
    - `crates/harness-internal/runner/src/host.rs`: `begin_run` (lines 184-201) passes the roster to the environment it builds (lines 197-200).
    - `crates/runvil/public-api.txt`: gains `Environment::plugins` (the environment's entries are at lines 30 and 390-393); regenerate with `cargo +nightly-2026-09-05 xtask api --bless`.
- File and public API changes:
  - Lua API: adds `plugins.offer`; removes `tools.always_offer` and the description-override argument of `tools.offer`.
  - Facade API: adds `Environment::plugins` in Step 3. `ToolSet`, `ToolView`, `ToolRuntime`, and `ToolBinding` aren't in the facade listing.
  - Shipped prompts: `crates/workshop/agents/agents/chat.md` changes in Step 2.
- Data, persistence, failure, security, and privacy constraints:
  - A prompt that calls none of the new or removed functions resolves exactly today's scope, so its recorded `EffectRecord::Chat.tools` lists don't change and no replay `Flags` bit is needed.
  - A run recorded with `tools.always_offer` or a description override no longer replays on the new code, since its prompt now raises.
  - Section offers live on the section VM, so a section re-entered by `jump` starts with none.
  - No new effect, event, or record shape.

### Declarations

The Lua surface and the Rust items where crates coordinate. Names marked *proposed* in the Technical Design are proposals here too.

#### Lua surface

```lua
plugins.offer(plugin)
tools.offer(tool)
```

`plugin` is a Plugin name, a Plugin object, or an array of either, and `tool` is a tool id, a tool object, or an array of either. `plugins.offer` puts every tool of each Plugin in this section entry's scope. An empty array does nothing. `tools.always_offer` isn't installed.

#### Rust

`crates/runvil-internal/lua/src/scope.rs`, after Step 2:

```rust
/// Tracks the tools and Plugins one section VM offered.
#[derive(Debug, Default)]
pub struct ToolRuntime {
    /// The wire names `tools.offer` put in the section's scope, in
    /// first-offer order.
    pub added: Vec<String>,
    /// The Plugins `plugins.offer` put in the section's scope, in
    /// first-offer order.
    pub plugins: Vec<PluginId>,
    // allowed_tasks unchanged
}
```

`crates/runvil-internal/lua/src/handles.rs`, after Step 3, shared read-only as `Arc<ToolSet>`:

```rust
/// The run's tool set: the declared Plugins, the run's roster, and every
/// catalog tool the run can offer. Nothing changes it during a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSet {
    /// The Plugins the frontmatter declares, in declaration order.
    pub declared: Vec<PluginId>,
    /// Every installed Plugin the caller reports, declared or not, with or
    /// without tools: no reason when it can serve the run, and the reason,
    /// written for the model, when it can't.
    pub roster: BTreeMap<PluginId, Option<String>>,
    /// Every catalog tool the run can offer, in id order, each bound under
    /// its wire name.
    pub offered: Vec<ToolBinding>,
}
```

`crates/runvil-internal/engine/src/execute/environment.rs`:

```rust
impl Environment {
    /// Sets the run's roster: every installed Plugin that can serve the
    /// run, declared by the prompt or not, and every installed Plugin that
    /// can't, with the reason.
    #[must_use]
    pub fn plugins(self, usable: Vec<PluginId>, unavailable: Vec<UnavailablePlugin>) -> Environment { ... }
}
```

</implementation-contract>
<verification-contract>

## Testing Plan

Each step ships with its tests in the same commit. Lua tests pin the verb's argument forms and errors, the round scope's order, and the roster's visibility, as table-driven cases in existing files where they fit. One Engine test drives a whole multi-section prompt through the scheduler to pin what only the Engine shows: Plugin offers reaching every round, a shared offers section reaching each section that requires it, and each section entry starting with no offers. Harness tests pin the roster from the snapshot. The existing suite is the regression check, apart from the tests listed below.

- Unit, in the Lua crate:
  - Step 1, in `crates/runvil-internal/lua/src/plugins-tests.rs`, over its `plugin_set` and `lua_over` fixtures (lines 31-47 and 52-71), or in a new `plugins-tests-offer.rs` wired as `#[path = "plugins-tests-offer.rs"] mod offer;` if the file passes about 450 lines:
    - the verb accepts a name, a Plugin object from `plugins.get`, an array mixing both, and `plugins.required()`; records each Plugin once in `runtime.plugins`; and treats a repeat as a no-op
    - one error table, in the style of `lua/src/tools/tests-offering.rs` line 324: nil, no argument, a number, a tool object, an array holding a number, a malformed name (asserting `got "web/fetch"`), and an extra string argument; an absent Plugin, alone and inside an array of valid names, raises the absent-Plugin error and records nothing
    - the scope order: `current_tool_bindings` over section tools and section Plugins gives each wire name once at its first place, and offering `mail`, a declared Plugin with no tools, leaves it unchanged
  - Step 2, in the tools tests: `tools.offer` with a second argument raises the extra-argument text; `always_offer` joins the names `the_slot_era_functions_are_not_installed` (`lua/src/tools/tests-offering.rs` line 299) checks are absent; `the_tools_namespace_exposes_scoping_without_bind_or_call` (`lua/src/tools/tests.rs` line 286) drops its `always_offer` check; the shape-error table (`tests-offering.rs` line 324) drops its `tools.always_offer` and override rows.
  - Step 3, in `plugins-tests.rs`: an undeclared usable Plugin with zero tools gets an object from `plugins.get` and appears in `plugins.extras()` in name order; an unavailable Plugin gets `nil` from `plugins.get`, stays out of `plugins.extras()`, and raises its reason from `plugins.offer`, recording nothing; with an empty roster, the objects are today's.
- Existing tests that change:
  - Step 1: the six `ToolRuntime` literals become `ToolRuntime::default()`, as the Technical Design lists.
  - Step 2, removals. Tests whose subject is `tools.always_offer`, the description override, or `ToolView` go: `lua/src/tests/tool_scoping.rs` lines 8, 42, 143 (its refusal half moves to a `tools.offer` case if no other test covers it), and 373; `lua/src/tools/tests-offering.rs` line 174; the Engine's `engine/src/execute/tests.rs` lines 50 and 115; and `the_view_snapshots_the_declared_plugins_and_the_offering` (`lua/src/handles-tests.rs` lines 71-87). `handles-tests.rs` line 33 drops its `model_description` assert. `tests-offering.rs` lines 56 and 150 drop their override parts, and line 116 keeps only `tools.offer`. `tool_scoping.rs` line 257, `offer_rejects_misshapen_override_arguments`, becomes the extra-argument case.
  - Step 2, moves. Each `tools.always_offer` in a test prompt becomes a `tools.offer` in the section whose rounds use the tool, or a shared offers section those sections require:
    - `crates/harness-internal/runner/tests/it/harness-stop.rs`: `CHATS` (line 62) offers in `## Conversation`, and `LOOPS_ONCE` (line 76) in `## Only`
    - `crates/harness-internal/runner/tests/it/harness-stop-cap.rs`: `loops_offering` (line 36) writes its offers into the section its template loops in
    - `crates/harness-internal/runner/tests/it/effect_loop-cap.rs`: `PREAMBLE` (line 44) gains a shared offers section that each looping section requires, or each section offers for itself
    - `crates/runvil-internal/engine/src/execute/tests/observations.rs` line 338: the offer moves into the section whose round uses `tools/echo`
    - `crates/runvil-internal/engine/src/execute/tests/suite/require/effects.rs` lines 63-80, `an_h1_library_sets_prompt_wide_state_every_later_section_sees`: the library keeps `models.default` as the prompt-wide state, and the offer moves into a shared offers section that the later sections require
  - Step 2, fixtures. Every Engine test that put a tool in scope through a fixture's run-wide list offers it itself, with `tools.offer('<id>')` in the section whose rounds use it, written into the test's prompt or into the helper that builds that section; a small fixture helper that returns the offer line for its tools keeps the edits mechanical. These fixtures (`echo_tools()`, `always_tool`, `arm_tools`, and `FixtureTools::new` with a non-empty list) appear about 60 times in these files under `crates/runvil-internal/engine/src/execute/tests/`: `tool_loop.rs`, `models_loop.rs`, `parallel_tool_calls.rs`, `chat_arm.rs`, `models_loop_contract-rounds.rs`, `effects.rs`, `batch_turn.rs`, `exit_rules.rs`, `tool_scoping.rs`, `debug_and_counts.rs`, `parallel_tool_calls-withdrawn.rs`, `tool_loop-scope-gate.rs`, `tool_call_arm-local-handlers.rs`, `rounds.rs`, `context-tools.rs`, `tool_call_arm.rs`, `observations.rs`, `models_loop_contract.rs`, `scheduler/fanout/failure_semantics.rs`, `fanout_acceptance.rs`, `scheduler/failures-script-tools.rs`, `local_tools.rs`, `chat_scope.rs`, and `chat_record_rebuild.rs`. A fixture built with an empty list already has its prompt offer, and changes only its call. `tool_loop-scope-gate.rs` (lines 9-25) keeps its subject: its section offers `tools/scoped` and leaves `tools/global_tool` unoffered.
  - Step 2: the `ToolSet` constructions and test helpers the Technical Design lists build struct literals and hold `Arc<ToolSet>`.
- Integration and end-to-end:
  - Step 1, Engine: one new `crates/runvil-internal/engine/src/execute/tests/offering-plugins.rs`, wired from `offering.rs` (380 lines) as `#[path = "offering-plugins.rs"] mod plugins;`. It reuses `offering.rs`'s `Offered` fixture, `offering_context`, and `rounds_and_calls` (lines 13-34, 61-80, and 259-273), and parses its own prompt, since `offering_prompt` (lines 84-88) builds only a single `## Only` section. Its fixture Plugins are `kit`, with `kit/a` and `kit/b`, and `web`, with `web/fetch`. Each section with a round runs it on a fresh message list, `local history = messages.new()`, `history:user('<section name>')`, `models.loop(history)`, as the existing offering tests do (`offering.rs` lines 295-301), against a `ScriptedChat` holding one `resp_text` reply per round. The test reads the rounds with `rounds_and_calls` and maps each `EffectRecord::Chat` to its section by that user message, since a spawned task's round can come in either order. One test, one prompt:
    - the H1 calls `plugins.offer('kit')` and runs one round, which advertises `kit_a` and `kit_b`
    - `## A` calls `tools.offer('kit/a')`, `require '## offers'`, and `plugins.offer('kit')`, and its round advertises `kit_a`, `web_fetch`, `kit_b`, in that order
    - `## A` also `call`s `## Called`, whose round advertises nothing, and spawns and joins `## Worker`, which requires `## offers` and advertises only `web_fetch`
    - `## B`, which `## A` falls into, makes no offers, so its round advertises nothing, and it returns
    - `## offers` holds only `plugins.offer('web')`, and it sits with `## Called` and `## Worker` after `## B`, so the walk reaches them only through `require`, `call`, and `tasks.spawn`
  - Step 2: the Workshop tests that launch the embedded chat agent pass with the new `chat.md`: `crates/workshop/server/tests/it/chat_gate.rs`, which runs `agents/chat.md` as `crates/workshop/agents/src/discovery.rs` line 10 embeds it.
  - Step 3, Harness: beside `an_installed_native_adds_no_catalog_tool_and_no_prelude` (`crates/harness-internal/runner/src/host-tests.rs` lines 266-274), the roster lists the installed native subagents stub as usable, and an installed Plugin whose `ready` failed as unavailable with its reason. A prompt run through the Harness that doesn't declare the stub gets an object from `plugins.get` for it, and a prompt that offers the failed Plugin raises the unavailable text with its reason.
- Regression, security, and performance:
  - `engine/src/execute/tests/offering.rs`, `lua/src/plugins-tests.rs`, `lua/src/tools/tests-offering.rs`, `lua/src/tests/tool_scoping.rs`, the Harness's `runner/tests/it/prepare.rs` and `prepare-ready.rs`, and each Plugin crate's tests pass, apart from the changes above.
  - Round lists that existing tests assert for prompts using none of the new or removed functions stay as they are.
  - The two-way reserved-global test in `crates/runvil-internal/engine/src/lua/tests/globals.rs` passes at 61 names.
- Exit criteria, run in Steps 2 and 3:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - the Workshop tests and the Workshop clippy need the gateway sidecar staged for Tauri's `externalBin`: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe` before them, and `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc` after them, always, even when an earlier command failed
  - both clippy runs with `$env:CARGO_BUILD_WARNINGS='deny'`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - with `$env:RUSTDOCFLAGS='-D warnings'`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p runvil --no-deps`, `cargo doc -p harness --no-deps`, and `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n -g '!target' -g '!vibe/**' "always_offer|description_override|model_description|ToolView|tool_set_snapshot" crates prompts` hits only `RETIRED_SEEDS` and the test that checks `always_offer` is absent

</verification-contract>
<decision-record>

## Decision Record

Authors offer a Plugin whole with `plugins.offer`, in the section the call runs in, and get shared offers by requiring a shared section. Offers are recorded by Plugin name and resolved as each round is built, which is what stage 2's rendering needs. `tools.always_offer`, withholding, and the description override are gone, and with no writer left, the run's tool set is one read-only value. Every usable installed Plugin is visible to Lua through a roster the Harness passes in, and an unavailable one raises at the offer. The main risk is the facade addition, `Environment::plugins`, which other embedders don't have to call.

- Decisions:
  - This plan is stage 1 of the late-bound Plugin offering design, revised from the earlier Plugin offering plan, and stage 2 (rendering, listed tools, and name clashes) builds on it. Rationale: landing the earlier plan as written would build `plugins.always_offer` and `plugins.withhold` only to remove them, and would leave undeclared Plugins with zero tools unofferable. User chose: "Revise the old Plugin offering plan into stage 1, keeping its implementation detail but dropping always_offer and withhold and adding the late-bound offering items".
  - One verb, `plugins.offer`, offers a Plugin to the section entry it runs in, including the H1's own rounds. Rationale: the late-bound design's offer rule; `require` makes an offer in a library apply to the requiring section.
  - Shared offers come from a shared section that each section requires, which retires `tools.always_offer` and the planned `plugins.always_offer`. User: "we dont need always_ because we can just `require` a shared section".
  - There is no withholding. User: "lets get rid of withhold and always_withhold".
  - Inheriting offers from parent sections was considered and set aside. User: "we considered inheritance but there are problems with it which have not been solved and the solutions are not necessarily an improvement."
  - `tools.always_offer` is simply removed, with no special error. User chose: "Nothing special: the field is simply gone".
  - An offer takes no description override. User: "remove offer options completely, including the description override".
  - Once `tools.always_offer` is gone, the run's tool set is one read-only `Arc<ToolSet>`: `ToolView`, the lock helpers, `from_parts`, `for_test`, and the copy every read makes all go. Rationale: the lock and the view exist only for that writer, and every round and tool call clones the whole set through them; struct literals replace the two constructors, since every field is public and the set derives `Default`; a test-support `RunState::with_tool_set` replaces the two test helpers that swapped the set through the lock. The steps run in the order that makes this possible: `plugins.offer`, then the retirement, then the roster as a plain field. User chose: "B: reorder, and after retiring always_offer replace the locked set with an Arc<ToolSet>, deleting ToolView, the lock helpers, from_parts, and for_test".
  - The Workshop chat agent offers web tools with `plugins.offer('web')` in its `## Conversation` section. User: "Workshop should write `plugins.offer('web')`". Its model loop runs in that section, so an H1 offer wouldn't reach it.
  - Every usable installed Plugin is visible to Lua, declared or not, with or without tools, through a roster the Harness passes in with `Environment::plugins`. Rationale: the Engine learns Plugins only from declared names and tool descriptors today, so the subagents stub is invisible unless declared; the Harness's snapshot already knows each Plugin's state.
  - The roster is one map from Plugin name to an optional reason, no reason meaning usable. `Environment::plugins` folds its two lists into that map, and `Environment`, `RunContext`, and `ToolSet` all hold the same shape. Rationale: a Plugin is missing, usable, or unavailable, and one lookup answers all three, as the Harness's own snapshot map does; `declared` stays a list in frontmatter order. User chose: "Yes: one BTreeMap from Plugin name to its state, usable or unavailable with a reason", "Use Option<String> in place of the PluginState enum", and "Fold the roster into one map in Environment::plugins and store that on RunContext".
  - Offering an unavailable Plugin raises, naming the Plugin and its reason. User chose: "Raise, naming the Plugin and why it's unavailable".
  - `tools.offer`'s errors stay as they are in this stage. The error that names a Plugin's tools waits for stage 2's listed tools, and `tools.offer` gets no unavailable text. User chose: "Defer the tools.offer error that lists a Plugin's tools to stage 2" and "Drop the tools.offer error for an unavailable Plugin".
  - Plugin offers are recorded by name and expanded as each round is built, never expanded into tool names when made. Rationale: stage 2 renders each offered Plugin's catalog for the round's model, so the record has to stay a name.
  - The expansion filters `offered` by Plugin inside `current_tool_bindings`, with no helper on `ToolSet`. User chose: "Inline plugin_bindings into current_tool_bindings".
  - Listed tools wait for stage 2. Rationale: without rendering, every tool a Plugin ships is effectively listed, and unlisted tools matter only once rendering and subagents arrive.
  - The smallest surface, carried over from the earlier plan's review: `ToolRuntime` deriving `Default`, one decode for the verb, and one new Engine test file.
  - Defaults chosen in planning, open to override:
    - `plugins.get` returns `nil` for an unavailable Plugin, so the `if plugins.get(...)` guard covers outages too, and only an unguarded offer raises
    - a Plugin whose needed services the run lacks counts as unavailable, with a reason naming the services
    - a round's bound tools are ordered section tools, then section Plugins, each tool once at its first place
    - `plugins.offer` accepts a Plugin object or a list as well as a name; the late-bound design's "an offer takes just a name" means no options
    - an extra argument raises, for `plugins.offer` and now for `tools.offer`
    - offering a name with no Plugin in the run raises
- Rejected alternatives:
  - Landing the earlier Plugin offering plan as written. Reason: its `plugins.always_offer` and `plugins.withhold` would be built and then removed, and its rule for which Plugins count leaves the subagents stub unofferable. No revisit.
  - `plugins.always_offer` and keeping `tools.always_offer`. Reason: a required shared section does the job without a run-wide list. No revisit.
  - `plugins.withhold`, and `plugins.hold_back` before it. Reason: the user removed withholding. No revisit.
  - Child sections inheriting their parent's offers. Reason: its problems have no settled answers, and the answers on hand are not clearly better. Revisit if those problems get settled answers.
  - An error that says `tools.always_offer` was removed and points to `plugins.offer` and `require`. Reason: the user chose to let the field go. No revisit.
  - Offering an unavailable Plugin as a no-op with an event. Reason: the user chose to raise. No revisit.
  - A `ToolView::snapshot` that clones the whole set under one lock. Reason: it rewrites a trait whose only job ends with `tools.always_offer`. No revisit.
  - Keeping the lock and adding one more per-field `ToolView` method for the roster. Reason: the user chose the read-only set. No revisit.
  - Two roster fields, a list of usable Plugins beside a map of unavailable ones with their reasons. Reason: every lookup would check both, and their collection types didn't match for any reason. No revisit.
  - A `PluginState` enum for the roster's values. Reason: an optional reason says the same thing without a new type. No revisit.
  - Listing unavailable Plugins in `plugins.get` and `plugins.extras()`. Reason: the guard pattern would stop covering outages. Revisit if authors need an unavailable Plugin's reason in Lua.
  - Method forms on Plugin objects. Reason: two spellings for one action, and Plugin objects stay frozen data. Revisit if authors keep reaching for methods.
  - Expanding a Plugin offer into tool names when it's made. Reason: stage 2 needs the name at render time, and a Plugin with no tools would leave no record that it was offered. No revisit.
  - Storing Plugin names in the existing `added` string list. Reason: a Plugin name may contain `_`, so a Plugin named `web_fetch` would collide with the wire name of `web/fetch`. No revisit.
- Assumptions, risks, and notes:
  - Risk: `Environment::plugins` is a facade addition. An embedder that doesn't call it keeps today's visibility: declared Plugins and Plugins with a tool.
  - Risk: Step 2's test migration is the plan's largest part. About 60 Engine fixture uses move from the run-wide list to explicit offers, beside the moved prompts and the `Arc<ToolSet>` holders.
  - Note: the late-bound plan's stage 2 builds on `ToolRuntime.plugins` and the roster, renders each offered Plugin when a round is built, and adds the `tools.offer` error that names a Plugin's listed tools.
  - Note: the subagents plan (`C:\Users\Vinnie\.cursor\plans\subagents_plugin_5d2a8f61.plan.md`) was written against the earlier design and reads `ToolSet.always_plugins` and `ToolRuntime.withheld`, which this plan doesn't add. It needs updating before it runs.
  - Note: the model-shaped tool rendering plan (`C:\Users\Vinnie\.cursor\plans\model-shaped_tool_rendering_a6781f33.plan.md`) is history; the late-bound plan's stage 2 supersedes it.
  - Note: the guide books under `guide/books/` hold no chapters yet, and the separate `runvil-docs` repository documents an older tools API and gets its own update.

### Deferred and Out of Scope

- Deferred to stage 2, the late-bound plan: model-shaped rendering, listed and unlisted tools, the `tools.offer` error that names a Plugin's listed tools, name-clash numbering, and local tools no longer hiding catalog tools.
- Out of scope: the separate `runvil-docs` repository and the language spec in `../runvil-design`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the gateway (`crates/gateway/app`, package `gateway`, binary `runvil-gateway`), the workspace's one default member; any other crate is `cargo build --locked -p <package>`, and the desktop app is `cargo build --locked -p workshop`. Crate build scripts bundle the TypeScript UIs, so a fresh checkout first needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` (both `node_modules` trees are present on this machine).
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-substring>`; drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file is `node --test <path-to-file.mjs>`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, with the same `--all-features` exception for the three Workshop crates. The Plugin path's packages are `runvil-plugin`, `plugin-web`, `plugin-mcp`, `plugin-user-input`, `harness`, `harness-runner`, `harness-gateway-client`, `workshop-agents`, and `workshop-server`. CI also runs `cargo nextest run --locked -p workshop-server --features headless` and `cargo nextest run --locked -p workshop-workspace --all-features` on their own. Workshop UI packages: `npm test --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs`. The workspace run includes `build-xtask`, whose tests are the boundary and structural checks (`cargo xtask tidy` prints the same report on demand); its nightly-only fixtures run with `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Linter command: with `CARGO_BUILD_WARNINGS=deny` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`), run `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features` for the headless build shape. Never run a standalone `cargo check --workspace`. Supply chain: `cargo deny check` and `cargo hakari verify` (both installed here).
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; `core.hooksPath` is `.githooks`, whose pre-commit hook runs it).
- Docs command: with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` for the facades with default features, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items` and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. When the `runvil` facade's public surface changes, also run `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed here), which compares against `crates/runvil/public-api.txt`; `--bless` rewrites the listing.
- Test placement and naming conventions:
  - Unit tests live in a sibling file wired from the module as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `runvil-plugin/src/context.rs` beside `context-tests.rs`, `harness-internal/runner/src/packages.rs` beside `packages-tests.rs`); extra topic files take `<stem>-tests-<topic>.rs`. A module with three or more test files uses a `tests/` subdirectory instead (for example `runvil-internal/lua/src/tests/`). Older code, mostly `gateway/cloud-providers` and `gateway/stt`, keeps an inline `mod tests { }`.
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` for the `runvil` and `harness` facades, `tests/it/main.rs` elsewhere (including `runvil-plugin`, `plugin-mcp`, `plugin-user-input`, `harness-runner`, and most Workshop crates). Each topic is a `mod` file, and shared helpers sit in `support.rs`. `plugin-web` and `harness-gateway-client` have unit tests only. Prompt fixtures live under `crates/runvil/tests/prompts/` and `crates/runvil-internal/engine/tests/prompts/`.
  - Test functions are snake_case sentences stating the behavior (for example `construct_accepts_no_configuration_or_an_empty_object_and_refuses_anything_else`); a `test_` prefix is rare.
  - Test-only hooks sit behind a `test-support` feature (`runvil-plugin`, whose `testing::TestCall` lends a `ToolContext` to a Plugin crate's own tests, plus `runvil-lua`, `runvil-parser`, and `runvil-engine`) or `test-fixtures` (gateway and Workshop crates). Only `[dev-dependencies]` enable them, which `build-xtask`'s leak guard enforces.
  - Doctests are banned: `build-xtask` fails on any compiled code block in a doc comment, so examples belong in tests or `examples/` (for example `harness/examples/run-prompt.rs`). Benches use criterion with `harness = false`. JS tests are `*.test.mjs` beside the source or `*.mjs` under `test/`, run by `node --test`.
- Directory map:
  - `crates/`: all code. The top level is the public and shared layer: the Engine facade `runvil` and the Plugin contract `runvil-plugin`; the Harness facade `harness` and `harness-gateway-client`; the Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`; the gateway's client-facing `gateway-api-types` and `gateway-api-discovery`; `shared-error-source`, `shared-loopback`, and `shared-ui` (a TypeScript and CSS package, not a crate); tooling `build-xtask` (`cargo xtask tidy`, `api`, `site`, `new-crate`), `build-workshop` (`cargo workshop`), `build-ui`, `build-user-guide`, `build-ceiling`, and `build-llama-cuda`; and `workspace-hack` for cargo-hakari.
  - `crates/runvil-internal/`: the private Engine crates `types`, `vfs`, `model-client`, `lua`, `parser`, and `engine`.
  - `crates/harness-internal/runner`: `harness-runner`, the private Harness crate holding `PackageRegistry` (`packages.rs`), `HostContext` and installs (`host.rs`), and the effect loop.
  - `crates/gateway/`: private gateway crates; `app` is the binary, beside `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the `stt/` speech subsystem (`api` is the public `gateway-stt`, plus `engine`, `backend-whisper`, and `whisper-ffi`).
  - `crates/workshop/`: the Workshop Host: `desktop` (Tauri app, package `workshop`), `server` and `server-api`, the tier crates `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, and `workspace`, and the npm workspaces `ui`, `look`, and `platform`. `workshop-server`'s `src/agents.rs` (`with_plugins`) builds the Host's `PackageRegistry` from `plugin_web::PACKAGE`, `plugin_user_input::PACKAGE`, and `plugin_mcp::PACKAGE`, then installs `web`, `user-input`, `runvil/subagents`, and one `plugin-mcp` install per configured MCP server.
  - Repository root: `guide/` (docs site sources `books`, `chrome`, `landing`), `prompts/` (example prompts), `tools/` (Node scripts, each with a `.test.mjs`), `vibe/` (dated plan and run records), `local/` (gitignored machine-local gateway config and fixtures), and `images/`. `.github/workflows/` holds CI (`ci.yml`, gated by `ci-green`), release, and nightly jobs; `.githooks/` holds pre-commit (fmt) and pre-push (docs-claims, headless check, workspace clippy, cargo-deny); `.config/` holds nextest and hakari config; `.cargo/config.toml` holds the `workshop` and `xtask` aliases and the Windows `rust-lld` static-CRT linker setup; `.cursor/rules/` holds `workshop-architecture.mdc` and `workshop-spa.mdc`. Many crates carry their own `AGENTS.md` or `README.md`.
- Component boundaries (enforced by `cargo test -p build-xtask`):
  - Engine internal graph: `types` and `vfs` depend on no Engine crate; `model-client` depends on `types`; `lua` on `types`, `vfs`, and `model-client`; `parser` on `lua` and `types`; `engine` on all five. The `runvil` facade re-exports from all six. `runvil-plugin` is an Engine crate that depends only on `types` and `vfs` and declares no async runtime, `async-trait`, or HTTP client. Other families reach the Engine only through `runvil` and `runvil-plugin`.
  - Harness crates depend outside their family only on `runvil`, `runvil-plugin`, and `workspace-hack`, and never on a `build-*` crate. `harness-runner` is private to `harness`, whose `plugin` module re-exports `PackageRegistry`, `HostContext`, `InstallError`, `PluginKind`, `HostServices`, and `PluginId`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*` crates, `workspace-hack`, and outside libraries, never on a `build-*` crate. Engine, gateway, and Harness crates must not depend on a Plugin crate, except `harness-gateway-client`, which depends on `plugin-web` to implement its `SearchProvider`. The Host depends on Plugin crates directly (`workshop-server` on all three, `workshop-agents` on `plugin-user-input`).
  - Gateway crates depend on no Engine, Workshop, or Harness crate; their container crates are private to the family, and Workshop sees the gateway only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop tiers flow one way: the server (`workshop-server`), then features (`agents`, `run-log`, `user-state`, `workspace`), then services (`gateway`, `menu`, `status`), then vocabulary (`protocol`, `registry`, `support`). Same-tier crates never depend on each other, except `workshop-registry` on `workshop-protocol`. The desktop app's only Workshop dependency is `workshop-server-api`, which depends only on `workshop-server`.
  - `shared-*` crates depend on no product crate. Every `workshop-*`, `harness-*`, and `plugin-*` crate root must carry a `//! ## Invariants` section.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized terms with exactly one meaning each (AGENTS.md Definitions). A Plugin is never a capability, pack, or addon, and its lifecycle words are installed, snapshotted, and declared. Engine crates say "the caller" and never mention the Host. `docs-claims.mjs` enforces this wording in every `AGENTS.md`, `## Invariants` crate doc, and `.cursor/rules` file.
  - Every member inherits strict workspace lints: clippy `all` and `pedantic` at deny, `unwrap_used` and `expect_used` denied outside tests, `#[allow]` banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` and `unreachable_pub` warned, broken and private intra-doc links denied. `clippy.toml` bans installing process-global state outside binary entry points.
  - Crate roots open with a `//!` doc. A Plugin crate exports one `pub const PACKAGE: Package`, built with `Package::new("<vendor>/<name>", construct)`, and its crate doc names the default install name (`web`, `user-input`) and the services it needs.
  - Source directories are flat: a subdirectory needs three or more files, otherwise use `parent-label.rs` siblings wired with `#[path]`, converting in either direction when touching a group on the wrong side of the line. No Rust file under a crate's `src`, `tests`, `benches`, or `examples` may exceed 500 lines: every build script except `workspace-hack`'s runs `build_ceiling::check()`, and Harness and Plugin crates (and `runvil-vfs`) include its source through `#[path = "../build-ceiling/src/lib.rs"]` because they may name no `build-*` crate. A new crate comes from `cargo xtask new-crate <name>`.
  - A removed Engine symbol is added to `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`, and the retired-symbol scan then fails if live Engine source names it again.
  - Dependencies are declared once in `[workspace.dependencies]`, with comments explaining each pin and disabled feature, and members name them with `.workspace = true`.
  - Reuse an existing facility before adding machinery (AGENTS.md Principles). Error and status messages are written for model consumption: concise, naming what is missing with required versus actual. Comments explain only non-obvious constraints, and platform workarounds cite the upstream issue URL.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers).
  - Workshop UI CSS uses only `@workshop/look` and `ui/src/tokens/component.css` tokens, and persisted UI state goes through `ui-storage` to `ui-state.json` or the workspace file.
  - A finished plan is recorded under `vibe/` and closed with a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Five steps in three components. Each step is one commit that leaves its tests passing. The contracts above group the work as their Step 1 (`plugins.offer`), Step 2 (the retirement and the read-only tool set), and Step 3 (the roster). This section keeps their Step 1 as Step 1, splits their Step 2 into Steps 2, 3, and 4, and runs their Step 3 as Step 5. Each step says which contract items it does.

Components, in dependency order:

1. Plugin offers (Step 1). It comes first because it needs nothing new, and the Workshop chat agent's move off `tools.always_offer` needs `plugins.offer`. One piece, the verb and the round expansion, built jointly: the verb only writes `ToolRuntime.plugins` and only `current_tool_bindings` reads it, so neither is testable alone.
2. Run-wide offer retirement (Steps 2-4). It comes second because the chat agent needs `plugins.offer`, and the roster lands as a plain field of the read-only set this component leaves. Three pieces, built in sequence:
   - The move (Step 2): every prompt, test, and fixture comes off the run-wide list while `tools.always_offer` still works, so the suite stays green and the removal commit carries no migration.
   - The removal (Step 3): `tools.always_offer`, `ToolSet.always`, and the description override go. It needs the move first, since the removal makes every remaining caller raise.
   - The read-only set (Step 4): one `Arc<ToolSet>` replaces the lock. It needs the removal first, since `tools.always_offer` is the set's only writer. `lock_tools` goes here and not in Step 3, because `tools.offer` and the tool-object install also read the set through it.
3. Plugin roster (Step 5). It comes last because it is a plain field of the read-only set. Two pieces, built jointly: the Engine intake (`Environment::plugins` to `RunContext` to `ToolSet.roster` to `plugins.get`, `plugins.extras()`, and `plugins.offer`) and the Harness source (`HostRunContext::roster`, passed by `begin_run`). Joint because the Engine path's end-to-end tests run through the Harness, and the Testing Plan pins both halves in one set.

<step-1>

### Step 1: Offer a whole Plugin with `plugins.offer` [completed]

- Component: Plugin offers
- Tests:
  - `cargo nextest run --locked -p runvil-lua -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-lua -p runvil-engine --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-lua -p runvil-engine`
  - `cargo fmt --all --check`
- Commit: one commit with the summary `Offer a whole Plugin to a section with plugins.offer`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: the Technical Design's "Step 1, `plugins.offer`" list and the Testing Plan's Step 1 entries. Where the contracts say "Step 3" for the roster or the unavailable error, that is Step 5 here.
- Before editing: read the root `AGENTS.md` and the `AGENTS.md` of `crates/runvil-internal/lua` and `crates/runvil-internal/engine`.
- Terms: a section entry is one entry into a section (falling into it, `jump`, `call`, a task worker starting there, or the H1 pass), each with a fresh Lua VM and a fresh `ToolRuntime`. A wire name is a tool id with `/` and `.` replaced by `_` (`web/fetch` is `web_fetch`). A round is one model request inside `models.loop`, and the tools it advertises are recorded in `EffectRecord::Chat.tools`.

Artifacts:

- `crates/runvil-internal/lua/src/scope.rs`: `ToolRuntime` derives `Default` and gains `plugins: Vec<PluginId>`, documented as the Plugins `plugins.offer` put in the section's scope, in first-offer order. Its six default-valued struct literals become `ToolRuntime::default()`: `lua/src/vm.rs` (about line 281), `lua/src/tools/tests.rs` (207), `lua/src/plugins-tests.rs` (56), `lua/src/protocol/tests/answer.rs` (171), and the Engine's `execute/tests/tool_scoping.rs` (27 and 51). Drop each `BTreeMap` import that becomes unused, or clippy fails.
- `crates/runvil-internal/lua/src/plugins.rs`: `install_plugins` (lines 82-132) takes the section's shared `ToolRuntime` handle, which its call in `lua/src/vm/install.rs` (line 90) passes from `self.tool_runtime`, and installs `plugins.offer`. One decode, `plugin_names`, raises on an extra argument, walks an array, reads a string through `PluginId::parse` and `plugin_object` (lines 64-72), reads a `LuaPluginHandle` (lines 26-31) as its name, and checks every entry before recording any. The verb pushes each Plugin not already in `runtime.plugins`. If `plugins.rs` passes about 450 lines (it is at 154), the verb moves to a sibling `plugins-offer.rs` wired with `#[path]`.
- `crates/runvil-internal/lua/src/vm/state.rs`: `current_tool_bindings` (lines 183-196) appends, after the section's `added` names, the offered bindings of each `runtime.plugins` Plugin, by filtering `ToolSet.offered` on each binding's Plugin inline, with no helper on `ToolSet`. The run-wide `always` names still come first until Step 3. Its doc (lines 171-182) states the order.
- Lua tests: in `lua/src/plugins-tests.rs` over its `plugin_set` and `lua_over` fixtures (lines 31-47 and 52-71), or in a new `plugins-tests-offer.rs` wired as `#[path = "plugins-tests-offer.rs"] mod offer;` if the file passes about 450 lines (it is at 216).
- Engine test: a new `crates/runvil-internal/engine/src/execute/tests/offering-plugins.rs`, wired from `offering.rs` (380 lines) as `#[path = "offering-plugins.rs"] mod plugins;`. It reuses `offering.rs`'s `Offered` fixture, `offering_context`, and `rounds_and_calls` (lines 13-34, 61-80, and 259-273) and parses its own prompt, since `offering_prompt` (lines 84-88) builds only a single `## Only` section. No Engine production code changes: per-round resolution already runs through `current_tool_bindings`.

Behavior:

- `plugins.offer(plugin)` takes a local name such as `'web'`, a Plugin object (from `plugins.get`, `plugins.required()`, `plugins.extras()`, or a tool object's `plugin`), or an array of names and objects, and returns nothing. It takes no options.
- It offers each Plugin whole to the section entry it runs in. A call inside a section run by `require` applies to the requiring section, since a required section runs in the caller's own environment. A call in the H1 covers the H1's own rounds, and the sections after it start with no offers. A `call`ed section, a spawned task, a child section, and the next section each start with no offers, and a section re-entered by `jump` starts with none.
- Offering a Plugin again is a no-op, and an empty array does nothing. A Plugin with no tools, such as one with only a prelude, is accepted and adds nothing to a round.
- A round's bound tools, resolved each time a round is built: the run-wide `always` tools (until Step 3), then the section entry's `tools.offer` tools in first-offer order, then the tools of each `plugins.offer` Plugin, Plugins in first-offer order and each Plugin's tools in id order. Each tool appears once, at its first place. Local tools and task built-ins follow as today, and a local alias still wins over a bound tool with the same wire name.
- Plugin offers are recorded as Plugin names, never expanded into tool names when made, so stage 2 can render each offered Plugin when a round is built. They never go into `added`, since a Plugin named `web_fetch` would collide with the wire name of `web/fetch`.
- Errors, with exact text:
  - A wrong argument (nil, no argument, a number, a tool object, an array holding one, or a string that isn't a valid Plugin name) raises `plugins.offer takes a Plugin name, a Plugin object, or a list of them, got nil`, with the value found in place of `nil`, such as `got "web/fetch"`.
  - An extra argument raises `plugins.offer takes one Plugin or a list of them, got extra string`, so `plugins.offer('web', 'kit')` can't silently offer only `web`.
  - A name with no Plugin in the run raises `plugins.offer: no Plugin named "mcp" in this run; declare it under plugins: to require it, or offer it only when plugins.get("mcp") returns it`.
  - Every entry of a list is checked before any is recorded, so one bad entry records nothing.
- Unchanged here: `tools.always_offer` still works, `tools.offer` and its errors, `tools.offer_local`, `tools.call`, `tools.calls`, and what `plugins.get`, `plugins.required()`, and `plugins.extras()` return. `plugins` is already a reserved Engine global, so the reserved-global list stays at 61 names (`lua/src/globals.rs` line 168). The unavailable-Plugin error is Step 5's.
- Security: offering a Plugin grants nothing a per-tool offer of each of its tools couldn't. The round gate in `engine/src/execute/scheduler/chat.rs` still refuses a model call to a name its round didn't advertise.

Test cases:

- Lua, the verb: it accepts a name, a Plugin object from `plugins.get`, an array mixing both, and `plugins.required()`; it records each Plugin once in `runtime.plugins`; a repeat is a no-op.
- Lua, one error table in the style of `lua/src/tools/tests-offering.rs` line 324: nil, no argument, a number, a tool object, an array holding a number, a malformed name (asserting `got "web/fetch"`), and an extra string argument. An absent Plugin, alone and inside an array of valid names, raises the absent-Plugin error and records nothing.
- Lua, the scope order: `current_tool_bindings` over section tools and section Plugins gives each wire name once at its first place, and offering `mail`, a declared Plugin with no tools, leaves it unchanged.
- Engine, `offering-plugins.rs`, one test over one prompt. The fixture Plugins are `kit`, with `kit/a` and `kit/b`, and `web`, with `web/fetch`. Each section with a round runs it on a fresh list, `local history = messages.new()`, `history:user('<section name>')`, `models.loop(history)`, as the existing offering tests do (`offering.rs` lines 295-301), against a `ScriptedChat` holding one `resp_text` reply per round. Read the rounds with `rounds_and_calls` and map each `EffectRecord::Chat` to its section by that user message, since a spawned task's round can come in either order:
  - the H1 calls `plugins.offer('kit')` and runs one round, which advertises `kit_a` and `kit_b`
  - `## A` calls `tools.offer('kit/a')`, `require '## offers'`, and `plugins.offer('kit')`, and its round advertises `kit_a`, `web_fetch`, `kit_b`, in that order
  - `## A` also `call`s `## Called`, whose round advertises nothing, and spawns and joins `## Worker`, which requires `## offers` and advertises only `web_fetch`
  - `## B`, which `## A` falls into, makes no offers, so its round advertises nothing, and it returns
  - `## offers` holds only `plugins.offer('web')`, and it sits with `## Called` and `## Worker` after `## B`, so the walk reaches them only through `require`, `call`, and `tasks.spawn`
- Regression: `engine/src/execute/tests/offering.rs`, `lua/src/plugins-tests.rs`, `lua/src/tools/tests-offering.rs`, and `lua/src/tests/tool_scoping.rs` pass apart from the literal changes, and round lists for prompts that don't call `plugins.offer` don't change.

Constraints: the Engine stays sans-IO and deterministic, and scope resolution walks ordered lists only. Every `.rs` file stays under 500 lines (`build_ceiling::check()`). Error text is concise and self-contained for a model, naming what was required and what was found. Engine crates say "the caller" and never mention a Host, and Plugin is capitalized.

</step-1>

<step-2>

### Step 2: Move every prompt and fixture off the run-wide offer list [completed]

- Component: Run-wide offer retirement
- Tests:
  - `cargo nextest run --locked -p runvil-engine -p harness-runner -p workshop-agents --all-features`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `cargo nextest run --locked -p workshop-server`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, always, even when an earlier command failed
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine -p harness-runner -p workshop-agents --all-targets --all-features`
  - `cargo fmt --all --check`
- Commit: one commit with the summary `Offer tools per section in place of the run-wide list`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: from the Technical Design's "Step 2" list, only the Engine test fixtures losing the run-wide list and the `chat.md` change; from the Testing Plan's Step 2 entries, the moves, the fixtures, and the Workshop `chat_gate.rs` run. The rest of those lists is Steps 3 and 4 here.
- Before editing: read the root `AGENTS.md` and the `AGENTS.md` of `crates/runvil-internal/engine` and any other touched crate that has one.
- Terms: a round is one model request inside `models.loop`, and its advertised tools are recorded in `EffectRecord::Chat.tools`. A section entry gets a fresh VM, so a `tools.offer` reaches only that entry's rounds, while `tools.always_offer` today reaches every section.

Artifacts:

- Engine fixtures, under `crates/runvil-internal/engine/src/execute/tests/`:
  - `context-tools.rs`: `FixtureTools::new` (lines 35-51) drops its `always` parameter and builds its set with an empty run-wide list (still `ToolSet::for_test(Vec::new(), Vec::new(), bindings)`; Step 3 turns it into a literal). `arm_tools` and `arm_tools_scoped` (lines 77-102) become one helper that arms the bindings with nothing in scope, and their docs follow.
  - `models_loop.rs`: `always_tool` and `echo_tools` (lines 83-95) stop putting their tool in scope, and their docs stop saying it is always in scope.
  - `tool_call_arm.rs` line 199: its `ToolSet::for_test` passes an empty run-wide list, and its prompt offers the tool.
  - A small fixture helper that returns the `tools.offer(...)` line for a fixture's tools keeps the edits mechanical.
- Every Engine test that put a tool in scope through `echo_tools()`, `always_tool`, `arm_tools`, or `FixtureTools::new` with a non-empty list offers it itself, with `tools.offer('<id>')` in the section whose rounds use it, written into the test's prompt or into the helper that builds that section. These are about 60 uses in `tool_loop.rs`, `models_loop.rs`, `parallel_tool_calls.rs`, `chat_arm.rs`, `models_loop_contract-rounds.rs`, `effects.rs`, `batch_turn.rs`, `exit_rules.rs`, `tool_scoping.rs`, `debug_and_counts.rs`, `parallel_tool_calls-withdrawn.rs`, `tool_loop-scope-gate.rs`, `tool_call_arm-local-handlers.rs`, `rounds.rs`, `context-tools.rs`, `tool_call_arm.rs`, `observations.rs`, `models_loop_contract.rs`, `scheduler/fanout/failure_semantics.rs`, `fanout_acceptance.rs`, `scheduler/failures-script-tools.rs`, `local_tools.rs`, `chat_scope.rs`, and `chat_record_rebuild.rs`. A fixture built with an empty list already has its prompt offer and changes only its call.
  - Put each moved offer ahead of the section's other `tools.offer` calls, since the run-wide tools came first in a round, so asserted round lists keep their order.
  - `tool_loop-scope-gate.rs` (lines 9-25) keeps its subject: its section offers `tools/scoped` and leaves `tools/global_tool` unoffered, and the gate still refuses the model's call to it.
  - `tool_scoping.rs`: `always_offer_advertises_the_concrete_schema_under_its_wire_name_and_dispatches_by_id` (line 40) puts its tool in scope through `tools.offer` and its runtime's `added`, and is renamed without `always_offer`, for example `an_offered_tool_advertises_its_concrete_schema_under_its_wire_name_and_dispatches_by_id`. The module doc (lines 1-2) and the doc at line 16 stop naming `tools.always_offer`.
  - `scheduler/failures-script-tools.rs` line 117: the comment stops naming `always_offer`.
- Test prompts that call `tools.always_offer` move each offer into a `tools.offer` in the section whose rounds use the tool, or into a shared offers section those sections `require`:
  - `crates/harness-internal/runner/tests/it/harness-stop.rs`: `CHATS` (line 62) offers in `## Conversation`, and `LOOPS_ONCE` (line 76) in `## Only`
  - `crates/harness-internal/runner/tests/it/harness-stop-cap.rs`: `loops_offering` (line 36) writes its offers into the section its template loops in
  - `crates/harness-internal/runner/tests/it/effect_loop-cap.rs`: `PREAMBLE` (line 44) gains a shared offers section that each looping section requires, or each section offers for itself
  - `crates/runvil-internal/engine/src/execute/tests/observations.rs` line 338: the offer moves into the section whose round uses `tools/echo`
  - `crates/runvil-internal/engine/src/execute/tests/suite/require/effects.rs` lines 63-80, `an_h1_library_sets_prompt_wide_state_every_later_section_sees`: the library keeps `models.default` as the prompt-wide state, and the offer moves into a shared offers section that the later sections require
- `crates/workshop/agents/agents/chat.md`: the H1 keeps `models.default("chat")` and drops `tools.always_offer({"web/fetch", "web/search"})` (line 23). `## Conversation` gains `plugins.offer('web')` before `tools.offer(tools.extras())` (line 29), since its model loop runs there and an H1 offer wouldn't reach it.
- `crates/workshop/agents/src/discovery-tests.rs` line 27: rename `the_built_in_chat_is_always_offered_and_a_dir_file_shadows_its_source` without `always_offer`, for example `the_built_in_chat_is_always_listed_and_a_dir_file_shadows_its_source`, since Step 4's retired-name search matches it.
- Leave alone: production code; the tests whose subject is `tools.always_offer` or the description override (`lua/src/tests/tool_scoping.rs`, `lua/src/tools/tests-offering.rs`, and `engine/src/execute/tests.rs` lines 50 and 115), which Step 3 removes; and the tool set's lock and its holders, which Step 4 replaces.

Behavior:

- `tools.always_offer` still works, but after this step no shipped prompt, test prompt, or Engine fixture outside the tests of `tools.always_offer` itself puts a tool in scope run-wide.
- The Workshop chat agent offers web tools with `plugins.offer('web')` in its `## Conversation` section, so its conversation rounds advertise every `web` tool.
- Every round list an existing test asserts keeps its contents and order, and `tools.calls` seeds stay the same.

Test cases:

- The existing suites of `runvil-engine`, `harness-runner`, `workshop-agents`, and `workshop-server` pass, the last including `crates/workshop/server/tests/it/chat_gate.rs`, which runs `agents/chat.md` as `crates/workshop/agents/src/discovery.rs` line 10 embeds it.
- `tool_loop-scope-gate.rs` still pins that an unoffered catalog tool is refused.

</step-2>

<step-3>

### Step 3: Retire `tools.always_offer` and the offer description override [completed]

- Component: Run-wide offer retirement
- Tests:
  - `cargo nextest run --locked -p runvil-lua -p runvil-engine -p runvil -p harness-runner -p workshop-agents -p plugin-user-input --all-features`
  - `cargo test -p build-xtask`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-lua -p runvil-engine -p runvil -p harness-runner -p workshop-agents -p plugin-user-input --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-lua -p runvil-engine -p plugin-user-input`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo fmt --all --check`
  - `rg -n -g '!target' -g '!vibe/**' "always_offer|description_override|model_description" crates prompts` hits only `RETIRED_SEEDS` and the test that checks `always_offer` is absent
- Commit: one commit with the summary `Retire tools.always_offer and the offer description override`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: from the Technical Design's "Step 2" list, the items for `lua/src/tools.rs` except `lock_tools`, `lua/src/tools/decode.rs`, `lua/src/scope.rs`, `lua/src/handles.rs` except `ToolView`, `lock_tool_set`, and the trait's impl, `lua/src/vm/state.rs`'s `current_tool_bindings` and `binding_for_scope`, `engine/src/execute/scope.rs`, `plugin-user-input`, and the first three `RETIRED_SEEDS` names; from the Testing Plan's Step 2 entries, the unit tests, the removals, and the `ToolSet` constructions. Step 4 does the rest.
- Before editing: read the root `AGENTS.md` and the `AGENTS.md` of `crates/runvil-internal/lua` and `crates/runvil-internal/engine`.

Artifacts:

- `crates/runvil-internal/lua/src/tools.rs`: `install_always_offer` (lines 253-282) and its call in `install_tools` (lines 199-211) go. `install_offer` (lines 214-251) drops the override and the check that skips a tool already offered prompt-wide, and `resolve_entries` returns wire names only. `lock_tools` (lines 73-76) stays until Step 4, since `tools.offer` and `install_tools` still read the locked set through it. The module doc (lines 1-19) and `install_offer`'s doc follow.
- `crates/runvil-internal/lua/src/tools/decode.rs`: `OfferEntry` (lines 46-49) loses `description_override`, or the decode returns names, and `collect_offer_entries` (lines 71-117) raises the extra-argument text on any second argument in place of today's three override errors. Its docs (lines 3-4, 44, and 63-69) follow, and so does `lua/src/tools/userdata.rs` lines 6-8.
- `crates/runvil-internal/lua/src/scope.rs`: `ToolRuntime` loses `description_overrides`.
- `crates/runvil-internal/lua/src/handles.rs`:
  - `ToolSet` (lines 195-206) loses `always` and its accessor (lines 244-248), and its doc stops naming prompt-wide offers.
  - The `ToolView` trait loses its `always` method, and its impl loses that body. The rest of the trait, `lock_tool_set`, and the impl stay until Step 4, and the trait's doc stops naming `tools.always_offer` as a writer.
  - `for_test` (lines 208-221) and `from_parts` (lines 223-236) go, since every field is public and the set derives `Default`. Their callers build struct literals: `from_parts` at `engine/src/execute/context-bound.rs` line 23, `engine/src/execute/context.rs` line 286 (inside `tool_set_snapshot`, which then builds its literal from the view's `declared` and `offered`; Step 4 deletes it), and `lua/src/handles-tests.rs` line 57, and `for_test`'s 12 calls in `lua/src/tools/tests.rs`, `lua/src/tools/tests-offering.rs`, `lua/src/tests.rs`, `lua/src/tests/tool_scoping.rs`, `lua/src/plugins-tests.rs`, `lua/src/handles-tests.rs`, and the Engine's `engine/src/lua/tests.rs`, `engine/src/lua/tests/globals.rs`, `engine/src/execute/tests.rs`, `engine/src/execute/tests/context-tools.rs`, and `engine/src/execute/tests/tool_call_arm.rs`. A Lua test that relied on a non-empty run-wide list offers its tool with `tools.offer` instead.
  - Literal rule: name only the fields that aren't empty, and add `..ToolSet::default()` only when a field is left out. A literal naming both `declared` and `offered` takes no `..ToolSet::default()`, since clippy's `needless_update` denies a base when every field is named; Step 5 adds the base when `roster` arrives.
  - `ToolBinding` loses `model_description` (line 44), its getter (lines 108-109), and its `None` in `from_descriptor` (line 63).
- `crates/runvil-internal/lua/src/vm/state.rs`: `current_tool_bindings` drops the `always` prefix, so a round's bound tools are the section's `tools.offer` tools, then its offered Plugins' tools. `binding_for_scope` (lines 231-242) returns the offered binding unchanged or is folded into its caller. The doc follows.
- `crates/runvil-internal/engine/src/execute/scope.rs`: `prepare_scoped_tools` (lines 59-110) advertises `binding.description()`, and the precedence comment (lines 74-79) goes.
- Docs that name `tools.always_offer` as the set's writer or as a prompt-wide record: `lua/src/vm.rs` lines 61 and 323 and `engine/src/execute/context.rs` lines 89 and 326. Reword them to say nothing writes the set during a run; the lock itself stays until Step 4.
- `crates/plugin-user-input/src/lib.rs` lines 60-64: the opt-in doc names `plugins.offer('user-input')` or `tools.offer("user-input/ask")` in place of `tools.always_offer`.
- `crates/build-xtask/src/engine_guards.rs`: `RETIRED_SEEDS` gains `install_always_offer`, `description_overrides`, and `model_description`, after a search shows no other Engine code uses them, and its length goes from 25 to 28.

Behavior:

- `tools.always_offer` is no longer installed. A call to it fails as a call to any missing field does, with no special error.
- `tools.offer(tool)` takes one tool id or tool object, or an array of them, and no second argument. A second argument raises `tools.offer takes one tool or a list of them, got extra string`. Its other errors stay as they are, including `tools.offer: "<id>" is not a catalog tool in this run`.
- Every round's description is the tool's catalog text, and round lists are otherwise unchanged.
- A run recorded with `tools.always_offer` or a description override no longer replays, since its prompt now raises. No replay `Flags` bit is added, and no new effect, event, or record shape.

Test cases:

- New and changed, in the Lua tools tests: `tools.offer` with a second argument raises the extra-argument text; `always_offer` joins the names `the_slot_era_functions_are_not_installed` (`lua/src/tools/tests-offering.rs` line 299) checks are absent; `the_tools_namespace_exposes_scoping_without_bind_or_call` (`lua/src/tools/tests.rs` line 286) drops its `always_offer` check; the shape-error table (`tests-offering.rs` line 324) drops its `tools.always_offer` and override rows.
- Removed, since their subject is `tools.always_offer` or the description override: `lua/src/tests/tool_scoping.rs` lines 8, 42, 143, and 373, with line 143's refusal half moving to a `tools.offer` case if no other test covers it; `lua/src/tools/tests-offering.rs` line 174; and the Engine's `engine/src/execute/tests.rs` lines 50 and 115. The module doc of `lua/src/tests/tool_scoping.rs` (line 1) follows.
- Trimmed: `lua/src/handles-tests.rs` line 33 drops its `model_description` assert, and `the_view_snapshots_the_declared_plugins_and_the_offering` (lines 71-87) drops its `always` part (Step 4 removes it with `ToolView`). `tests-offering.rs` lines 56 and 150 drop their override parts, and line 116 keeps only `tools.offer`. `lua/src/tests/tool_scoping.rs` line 257, `offer_rejects_misshapen_override_arguments`, becomes the extra-argument case.
- Regression: the reserved-global test in `engine/src/lua/tests/globals.rs` passes at 61 names, and round lists that existing tests assert for prompts using none of the new or removed functions stay as they are.

</step-3>

<step-4>

### Step 4: Share the run's tool set read-only [completed]

- Component: Run-wide offer retirement
- Tests:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, always, even when an earlier command failed
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p harness --no-deps`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n -g '!target' -g '!vibe/**' "always_offer|description_override|model_description|ToolView|tool_set_snapshot" crates prompts` hits only `RETIRED_SEEDS` and the test that checks `always_offer` is absent
- Commit: one commit with the summary `Share the run's tool set read-only as one Arc<ToolSet>`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: the rest of the Technical Design's "Step 2" list: `ToolView`, `lock_tool_set`, the trait's impl, `lock_tools`, the `Arc<ToolSet>` holders, `RunState::with_tool_set`, `RunState`'s fields and `tool_set_snapshot`, and the last three `RETIRED_SEEDS` names; from the Testing Plan, the `Arc<ToolSet>` test helpers and the exit criteria.
- Before editing: read the root `AGENTS.md` and the `AGENTS.md` of `crates/runvil-internal/lua` and `crates/runvil-internal/engine`.

Artifacts:

- `crates/runvil-internal/lua/src/handles.rs`: `ToolView` (lines 278-305), `lock_tool_set` (lines 307-312), and the trait's impl (lines 314-326) go. `lua/src/lib.rs` line 170 and the Engine's `engine/src/lua.rs` line 20 stop exporting `ToolView`. The `ToolSet` doc says nothing changes it during a run. `the_view_snapshots_the_declared_plugins_and_the_offering` (`lua/src/handles-tests.rs` lines 71-87) goes.
- `crates/runvil-internal/lua/src/tools.rs`: `lock_tools` (lines 73-76) goes, and `install_tools` and `install_offer` read the shared set without a lock.
- The set's holders take `Arc<ToolSet>` in place of `Arc<Mutex<ToolSet>>`:
  - production: `RunState::new` builds the set (`engine/src/execute/context.rs` line 139); section setup hands `ctx.tool_set()` to each section VM and to the H1's (`engine/src/execute/section_context-construct.rs` lines 99 and 191); `SectionVm`'s `bound_tools` (`lua/src/vm.rs` line 63, its default at line 279, and `new_for_section` at lines 331-337); `install_tools` (`lua/src/tools.rs` lines 199-211) and `install_offer` (line 221); `install_tool_lists` (`lua/src/tools/objects.rs` lines 62-72); `install_plugins`; and the calls in `lua/src/vm/install.rs` lines 83-90. `install_tool_objects` (`objects.rs` line 42) already takes `&ToolSet`. `install_tool_call_counts` (`lua/src/vm/state.rs` lines 102-109) and the test-only `tool_bag_handles` (lines 119-127) read the set without a lock.
  - tests and benches: `shared_set` and `section_vm_with_set` (`lua/src/tests.rs` lines 243 and 248), `lua/src/protocol/tests/answer.rs` line 170, `lua/benches/surface.rs` line 43, `engine/src/lua/tests.rs` line 96, `engine/src/lua/tests/globals.rs` line 93, `engine/src/execute/tests.rs` lines 58 and 130, `engine/src/execute/tests/tool_scoping.rs` line 103, the `lua_over` helpers (`lua/src/plugins-tests.rs` line 52 and `lua/src/tools/tests.rs` line 203), `lua/src/tools/tests-offering.rs` line 34, and `shared_tools` (`engine/src/model/tests.rs` line 65).
- `crates/runvil-internal/engine/src/execute/context.rs`: the `tools` and `tool_set` fields (lines 88-95) become one `Arc<ToolSet>`, which `tool_set()` (lines 272-277) hands out. `tool_set_snapshot` (lines 279-291) goes, and its callers read the shared set: `scheduler/chat.rs` lines 147 and 270, `scheduler/tool_call.rs` lines 210 and 291, and the tests `execute/tests/full_id_calls.rs` line 131 and `execute/tests/offering.rs` lines 92 and 131. The `ToolView` import (line 24) goes, and the `Debug` impl's `"<dyn ToolView>"` field (line 429) follows. The docs reworded in Step 3 now describe one shared read-only value.
- A test-support method `RunState::with_tool_set(self, set: ToolSet) -> RunState`, gated like the other test-support hooks, replaces the shared set before any section VM exists. `FixtureTools::install` (`engine/src/execute/tests/context-tools.rs` lines 60-65) and `engine/src/execute/tests/model_tasks.rs` lines 69-71, which today replace the set through the lock after `RunState::new`, switch to it, and the fixtures' callers, such as `loop_context_observed` (`engine/src/execute/tests/models_loop.rs` lines 55-73) and the arming helper Step 2 merged in `context-tools.rs`, build the state with it.
- `crates/build-xtask/src/engine_guards.rs`: `RETIRED_SEEDS` gains `ToolView`, `lock_tool_set`, and `tool_set_snapshot`, after a search shows no other Engine code uses them, and its length goes from 28 to 31.

Behavior:

- No behavior change. Every round, every name check of a model's tool call, and every tool call reads the one shared set with no lock and no copy, where today each clones the whole set, every binding and schema included.
- Round lists, tool call counts, and recorded effects stay as they are.

Test cases:

- No new tests; the existing suite is the regression check.
- `engine/src/execute/tests/offering.rs` and `offering-plugins.rs`, `lua/src/plugins-tests.rs`, `lua/src/tools/tests-offering.rs`, `lua/src/tests/tool_scoping.rs`, the Harness's `crates/harness-internal/runner/tests/it/prepare.rs` and `prepare-ready.rs`, and each Plugin crate's tests pass.
- The Workshop tests that launch the embedded chat agent pass, including `crates/workshop/server/tests/it/chat_gate.rs`.
- The two-way reserved-global test in `engine/src/lua/tests/globals.rs` passes at 61 names.

</step-4>

<step-5>

### Step 5: Give Lua an object for every usable installed Plugin [completed]

- Component: Plugin roster
- Tests:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, always, even when an earlier command failed
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p harness --no-deps`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test -p build-xtask`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n -g '!target' -g '!vibe/**' "always_offer|description_override|model_description|ToolView|tool_set_snapshot" crates prompts` hits only `RETIRED_SEEDS` and the test that checks `always_offer` is absent
- Commit: one commit with the summary `Give Lua an object for every usable installed Plugin`, a body, `Design:` lines, and a `Plan: vibe/<file>` trailer naming the run's copy of this plan. Run `cargo +nightly-2026-09-05 xtask api --bless` before committing, so `crates/runvil/public-api.txt` lists `Environment::plugins`.
- Contract items: the Technical Design's "Step 3, the roster" list and the Testing Plan's Step 3 entries. Where the contracts say "Step 3" or "from Step 3", that is this step.
- Before editing: read the root `AGENTS.md`, the `AGENTS.md` of `crates/runvil-internal/lua`, `crates/runvil-internal/engine`, and `crates/runvil-internal/types`, and any `AGENTS.md` under `crates/harness-internal/runner`.
- Terms: the run's roster is what the Harness's snapshot says about each installed Plugin for this run, usable or unavailable with a reason (`crates/harness-internal/runner/src/host-run.rs` lines 34-46). A Plugin in the run is one the frontmatter declares, one with a tool in the run's offering, and from this step every usable Plugin on the roster.

Artifacts:

- `crates/runvil-internal/engine/src/execute/environment.rs`: `Environment` (lines 27-38) gains the roster as one map from Plugin name to an optional reason, with no reason for a usable Plugin. A new builder sets it, `#[must_use] pub fn plugins(self, usable: Vec<PluginId>, unavailable: Vec<UnavailablePlugin>) -> Environment`, documented as setting every installed Plugin that can serve the run, declared by the prompt or not, and every installed Plugin that can't, with the reason. It folds both lists into the map. `prepare` (lines 107-114) copies the map onto the context beside the catalog and the preludes, and the `Debug` impl (lines 122-136) lists it.
- `crates/runvil-internal/engine/src/execute/config.rs`: `RunContext` (line 42) gains the same map as a `pub(super)` field beside `tools` (line 95) and `preludes` (line 101), empty by default.
- `crates/runvil-internal/engine/src/execute/requirements.rs`: the `UnavailablePlugin` docs (lines 150-170) widen from a required Plugin to any installed Plugin that can't serve.
- `crates/runvil-internal/engine/src/execute/context-bound.rs`: `bound_tool_set` (lines 22-28) copies the context's map into the set.
- `crates/runvil-internal/lua/src/handles.rs`: `ToolSet` gains `pub roster: BTreeMap<PluginId, Option<String>>` between `declared` and `offered`, documented as every installed Plugin the caller reports, declared or not, with or without tools: no reason when it can serve the run, and the reason, written for the model, when it can't. The `ToolSet` doc becomes: the run's tool set, the declared Plugins, the run's roster, and every catalog tool the run can offer; nothing changes it during a run. Every `ToolSet` literal that names `declared` and `offered` but not `roster` gains `..ToolSet::default()`.
- `crates/runvil-internal/lua/src/plugins.rs`: `plugin_tools` (lines 134-150) seeds every usable roster entry with an empty tool list beside every declared Plugin, so each gets an object, and the module doc (lines 1-9) says so. `plugin_names` raises the unavailable text for a name whose roster entry holds a reason, before anything is recorded.
- `crates/harness-internal/runner/src/host-run.rs`: `HostRunContext` gains a method `roster` returning the usable Plugins' names and an `UnavailablePlugin` for each `Unavailable` and `NeedsUnmet` entry. A `NeedsUnmet` reason names the missing services, for example `the run lacks the services it needs: operator`.
- `crates/harness-internal/runner/src/host.rs`: `begin_run` (lines 184-201) passes the roster to the environment it builds (lines 197-200).
- `crates/runvil/public-api.txt`: gains `Environment::plugins` (the environment's entries are at lines 30 and 390-393), regenerated with `cargo +nightly-2026-09-05 xtask api --bless`.

Behavior:

- `plugins.get(name)` returns an object for every Plugin in the run, now including every usable installed Plugin, declared or not, with or without tools. It returns `nil` for a Plugin that's absent or unavailable, so `if plugins.get('mcp') then plugins.offer('mcp') end` covers both a missing Plugin and an outage.
- `plugins.required()` lists the declared Plugins and `plugins.extras()` every other Plugin in the run, both in name order as today, so `plugins.extras()` now includes usable undeclared Plugins with zero tools. Unavailable Plugins stay out of both.
- `plugins.offer` of an unavailable Plugin raises `plugins.offer: the mcp Plugin is unavailable in this run: <reason>`, with the Plugin's name and reason in place of `mcp` and `<reason>`. Like every other entry error, it records nothing from the call.
- In a Workshop run whose prompt doesn't declare it, `plugins.get('subagents')` returns an object for the installed native `runvil/subagents` stub, and `plugins.offer('subagents')` succeeds and adds nothing to a round, since the stub has no tools.
- A declared Plugin that is unavailable still fails the run at start (`host-run.rs` lines 130-153).
- An embedder that doesn't call `Environment::plugins` keeps today's visibility: declared Plugins and Plugins with a tool.
- Unchanged: `tools.offer` gets no unavailable text, and its errors stay as they are.
- Constraints: the Engine crates don't depend on `runvil-plugin` or any Harness crate; the roster reaches the Engine as plain Plugin names and reasons through `Environment`. Engine crates say "the caller" and never mention a Host, and Plugin-facing docs say installed, snapshotted, or declared. The reasons are already written for the model that reads them (`engine/src/execute/requirements.rs` lines 150-158). Every `.rs` file stays under 500 lines.

Test cases:

- Lua, in `lua/src/plugins-tests.rs`: an undeclared usable Plugin with zero tools gets an object from `plugins.get` and appears in `plugins.extras()` in name order; an unavailable Plugin gets `nil` from `plugins.get`, stays out of `plugins.extras()`, and raises its reason from `plugins.offer`, recording nothing; with an empty roster, the objects are today's.
- Harness, in `crates/harness-internal/runner/src/host-tests.rs` beside `an_installed_native_adds_no_catalog_tool_and_no_prelude` (lines 266-274): the roster lists the installed native subagents stub as usable, and an installed Plugin whose `ready` failed as unavailable with its reason.
- Harness, a prompt run through the Harness, in `crates/harness-internal/runner/tests/it/` beside `prepare-ready.rs`'s `an_undeclared_plugin_whose_ready_fails_leaves_the_run_going_without_its_tools`: a prompt that doesn't declare the stub gets an object from `plugins.get` for it, and a prompt that offers the failed Plugin raises the unavailable text with its reason.
- Regression: `prepare.rs` and `prepare-ready.rs` pass, the reserved-global test passes at 61 names, and Step 1's `offering-plugins.rs` is unchanged.

</step-5>

Owned by the run and not by any step: the seed copy of this plan into `vibe/` and `vibe/ACTIVE` before Step 1, and after Step 5, marking the `vibe/` copy completed, deleting `vibe/ACTIVE`, and the `Close plan: plugin-offering` commit. Nothing is pushed.

Dependencies: the baseline is `ba4a63beb`, and no unlanded plan comes first. The late-bound plan's stage 2 starts after `Close plan: plugin-offering`, and the subagents plan after that, once it's updated.

</execution-plan>
