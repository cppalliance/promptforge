---
name: tools-local
overview: "Add tools.local(alias, description, params, handler) - Lua-backed tools with explicit parameter schema. Includes companion refactor: install control globals once, execute() passes nil reply."
todos:
  - id: install-log-store
    content: Install log and store once per section via vm.install_host_apis()
    status: pending
  - id: install-control
    content: Install control globals once per section via vm.install_control_globals()
    status: pending
  - id: delete-prologue-epilog
    content: Delete prologue/epilog split, one run_loaded_with_control for every chunk
    status: pending
  - id: execute-nil-reply
    content: execute() passes nil reply to subroutines
    status: pending
  - id: models-default
    content: Rename models.only to models.default, remove models.use foreclosure
    status: pending
  - id: local-storage
    content: Add LocalTools storage to SectionVm
    status: pending
  - id: tools-local-fn
    content: Install tools.local Lua function with params table to JSON schema
    status: pending
  - id: schema-rebuild
    content: Rebuild schemas per prose block, delete ToolScope/snapshot_tool_scope
    status: pending
  - id: dispatch-local
    content: Dispatch local tools in tool loop with sentinel ToolId
    status: pending
  - id: models-get-infer
    content: Add models.get() and models.infer(), keep handle:infer()
    status: pending
  - id: tests
    content: Tests for all new behavior
    status: pending
  - id: docs
    content: Documentation for all new APIs
    status: pending
isProject: false
---

# tools.local - Lua-backed tools

## What it is

`tools.add_local(alias, description, params, handler)` declares a tool backed by a Lua function with an explicit parameter schema. When the model calls this tool during inference, the tool loop invokes the Lua handler instead of an external service.

## Lua API

```lua
tools.local("extract_section", "Extract a range of lines from the paper", {
    name = {"string", "Section heading text"},
    start_line = {"integer", "1-based line number where the section begins"},
    end_line = {"integer", "1-based line number where the section ends"},
}, function(args)
    store.write("sections/" .. args.name .. ".md", sliced)
    return "stored " .. args.name
end)
```

Four positional arguments:
- `alias` (string): tool name, same rules as `tools.need` (`[A-Za-z][A-Za-z0-9_-]{0,63}`)
- `description` (string): one-sentence description the model sees
- `params` (table): flat table of `{param_name = type_or_spec}`. Each value is either a bare type string or a two-element array `{type, description}`. Supported types: `"string"`, `"integer"`, `"number"`, `"boolean"`. All parameters are required.
- `handler` (function): receives `args` as a Lua table with the named fields, returns a string

Parameter declaration forms:

```lua
-- Bare type (no per-parameter description)
{ name = "string", start_line = "integer" }

-- Type + description (helps small models)
{ name = {"string", "Section heading text"},
  start_line = {"integer", "1-based line number where the section begins"},
  end_line = {"integer", "1-based line number where the section ends"} }

-- Mixed (some with descriptions, some without)
{ name = {"string", "Section heading text"}, count = "integer" }
```

The engine converts the params table into JSON Schema:

```json
{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Section heading text"},
    "start_line": {"type": "integer", "description": "1-based line number where the section begins"},
    "end_line": {"type": "integer", "description": "1-based line number where the section ends"}
  },
  "required": ["name", "start_line", "end_line"]
}
```

Rules:
- Called from any H2 Lua chunk
- Immediately available to the model starting from the next prose block or `model:infer` call
- Handler defined in `lua shared` can be referenced by name from any section
- Handler has access to the section's `store` and globals (same VM)
- Handler returns a string; Lua errors propagate as tool-call failures
- Trusted output (no nonce wrapping) - the prompt author wrote the handler

## Design decisions

- **Explicit schema from params table:** The model sees full `parameters` JSON Schema with types. Small models need this to call tools reliably. All params are required - no optional parameter support.
- **Flat types only:** `"string"`, `"integer"`, `"number"`, `"boolean"`. No nested objects, no arrays, no enums. Keeps both the Lua API and model alignment simple.
- **Trusted output:** Local tool output is trusted since the prompt author wrote the handler. External tool output (web_search, web_fetch) stays untrusted.
- **Same VM:** The handler runs in the section VM that declared it, sharing globals, store handle, and closures. This enables the accumulator pattern (handler appends to a table the epilog reads).
- **Handlers can call `execute()`, `fanout`, and `model:infer`:** Local tool handlers can call `execute()` to spawn subagent sections, `fanout` for parallel work, and `model:infer` for direct inference on a model handle. This is the primary use case - a tool that combines deterministic Lua data with one-shot inference. `execute()` from a handler works because the tool loop is suspended while the handler runs. The inner section runs to completion and returns its reply as the tool result.
- **Two inference paths:** `handle:infer(prompt)` runs inference with that specific model, fresh context, no tools. `models.infer(prompt)` uses the section's current model, fresh context, no tools. Both are direct gateway calls, not tool loops. `models.get(alias)` returns the handle for a pre-declared model without changing the section's model.
- **Handlers cannot call `jump`:** `jump` from a handler would set the jump slot but not take effect until the current prose block finishes - the model's response would complete, then the jump fires. That's confusing semantics. When calling a local handler, the engine temporarily nils `jump` and restores it after. Normal Lua chunks keep full `jump` access.
- **Sequential dispatch:** Multiple local tool calls in one response execute sequentially, same as external tools. Fine for store operations.

## Companion refactor: install control globals once

This is a prerequisite simplification that makes the whole system cleaner before adding `tools.local`.

**What changes:**

- `jump`, `execute`, `fanout`, `tasks`, `log` are installed ONCE per section, not per chunk
- The install happens in `run_one_section` in engine.rs (after VM creation, before the block walk), NOT in `inject_host_with_var`. The callbacks need WalkContext which isn't available at inject time.
- `execute()` starts with `reply` = nil (a fresh subroutine has no reply context). The `execute` callback passes `None` to `run_execute_section` for `last_reply`. If the caller wants to pass context, use the `input` parameter.
- `clear_control_globals` is deleted. No per-chunk install/clear cycle.
- The `before_prose` parameter threading through `run_section_lua` is deleted. All chunks are just Lua chunks. The prologue/epilog distinction is gone - no `seen_prose` flag, no `run_prologue_with_control` vs `run_epilog_with_control`. One `run_loaded_with_control` for every chunk. Observer events just report the section name, not prologue/epilog granularity.
- `jump_slot` stays as `Arc<Mutex<Option<String>>>` on SectionVm (NOT simplified to stack-local). The closures are installed once with `lua.create_function` (not scope-bound), so they need owned state that outlives the scope. The Arc<Mutex> is the correct shape for a closure that persists across multiple chunks. The stack-local idea via `create_function_mut` doesn't work because each chunk opens its own `lua.scope` - the closure would die when the scope ends.
- The `has_children` gate on `fanout_callback` is removed - `fanout` is installed unconditionally.

**Why install from engine.rs, not inject_host_with_var:** The `execute` and `fanout` callbacks need WalkContext (store, args, observer, tools, models, etc.). That context is built in `run_one_section`, not at inject time. The VM method `inject_host_with_var` doesn't have access to it. So the install happens in `run_one_section` after `inject_host_with_var` returns, using a new method like `vm.install_control_globals(callbacks)`.

**Why `execute()` gets nil reply:** `reply` after a `fanout()` is the parent's reply, not the fanout results. `reply` is whatever the last prose produced - meaningless in the context of calling a subroutine. The subroutine should start clean. Pass context via `execute(target, input)`.

**Files:**

- `crates/promptforge-core/src/lua/vm.rs`: Add a method `install_control_globals` that takes the execute/fanout callbacks and installs `jump`, `execute`, `fanout`, `tasks`, `log`, and `store` as globals using `lua.create_function` (not scope-bound). Delete `clear_control_globals`. Delete `run_prologue_with_control`/`run_epilog_with_control` distinction - just `run_loaded_with_control` called for every chunk (which now doesn't need to install anything). Keep `jump_slot` as `Arc<Mutex<Option<String>>>` - it's the correct shape for a persistent closure.

**Store must be installed once:** Currently `store` is installed per-chunk inside `lua.scope` via `install_store_table`. With install-once, store methods would be invalid after the chunk's scope ends. Local tool handlers running during the tool loop have no live Scope. Install `store` once alongside the control globals using `lua.create_function` for each store method instead of scope-bound closures.

- `crates/promptforge-core/src/execute/engine.rs`: In `run_one_section`, after `inject_host_with_var`, build the execute/fanout callbacks once and call `vm.install_control_globals(callbacks)`. `run_section_lua` loses the `before_prose` parameter and the callback-building code - it just runs the chunk. The callbacks live on the VM now. `execute` callback passes `None` for `last_reply`. Remove the `has_children` gate.

- `crates/promptforge-core/src/execute/engine.rs` (`run_execute_section`): Already takes `last_reply: Option<&str>` - pass `None` instead of the captured value.

**Impact on `tools.local` handlers:** With control globals installed once and always live, a local tool handler CAN see `jump`/`execute`/`fanout`/`models.infer`. Handlers are allowed to call `execute`, `fanout`, and `models.infer`. Only `jump` is blocked - the engine nils it before calling the handler and restores after. The infer hook stays live (handlers may call `models.infer`).

**Net effect:** ~80-100 lines deleted (install/clear machinery, per-chunk closure creation), ~30 lines changed (execute callback simplification), net ~50-70 lines simpler.

## Implementation

### Where tools live in the architecture

- `Tool` trait (`crates/promptforge-core/src/tools/registry.rs`): `id()`, `wire_name()`, `description()`, `parameters_schema()`, `call(args)`
- `ToolRegistry`: holds `&dyn Tool` by `ToolId`, built from the host's live tool set
- `ToolSchema` (`client/wire.rs`): `{name, description, parameters}` sent to the model
- Tool loop (`execute/tool_loop.rs:215`): dispatches via `dispatch` map (alias to ToolId) then `registry.get(id)` then `tool.call(args)`
- `ToolBinding` (`lua/handles.rs`): carries alias, description, ToolId
- `snapshot_tool_scope` (`lua/vm.rs`): unions `tools.always` + H2 `tools.add` bindings

### The problem

Local tools can't go through `ToolRegistry` because:
1. The handler is a Lua closure tied to a specific section VM
2. The handler needs access to the VM's store and globals
3. They're per-section, not global

### Approach

A local tool lives alongside the `ToolRegistry` path, not inside it:

1. `tools.local(alias, desc, params, handler)` stores the handler as a Lua registry key in a per-VM `LocalTools` map. Also stores the generated `ToolSchema`.

2. When building effective tool schemas for inference, local tools are appended to the schema list alongside registry tools. Local aliases go into the dispatch map with a sentinel `ToolId("local", alias)`. No registry entry is created. The tool loop checks `id.server() == "local"` before calling `registry.get(&id)` - if local, dispatch to the VM's handler instead. No new types, no enum, no signature changes to the tool loop.

3. In the tool dispatch loop, check the local dispatch map first. If the call name matches a local tool, call the handler directly via the VM. Otherwise fall through to `registry.get(&id)`.

4. The handler call happens synchronously on the current thread (it's Lua in the same VM, no async needed). The tool loop runs on the same task as the section's Lua execution, so the handler can be called directly without any bridge.

### Files to modify

- `crates/promptforge-core/src/lua/vm.rs`: Add `LocalTools` storage (alias to schema + handler key). Methods: `register_local_tool`, `call_local_tool`, `local_tool_schemas`.

- `crates/promptforge-core/src/lua/tools_bridge.rs`: Install the `tools.local` function. Validate alias, parse params table into JSON Schema, store handler via `lua.create_registry_value`, record in LocalTools.

- `crates/promptforge-core/src/execute/tool_loop.rs` (~line 215): Accept an optional local tool dispatcher (closure or trait object). Before `registry.get(&id)`, check if alias is local. If so, call via the dispatcher.

- `crates/promptforge-core/src/execute/engine.rs`: Build the local dispatcher from the VM's LocalTools and thread it into `run_prose_inference`.

- `crates/promptforge-core/src/execute/tools.rs` (`ToolBag`/`InferContext`): Include local tool schemas in `prepare()`. Wire local dispatch into the `model:infer` path.

- `crates/promptforge-core/src/execute/scope.rs` (`prepare_scoped_tools`): Append local tool schemas to the schema list.

### Steps

For every step, the coder subagent must audit every function in every file it modifies: is this function truly needed? Can it be simplified? Look at defensive checks and error handling layers - if a mechanism exists to guard against a case that can't actually happen, remove it. Examples: `PoisonError` handling on locks that are never contended, `#[expect]` annotations that outlived their reason, parameters that are always the same value at every call site, functions only called from tests. The `jump_slot` `Arc<Mutex>` is an exception - it stays because persistent closures need owned interior-mutable state across `lua.create_function` calls. When in doubt, simplify and let the tests catch it.

**Component: Control globals install-once (steps 1-4)**

1. **Install `log` and `store` once per section.** Move `install_log` and `install_store_table` out of `run_loaded_with_control` and into a new `vm.install_host_apis()` called once from `run_one_section` after `inject_host_with_var`. Use `lua.create_function` (not scope-bound). The log budget counters and store handle are already on the VM - the closures just capture them. Test: verify `log()` and `store.*` work across multiple Lua chunks in a section.

2. **Install control globals once per section.** Move `tasks`, `execute`, `jump`, `fanout` installation out of `run_loaded_with_control` into `vm.install_control_globals(callbacks)` called once from `run_one_section`. The callbacks need WalkContext so they're built in `run_one_section` after `inject_host_with_var`. Use `lua.create_function` (not scope-bound). Remove the `has_children` gate on fanout. Delete `clear_control_globals`. Test: verify jump/execute/fanout work from multiple chunks in a section.

3. **Delete prologue/epilog split.** Delete `run_prologue_with_control`/`run_epilog_with_control` - one `run_loaded_with_control` for every chunk. Remove `before_prose` parameter from `run_section_lua`. Remove `LUA_PROLOGUE_*` and `LUA_EPILOG_*` observer variants - just use section-level observation. Delete LUA-007 test (`control_globals_are_cleared_even_when_the_block_errors` in lua/tests.rs:1190) - it guards against a case that can't happen (if a chunk errors, the section fails, VM dies). Test: verify observation events fire correctly per chunk without prologue/epilog labels.

4. **`execute()` passes nil reply.** Change `execute` callback to pass `None` to `run_execute_section` for `last_reply` instead of capturing it. The subroutine starts clean. Test: verify `execute()` subroutine sees `reply` as nil.

**Component: models.default rename (step 5)**

5. **Rename models.only to models.default, remove foreclosure.** `models.only` becomes `models.default`. The H2 `models.use` override is restored - no error stub. `models.default` sets the baseline, `models.use` overrides per section. A prompt can have both. `models.use` returns the `LuaModelHandle` for the selected binding. Update `ModelBindings.only()` to `ModelBindings.default()`, `ModelBindingState.only` to `ModelBindingState.default`. Update `resolve_model_binding` to fall back to `default()` when `use()` wasn't called. Update all prompt files, test files, docs, and error messages that reference `models.only`. This is a new commit on top of the `only` rename - history shows always->only->default which is honest about the evolution.

**Model selection rules (enforce with tests):**
- Prose or `models.infer` before any model is set = `ModelRequired` error
- `models.use` can be called at most once per section (existing `AlreadyUsed` error). Set it in the first Lua chunk. After the first model interaction (prose or `models.infer`), the model is locked for the section - changing it would break the KV cache prefix and hide provider boundaries from the programmer
- A section with only prose (no Lua chunks) requires `models.default` - without it, the first prose hits `ModelRequired`
- To use a different model, go to a different section. Context flows via `reply` or `var`
- `models.get(alias):infer(prompt)` is the only way to use a different model within a section - it's a fresh context, no conversation history, no KV cache concerns. String in, string out, blocking call

**Component: LocalTools (steps 6-7)**

6. **Add LocalTools storage to SectionVm.** A struct holding a `Vec<(String, ToolSchema, RegistryKey)>` - alias, prebuilt schema, handler key. Methods: `register_local_tool`, `call_local_tool`, `local_tool_schemas`. Test: register a local tool, call it, verify the handler runs and returns the result.

7. **Install `tools.local` Lua function.** Install in `install_h2_tools` (tools_bridge.rs) alongside `tools.add`. Validate alias (same rules as tools.need). Parse params table: iterate key-value pairs, map type strings to JSON Schema types, build the `ToolSchema`. Store the handler via `lua.create_registry_value`. Add to LocalTools. Bump `ToolRuntime::generation`. Note: H1 support is deferred - `tools.local` in H1 would need `install_live_tools` and H1 VMs are torn down before H2 sections, making H1-registered local tools useless for section work. Document as H2-only for now. Test: call `tools.local` from a chunk, verify the tool is registered with the right schema.

**Component: Schema building and dispatch (steps 8-9)**

8. **Rebuild schemas per prose block.** Split the `seen_prose` gate into two parts: (a) one-shot model resolution and counts install stays gated (runs once at first prose), (b) schema/dispatch rebuild runs on EVERY prose block. Delete `ToolScope` and `snapshot_tool_scope` - they're vestigial wrappers. On every prose block, read the current aliases from `ToolRuntime` (always + added), resolve them against the registry, and build schemas directly. No intermediate struct. This requires updating ALL ToolScope callers: engine.rs, tools.rs (ToolBag), h1.rs, fanout/arm.rs, lua/tests.rs, prepare_scoped_tools. The replacement is a simple function that takes `(bindings, runtime, registry)` and returns `(schemas, dispatch)`. This fixes the latent bug where `tools.add` between prose blocks is silently ignored. Test: `tools.add` between two prose blocks takes effect on the second block.

9. **Dispatch local tools in the tool loop.** Add local aliases to the dispatch map with a sentinel `ToolId("local", alias)`. In the tool loop, check `id.server() == "local"` before calling `registry.get(&id)` - if local, call the VM's handler instead. The tool loop is async; the Lua handler is sync. Call the handler directly on the current thread (the tool loop is already running on the section's task). Guard: nil `jump` before calling handler, restore after. Handlers CAN call `execute()`, `fanout`, and `models.infer`. Test: model calls a local tool, handler runs, result returns to model.

**Component: Model inference API (step 10)**

10. **Add `models.get(alias)` and `models.infer(prompt)`.** `models.get(alias)` returns the `LuaModelHandle` for a pre-declared model without changing the section's model. `models.use` also returns the handle. `handle:infer(prompt)` stays as-is (fresh context, no tools, direct gateway call). Add `models.infer(prompt)` which uses the section's current model (from `models.use` or `models.default`), fresh context, no tools. Both are direct gateway calls, not tool loops. `models.infer` takes exactly one argument - the prompt string. No two-arg form with model selection - use `models.get(alias):infer(prompt)` for that. Test: `models.get` returns handle without changing section model, `models.infer` uses section model, `handle:infer` uses that specific model.

**Component: Tests and docs (steps 11-12)**

11. **Tests.** Prompt with tools.local, model calls it. Handler runs, returns string. Store mutations from handler persist. Multiple calls in one response. Handler error surfaces as tool failure. Wrong arg type from model still reaches handler (Lua is dynamic - no enforcement). Control globals are nil during handler calls. Accumulator pattern: handler appends to a section-global table, epilog reads it (proves same-VM sharing works). Regression test: `tools.add` between two prose blocks takes effect on the second block. Model rules: prose without model errors, `models.use` after prose errors, `models.use` in first chunk works, prose-only section without default errors, `models.infer` before `models.use` errors when no default, `models.get(alias):infer(prompt)` works regardless of section model.

12. **Documentation.** design-core.md new item. guide/src/tools.md section on local tools. guide/src/lua.md API table. Update `execute()` docs to note nil reply. Update `models.get`, `models.infer`, `model:infer` docs.
