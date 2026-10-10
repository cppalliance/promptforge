---
name: Author API reshape
overview: "Reshape the prompt-author API in four changes: role labels stop being Lua globals; tools are named by canonical id, with tools: slots and tool alias globals removed and one shared read-only object per tool; model catalog entries carry an optional provider; and a new plugins table lists required and extra plugins as shared read-only objects. Each change builds on code that already exists. The models-loop-in-Rust plan's work, including handle methods (h:loop, h:infer), the guide, and the per-model tool-name rewrite are excluded."
todos:
  - id: p1-role-globals
    content: "Role-globals change: delete the model half of install_captured_bindings, stop treating model labels as globals in the parser, drop them from the prelude collision list, migrate the 2 role-global calls and 3 other bare role-label reads in 3 test files to models.get"
    status: pending
  - id: p2-tools-by-id
    content: "Tools change: delete tools: slots, slot fill, and tool alias globals; the offering holds every catalog tool and resolves ids; ToolSet swaps bindings for declared; tool objects; tools.offer, always_offer, offer_local, required, extras, get, call and calls by id; consumers including the Workshop run panel, error texts, facade listing, tests"
    status: pending
  - id: p3-model-provider
    content: "Provider change: optional provider on the gateway's Capabilities and its validation, client catalog decode, ModelDescriptor, ModelBinding, handle.provider, facade listing, tests"
    status: pending
  - id: p4-plugins-table
    content: "Plugins change: plugins global with required, extras, get over one shared read-only object per plugin; tool objects gain plugin; RESERVED_NAMES; section and H1 install; tests"
    status: pending
  - id: gates
    content: Run the repository verification commands after each change; the facade listing changes only in the tools change and the provider change
    status: pending
isProject: false
---

# Author API reshape

<product-contract>

## Product Requirements

Prompt authors name tools through frontmatter aliases that also become Lua globals, find role labels installed as globals too, and cannot see which plugins a run has. This plan names every tool by its canonical id, makes the frontmatter declare plugins only, stops the frontmatter from creating any Lua global, and adds a `plugins` table that reports what the run has. It also records each model's provider in its catalog entry, the input a later per-model tool-naming feature needs. The work lands as four changes in the promptforge repository, and every path below is relative to that repository's root.

- Problem and users:
  - Prompt authors. Each `tools:` alias and each `models:` role label is installed as a bare Lua global (`crates/promptforge-internal/lua/src/vm/install.rs`, `install_captured_bindings`, lines 29 to 64). The owner intended Lua-facing names to come from plugin preludes, and these globals block prelude names (`crates/promptforge-internal/engine/src/execute/context.rs`, lines 123 to 126).
  - Tools live in two systems with different rules: frontmatter slots, and the offering of undeclared plugins' tools under generated names (`crates/promptforge-internal/engine/src/execute/context-bound.rs`, lines 38 to 64). Declaring a plugin hides its tools from the offering (line 50), so a required plugin cannot be offered whole.
  - A prompt cannot ask which plugins the run has.
  - Engine maintainers, who keep the slot fill, the alias and label globals, and the reserved-name checks and prelude collision lists that exist only because of those globals.
- Goals:
  - Only the Engine and plugin preludes create Lua globals, and a prompt reaches a role's handle with `models.get(label)`.
  - Lua names every catalog tool by its canonical id, such as `web/fetch`, and the frontmatter declares plugins only.
  - `tools.offer_local` replaces `tools.add_local` with the same arguments and behavior.
  - `tools.offer` and `tools.always_offer` replace `tools.add` and `tools.always`, and take tool ids, tool objects, and arrays of them, so `tools.offer(plugins.get(name).tools)` offers a whole plugin.
  - Each tool and each plugin has one read-only Lua object per VM, and every list holds those same objects.
  - `tools.required()` and `tools.extras()` replace `tools.offered()`, and `tools.get(id)` returns one tool object or nil.
  - A `plugins` table with `plugins.required()`, `plugins.extras()`, and `plugins.get(name)`.
  - An optional provider on each model's catalog entry, readable as `handle.provider`.
  - Each goal is met by extending code that already does most of the job, so the Rust that changes stays small.
- Non-goals:
  - Handle methods `h:loop` and `h:infer`, and `models.loop` and `models.infer` without a leading handle. They already exist on `master`, landed by the models-loop-in-Rust plan (commit `9ca61463b`).
  - The rest of that landed plan's work, the Rust state machine behind `models.loop` and its contract tests, and the models-loop debt-removal plan that follows it.
  - The per-model rewrite of model-facing tool names, such as `WebSearch` for Grok and `web_search` for OpenAI models. Only its provider input lands here.
  - Canonical tool ids inside message-list records.
  - The `promptforge-docs` guide.
- Success criteria:
  - Every surface in Functional Specification behaves as specified, and every removed form fails with the specified error.
  - Both shipped prompts, `prompts/research-person.md` and `crates/workshop/agents/agents/chat.md`, run on the new surface.
  - After section setup, the global table holds only Lua, Engine, and prelude names.
  - The repository's verification commands pass after each change, and the facade listing changes only in the tools change and the provider change, exactly as Technical Design lists.
- Constraints:
  - The Engine stays sans-IO and deterministic. Every list in this plan is fixed before Lua runs: the catalog, MCP tool lists included, is a Host snapshot taken once plugins are ready (`crates/harness-internal/runner/src/host-run.rs`, lines 1 to 7 and 49 to 56; `crates/plugin-mcp/src/connect.rs`, lines 78 to 91).
  - The change is a clean break: prompts are at language version `promptforge: 0`, and removed forms fail at their call or at parse.
  - The 500-line ceiling on `.rs` files (`crates/build-ceiling/src/lib.rs`, line 14) and the flat-directory rule in `AGENTS.md` hold for every new and edited file.
- Open questions: None

## Functional Specification

Authors offer tools to the model by id or by tool object, taken from two lists or from a plugin, and call and count them by id. Each tool and each plugin is one shared, read-only object, so the same tool is the same value in every list. A role's handle comes only from `models.get`, `models.use`, or `models.default`, and carries the model's provider. A `plugins` table lists the declared plugins and the Host's other plugins. Every removed form fails at its call or at parse with a named error, and the model still sees each tool under a generated wire name.

- Actors and workflows:
  - A prompt author writes Lua in section blocks, the H1 body, and the shared library. Section VMs and the H1 VM share one setup path (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, lines 121 to 180), so every surface here exists in both.
  - The Host supplies the plugin catalog and the model catalog. For gateway Hosts, the model catalog is the gateway's `GET /v1/models` (`crates/harness-gateway-client/src/catalog.rs`).
- Inputs and outputs:
  - The new surface at a glance, frontmatter first:

    ```yaml
    plugins:
      - web
      - wg21-papers
    models:
      analyst: {}
    ```

    ```lua
    -- models: a role's handle comes from models.get
    local analyst = models.get('analyst')
    local provider = analyst.provider           -- 'xai', or nil when unknown
    -- analyst:loop(msgs) and analyst:infer(prompt) come from the models-loop-in-Rust plan

    -- tools, always by canonical id
    tools.offer('web/fetch', 'Fetch one page.')
    tools.offer({ 'web/search', 'web/fetch' })
    tools.offer(tools.get('web/fetch'))         -- the same tool, as its object
    if tools.get('wg21-papers/search') then tools.offer('wg21-papers/search') end
    tools.always_offer(tools.required())
    tools.offer(tools.extras())                 -- tool objects: id, description, plugin
    local page = tools.call('web/fetch', { url = 'https://example.com' })
    local fetches = tools.calls['web/fetch']

    -- a local tool is created and offered in one call, then called by alias
    tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(a)
      return 'got ' .. a.value
    end)
    local out = tools.call('grab', { value = 'hi' })

    -- plugins
    local declared = plugins.required()         -- plugin objects: name, tools
    local others = plugins.extras()
    local papers = plugins.get('wg21-papers')   -- the plugin object, or nil
    if papers then tools.offer(papers.tools) end

    -- one object per tool and per plugin, shared by every list
    assert(plugins.get('web').tools[1] == tools.required()[1])
    assert(plugins.get('web') == plugins.get('web'))
    assert(tools.get('web/fetch').plugin == plugins.get('web'))
    ```

  - Models:
    - A role label under `models:` is no longer a Lua global. `local analyst = models.get('analyst')` replaces a bare `analyst`.
    - `handle.provider` is the model's provider id, such as `xai`, or nil when the catalog names none.
  - Tools:
    - A tool object is read-only userdata with `id`, `description` (the catalog text, whatever override is set), and `plugin`, which is the plugin's own object, so `tools.get('web/fetch').plugin == plugins.get('web')`. A VM holds one per offerable catalog tool, so every list that names a tool holds that same value, and `==` compares identity. The tools change ships `id` and `description`, and the plugins change adds `plugin`. Local tools have no tool object.
    - A tool spec is a catalog tool id (`web/fetch`), a tool object, or an array of these.
    - `tools.offer(spec, description?)` offers to the model in the current section, and `tools.always_offer(spec, description?)` offers in every section. Both take the same arguments. The description override applies only to a single id or tool object. Overrides keep today's placement and precedence: `tools.always_offer`'s override is prompt-wide, `tools.offer`'s applies to its section only, and a section's override wins over the prompt-wide one, which wins over the catalog description.
    - `tools.required()` lists the tools of declared plugins, and `tools.extras()` the tools of every other plugin in the catalog. Each returns a fresh array of tool objects in id order, and lists only tools that can be offered.
    - `tools.get(id)` returns the tool object for a catalog id, or nil when the string names no offerable catalog tool, a local tool's alias and a malformed id included. A non-string argument raises `tools.get takes a tool id, got {type}`.
    - `tools.call(spec, args)` takes a catalog id, a tool object, or a local tool's alias. `tools.calls` is keyed by id for catalog tools and by alias for local tools, and a model call and a script call of the same tool count under the one id.
    - `tools.offer_local(alias, description, params, handler)` replaces `tools.add_local` with the same arguments and behavior: it creates a local tool in the current section, offers it to the model, and makes it callable with `tools.call(alias, args)`. Its registration errors keep their text with `tools.offer_local` in place of `tools.add_local`. Its refusal of an alias that duplicates a tool slot goes with the slots, and no check replaces it: a local alias may equal a catalog tool's wire name, and the local tool then wins in scope, as today.
    - `tools.allow_tasks` and the four task built-ins keep their current behavior.
  - Plugins:
    - A plugin object is read-only userdata with `name` and `tools`. A VM holds one per plugin, so `plugins.get(name)` returns the same value on every call.
    - A plugin object's `tools` reads a fresh array of that plugin's tool objects in id order, the same objects `tools.required()` and `tools.extras()` hold, so `tools.offer(plugin.tools)` offers the whole plugin.
    - `plugins.required()` lists every declared plugin, even one with no tools. `plugins.extras()` lists every other plugin that has tools in the catalog. Both return fresh arrays of plugin objects in name order. `plugins.get(name)` returns the plugin object or nil.
  - What the model sees: each offered catalog tool under its generated wire name, the id with `/` and `.` replaced by `_` (`web/fetch` becomes `web_fetch`), as the offering names tools today (`context-bound.rs`, line 56). Message-list records keep the wire name the model returned.
  - Frontmatter: `plugins:` lists the required plugin ids, and `tools:` no longer exists.
- States and validation:
  - Every declared plugin is required: `plugins:` is a plain list of ids (`crates/promptforge-internal/parser/src/build-frontmatter.rs`, lines 189 to 193).
  - Role labels keep the name grammar, but the reserved-name check and the tool-and-model clash check, which exist only because labels became globals, no longer apply to them.
  - `tools.offer` and `tools.always_offer` check every entry before recording any, so one bad entry records nothing.
  - Scope order stays as today: prompt-wide tools first, then the section's offers in first-offer order, each tool once.
  - Wire-name conflicts keep today's rules. A catalog tool whose wire name repeats an earlier one, matches a task built-in, or fails the schema check is left out with a log line (`context-bound.rs`, `offer_refusal`, lines 70 to 93, without its alias rule). A local tool wins over a catalog tool with the same wire name (`crates/promptforge-internal/engine/src/execute/scope.rs`, lines 66 to 74). A script can still call a catalog tool that was left out, by its id.
  - A provider id is lowercase `[a-z0-9._-]+`, checked by the gateway's capability validation for `[[model]]` and `[[local_model]]` alike.
- Errors and recovery: each error below is raised at the call, so `pcall` catches it, and is a `lua`-kind error value unless it names another kind.
  - `tools.offer: "{id}" is not a catalog tool in this run`, with `tools.always_offer` in place of `tools.offer` for that function. A string that fails the tool-id grammar, or names a local tool, gets the same error.
  - `Tool objects are frozen: cannot assign field {key:?}` and `Plugin objects are frozen: cannot assign field {key:?}` for an assignment to either object, the first being today's Tool object text (`crates/promptforge-internal/lua/src/tools/userdata.rs`, lines 64 to 70).
  - The argument-shape errors of today's `tools.add` (`crates/promptforge-internal/lua/src/tools/decode.rs`, lines 67, 90, 97, and 105), renamed for the function that raises them.
  - A `tools.call` name that is neither a local tool nor a catalog id raises kind `unbound_tool` with `tool {name:?} is not a tool in this run; catalog tools: {ids:?}`, in place of today's list of bound aliases (`crates/promptforge-internal/engine/src/error.rs`, lines 253 to 260). The list holds the id of every catalog tool the run can offer.
  - Reading `tools.calls[key]` for a key with no count raises `tools.calls: {key:?} has no seeded count; seeded names: {names:?}`, followed by ` (a catalog tool that was neither offered in this section nor called with tools.call)` when the key is the id of a tool the run can offer, by nothing when no name is seeded, and otherwise by ` - check for typos or offer it with tools.offer`. These replace today's bound-slot and `tools.add` suffixes (`crates/promptforge-internal/lua/src/tools.rs`, lines 99 to 110).
  - A model call to a catalog tool its section did not offer keeps today's out-of-scope error, with the suffix ` (a catalog tool that was not offered in this section)` in place of the bound-slot one (`error.rs`, line 235).
  - `tools.add`, `tools.always`, `tools.offered`, and `tools.add_local` no longer exist, so calling one fails as a call of nil.
  - A leftover `tools:` key fails the parse as an unknown frontmatter key, through `deny_unknown_fields` on the frontmatter struct (`build-frontmatter.rs`, line 43).
- Security and privacy behavior:
  - Trust rules stay as they are. Untrusted tool output is wrapped as today, and tool access is unchanged, since any prompt can already reach any catalog tool through the offering or by full id (`crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs`, lines 178 to 182).
  - The provider is catalog metadata: Lua sees the provider id alone, while credentials and endpoints stay in the gateway.
- Acceptance criteria:
  - Every surface and error above has a test in Testing Plan.
  - `prompts/research-person.md` and `crates/workshop/agents/agents/chat.md` have no `tools:` key and offer tools with `tools.offer` and `tools.always_offer` by id.

</product-contract>
<implementation-contract>

## Technical Design

Each change builds on code that already does most of its job. The tools change reuses the offering, which already binds catalog tools under generated wire names and is already what section scope, description overrides, dispatch, and the advertised map resolve against: the slot list beside it goes, the offering holds every catalog tool, and lookups accept canonical ids. The role-globals change deletes half of one install function. The provider change adds one field to the capability metadata the gateway already carries from both model configs to its catalog, then passes it through the descriptor and the binding to the handle. The plugins change adds one Engine global over the tool set.

- Architecture:

  ```mermaid
  flowchart LR
    GW[capabilities] -->|provider| CAT[v1 models]
    CAT --> DESC[ModelDescriptor]
    DESC --> BIND[ModelBinding]
    BIND --> HANDLE[model handle]
    HC[Host catalog] --> TL[offering]
    FM[plugins key] --> TL
    TL --> LUA[Lua tables]
  ```

  - The model sees wire names and Lua uses ids, and `ToolSet::offered_binding` is the one place that maps between them by accepting either. Everything keyed by wire name today stays keyed by wire name: section scope, description overrides, the round's advertised map, and dispatch.
  - Tool objects and plugin objects are built once per VM at install and kept in registry tables, one keyed by tool id and one by plugin name, the way `tools.rs` already keeps local handlers (`LOCAL_HANDLERS_REGISTRY`, lines 44 to 47).
- Modules and interfaces:
  - Role globals (role-globals change):
    - `install_captured_bindings` (`crates/promptforge-internal/lua/src/vm/install.rs`, lines 29 to 64) loses its role-label half (lines 48 to 63). The tools change deletes the rest, the function, and its call (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, line 180).
    - The parser sets `installs_global` to false for model labels (`crates/promptforge-internal/parser/src/contract/models.rs`, lines 127 to 142), which also turns off their reserved-name checks, and removes the tool-and-model clash check (`crates/promptforge-internal/parser/src/contract.rs`, lines 255 to 263). `frontmatter_aliases` (`context-bound.rs`, lines 115 to 124) drops the model labels.
  - Tools (tools change):
    - `ToolSet` (`crates/promptforge-internal/lua/src/handles.rs`, lines 140 to 153) swaps `bindings`, the slots, for `declared`, the frontmatter's plugin ids. `offered` keeps its name and type and now holds every offerable catalog tool, and `always` keeps holding wire names. `offered_binding` (lines 209 to 213) matches a wire name or a canonical id. `ToolBinding` is unchanged: its `alias` already holds the wire name of every offered tool. `ToolView::bindings` swaps for `declared`, which `RunState::tool_set_snapshot` reads (`context.rs`, lines 303 to 309).
    - `bound_tool_set` (`context-bound.rs`, lines 24 to 36) drops the slot loop and fills `declared`. `offered_bindings` (lines 42 to 64) drops the declared-plugin filter (line 50) and the alias set (lines 45 and 46), and `offer_refusal` drops its alias rule (lines 76 to 78). `catalog_bindings` and `RunState::catalog_binding` (`context.rs`, lines 314 to 316) stay, so a script still reaches, by id, a catalog tool that could not be offered.
    - Each new Lua function is today's function in `crates/promptforge-internal/lua/src/tools.rs` with its lookup widened:
      - `offer` is `add` (lines 177 to 214): each entry resolves through `offered_binding` in place of the alias grammar and the slot lookup, and the binding's wire name is recorded in section scope and overrides as today.
      - `always_offer` is `always` (lines 216 to 244): it decodes its arguments like `offer`, finds the binding in `offered` instead of `bindings`, keeps the prompt-wide override on that binding as today, and records its wire name in `always`.
      - `required` and `extras` are `offered` (lines 252 to 273), split by whether the tool's plugin is in `declared` and returning tool objects in place of records.
      - `get` reads the tool-object table.
      - `offer_local` is `add_local` (lines 275 to 318) without its slot-duplicate check.
      - `calls` counts are keyed by `binding.id()` in place of `binding.alias()` at the sites that seed and increment them (`tools.rs`, line 146; `crates/promptforge-internal/engine/src/execute/section_context.rs`, line 233; `scheduler/tool_call.rs`, lines 190 and 191), while local tools keep their alias. The unseeded-key diagnostic (`tools.rs`, lines 99 to 110) takes the text in Errors and recovery, built from the offered tools' ids in place of the slot aliases.
    - `LuaToolHandle` (`crates/promptforge-internal/lua/src/tools/userdata.rs`) becomes the tool object, built from a binding, and keeps its frozen-assignment refusal (lines 64 to 70). `tool_alias` (`crates/promptforge-internal/lua/src/tools/decode.rs`, lines 36 to 54) already accepts it. `collect_tools_add_entries` (lines 80 to 123) takes the calling function's name for its errors, and its record branch and `record_name` (lines 18 to 23) go, since tool objects replace records.
    - The engine's slot lookups go: `.binding(alias)` in dispatch (`scheduler/tool_call.rs`, line 179) and in `binding_for_scope` (`crates/promptforge-internal/lua/src/vm/state.rs`, line 237). `unbound_tool_call` (`scheduler/dispatch.rs`, lines 33 to 42) lists the ids in `ToolSet.offered`, every catalog tool the run can offer, and the out-of-scope check (`scheduler/chat.rs`, lines 269 to 271) asks `offered_binding`.
    - Deleted: slot fill (`crates/promptforge-internal/engine/src/execute/fill.rs`, `fill_tool_bindings`, called at `environment.rs`, line 123), `ToolBindings` (`bindings.rs`), `RunContext.tool_bindings` and its accessor (`config.rs`, lines 102, 142, 308 to 310, and 366), `Requirements.missing_tools`, the parser's `tools:` key and slot types, `frontmatter_aliases` with the alias list passed to prelude collision checks (`crates/promptforge-internal/lua/src/prelude.rs`), and the slot-plugin chain in Host requirements (`crates/harness-internal/runner/src/host-run.rs`, lines 128 to 167).
  - Provider (provider change):
    - `Capabilities` (`crates/gateway-api-types/src/metadata.rs`, lines 78 to 117) gains `provider`. It is already flattened into `[[model]]` and `[[local_model]]` (`crates/gateway/config/src/config.rs`, lines 317 and 387), copied into the routing table (`crates/gateway/app/src/routing.rs`, line 144; `crates/gateway/local/src/runtime/start.rs`, line 245), and flattened into the catalog's `ModelInfo` (`metadata.rs`, line 194), so this one field reaches the config, the routing table, and `/v1/models`. `validate_capabilities` (`crates/gateway/config/src/config/validate.rs`, lines 212 to 259) checks it for both model kinds.
    - The client's `ModelsListEntry` (`crates/harness-gateway-client/src/catalog.rs`, lines 20 to 28) reads it and passes it to the descriptor.
    - `ModelDescriptor` (`crates/promptforge-internal/types/src/models.rs`, line 147) and `ModelBinding` (`crates/promptforge-internal/model-client/src/model/options.rs`, lines 89 to 99) hold it, `bound_model_set` (`context-bound.rs`, lines 148 to 183) copies it, and `LuaModelHandle` (`crates/promptforge-internal/lua/src/models-userdata.rs`) exposes it.
  - Plugins (plugins change):
    - A new lua-crate module, `plugins.rs`, installs `plugins` right after the `install_tools` call in section setup (`vm/install.rs`, line 135). It builds one plugin object per declared plugin and per other plugin with offered tools, keeps them in its registry table, and the three functions and a tool object's `plugin` getter read that table.
    - `plugins` joins `RESERVED_NAMES` (`crates/promptforge-internal/lua/src/globals.rs`, line 157). The prelude sandbox's `VISIBLE_GLOBALS` (`prelude.rs`, line 31) is unchanged.
- File and public API changes:
  - The tools change removes these items from the facade listing (`crates/promptforge/public-api.txt`): `ToolSlot` (line 18), `ToolSlots` (63) and its methods (600 to 603), `ToolBindings` (64) and its methods (605 to 609), `RunContext::tool_bindings` (438), `Frontmatter::tools` (592), `Requirements::missing_tools` (723), and `ToolSlot::Exact` (1220 and 1221). Each exists only for slot fill (`fill.rs`, lines 13 to 52), and these are the change's only facade edits. `ToolSet`, `ToolBinding`, and `ToolView` are outside the facade.
  - The provider change adds `ModelDescriptor::with_provider`, `ModelDescriptor::provider`, `ModelBinding::with_provider`, and `ModelBinding::provider` to the listing; both types are public today (`public-api.txt`, lines 518 to 527, 539 to 543, and 1374).
  - The role-globals change and the plugins change leave the facade unchanged.
  - Consumers moved to the new surface:
    - the Workshop prompt DTO's `tools` field (`crates/workshop/server/src/routes/prompts.rs`, lines 59 and 230 to 234), the run panel's tool rows that read it (`crates/workshop/ui/src/parts/run/run-rows.ts`, lines 184 to 188), and their UI tests (`crates/workshop/ui/test/run-api.mjs`, lines 104 to 107; `crates/workshop/ui/test/run-panel.mjs`, line 199);
    - `prompts/research-person.md`, `crates/workshop/agents/agents/chat.md`, and the fixture in `crates/harness-gateway-client/src/wire/request-tests.rs`;
    - the shim comment that names `tools.add` (`crates/promptforge-internal/lua/src/__impl_coro.lua`, line 71). The shim's code is unchanged.
- Data, persistence, failure, security, and privacy constraints:
  - `ModelDescriptor` and `ModelBinding` live in memory only. `EffectRecord::Chat` records selected binding fields (`crates/promptforge-internal/engine/src/execute/run/effect.rs`, lines 172 to 198 and 230 to 256), and the provider stays out of it.
  - Effect and event `alias` fields keep their shape. Because a script call by id now resolves through `offered_binding`, they hold the wire name for every call to an offerable tool, model or script, and the id only for a script call to a catalog tool that could not be offered (`effect.rs`, lines 113 to 117 and 257 to 262).
  - The `/v1/models` response gains one optional field, which clients that ignore unknown fields skip.

### Rust declarations

Every new or changed declaration, checked against `master` at `2b53936e6`. Bodies are elided, and declarations not listed keep their current form.

**`promptforge-lua`, `handles.rs`** (tools change). `ToolBinding` is unchanged apart from its docs. `ToolSet::bindings`, `ToolSet::binding`, and `ToolView::bindings` go.

```rust
/// The run's tool set: the declared plugins, the prompt-wide offers, and
/// every catalog tool the run can offer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSet {
    /// The plugins the frontmatter declares, in declaration order.
    pub declared: Vec<PluginId>,
    /// The wire names `tools.always_offer` recorded, in first-offer order.
    pub always: Vec<String>,
    /// Every catalog tool the run can offer, in id order, each bound under
    /// its wire name.
    pub offered: Vec<ToolBinding>,
}

impl ToolSet {
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn for_test(declared: Vec<PluginId>, always: Vec<String>, offered: Vec<ToolBinding>) -> Self;
    #[must_use]
    pub fn from_parts(declared: Vec<PluginId>, always: Vec<String>, offered: Vec<ToolBinding>) -> Self;
    #[must_use]
    pub fn declared(&self) -> &[PluginId];
    /// The offered binding whose wire name or canonical id is `name`. Wire
    /// names and local aliases never contain `/` and ids always do, so a
    /// name matches at most one way.
    #[must_use]
    pub fn offered_binding(&self, name: &str) -> Option<&ToolBinding>;
}

pub trait ToolView: Send + Sync {
    /// Replaces `bindings` in `RunState::tool_set_snapshot`.
    fn declared(&self) -> Result<Vec<PluginId>>;
    // `always` and `offered` keep their signatures.
}
```

**`promptforge-lua`, `tools.rs`, `tools/decode.rs`, and `tools/userdata.rs`** (tools change). `install_tools`, `ToolsAddEntry`, and `tool_alias` keep their signatures.

```rust
// tools.rs
/// The registry key of the VM's tool-object table: one object per offered
/// tool, keyed by id, built when `install_tools` runs.
const TOOL_OBJECTS_REGISTRY: &str = "promptforge.tools.objects";

/// The VM's tool object for the catalog id `id`, or `None` when the run
/// cannot offer that tool.
pub(crate) fn tool_object(lua: &Lua, id: &str) -> mlua::Result<Option<AnyUserData>>;

// tools/decode.rs: the record branch and `record_name` go, and `call`
// names the function in every error.
pub(super) fn collect_tools_add_entries(call: &str, args: Variadic<Value>) -> mlua::Result<Vec<ToolsAddEntry>>;

// tools/userdata.rs
/// One catalog tool as Lua sees it: `id` and `description`, and from the
/// plugins change `plugin`, all read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaToolHandle {
    id: ToolId,
    description: String,
}

impl LuaToolHandle {
    #[must_use]
    pub(crate) fn from_binding(binding: &ToolBinding) -> Self;
    /// Replaces `name` for `tool_alias`.
    #[must_use]
    pub(crate) fn id(&self) -> &ToolId;
}
```

The `UserData` impl for `LuaToolHandle` keeps its `__newindex` refusal and swaps the field getters `name`, `parameters`, `wire_name`, and `untrusted` for `id` and `description`.

**`promptforge-lua`, `plugins.rs`, new** (plugins change)

```rust
//! The `plugins` table: one read-only object per plugin the run knows.

/// The registry key of the VM's plugin-object table, keyed by name.
const PLUGIN_OBJECTS_REGISTRY: &str = "promptforge.plugins.objects";

/// One plugin as Lua sees it: `name`, and `tools`, which reads a fresh
/// array of the plugin's tool objects in id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaPluginHandle {
    name: PluginId,
    /// The ids of the plugin's offered tools, in id order.
    tools: Vec<String>,
}

/// The VM's plugin object for `name`, read by `plugins.get` and by a tool
/// object's `plugin` getter, or `None` for a plugin the run does not know.
pub(crate) fn plugin_object(lua: &Lua, name: &str) -> mlua::Result<Option<AnyUserData>>;

/// Builds one plugin object per declared plugin and per other plugin with
/// offered tools, then installs `plugins.required`, `plugins.extras`, and
/// `plugins.get`.
///
/// # Errors
/// Returns [`Error::Lua`] if an object or function cannot be created or installed.
pub(crate) fn install_plugins(lua: &Lua, globals: &Table, set: &Arc<Mutex<ToolSet>>) -> Result<()>;
```

The `LuaPluginHandle` `UserData` impl has the field getters `name` and `tools` and a `__newindex` that raises `Plugin objects are frozen: cannot assign field {key:?}`. In the same change, `LuaToolHandle` gains a `plugin` getter over `plugin_object`, and `RESERVED_NAMES` becomes `[(&str, Reserved); 61]` with `("plugins", Reserved::EngineGlobal)`.

**Engine, parser, and prelude** (tools change). Signatures stay except these two; the deletions are those in Modules and interfaces.

```rust
// engine, context-bound.rs: the alias rule goes with the slots.
fn offer_refusal(name: &str, tool: &ToolDescriptor, taken: &BTreeSet<String>) -> Option<String>;

// lua, prelude.rs: the alias list goes.
pub fn install_preludes(lua: &Lua, preludes: &[Prelude]) -> Result<()>;
```

`Frontmatter` loses its `tools: ToolSlots` field and `tools()` accessor. `ContractKeys.installs_global` goes as well, because once the role-globals change has set it to false for model labels, only the slots set it.

**Provider change.** `ModelDescriptor::new` and `ModelBinding::new` keep their signatures, and `bound_model_set` calls `with_provider` when the descriptor names one. `LuaModelHandle` gains a `provider` field getter returning `Option<String>` from `binding().provider()`.

```rust
// gateway-api-types, metadata.rs, on `Capabilities`
    /// The model's provider id, such as `xai`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

// gateway-config, validate.rs, inside `validate_capabilities`: a provider
// outside `[a-z0-9._-]+` is refused with
// "{label} {name} provider must use lowercase letters, digits, '.', '_', or '-'".

// harness-gateway-client, catalog.rs, on `ModelsListEntry`
    /// The model's provider id, when the gateway names one.
    #[serde(default)]
    provider: Option<String>,

// promptforge-types, models.rs, on `ModelDescriptor`: a `provider: Option<String>` field, and
impl ModelDescriptor {
    /// Sets the provider id the catalog names for this model.
    #[must_use]
    pub fn with_provider(self, provider: impl Into<String>) -> Self;
    /// Returns the provider id, or `None` when the catalog names none.
    #[must_use]
    pub fn provider(&self) -> Option<&str>;
}

// promptforge-model-client, model/options.rs, on `ModelBinding`: a `provider: Option<String>` field, and
impl ModelBinding {
    /// Sets the model's provider id.
    #[must_use]
    pub fn with_provider(self, provider: impl Into<String>) -> Self;
    /// Returns the provider id, or `None` when the catalog names none.
    #[must_use]
    pub fn provider(&self) -> Option<&str>;
}
```

</implementation-contract>
<verification-contract>

## Testing Plan

Each change migrates the tests its removals break and adds a test for every new surface and error. The role-globals change rewrites every test that reads a role label as a global. The tools change carries the heaviest migration, rewriting every test that embeds `tools:` frontmatter, calls a removed function, builds a `ToolSet` with slot bindings, or reads a tool alias as a global.

- Unit:
  - Role globals: no role-label global after setup, in a section VM and the H1 VM; `models.get(label)` returns the role's handle; a role label that is a reserved name, or that equals a tool alias, now parses.
  - Tool alias globals: no tool alias global after setup, so the global table holds only Lua, Engine, and prelude names.
  - Tools:
    - tool object fields `id` and `description`, with the catalog description kept when an override is set; assignment to a tool object refused;
    - decoding ids, tool objects, and arrays; all-or-nothing recording; the not-found error for an absent id, a malformed string, and a local alias;
    - `offered_binding` resolving a wire name and an id to the same binding;
    - `required` and `extras` membership, with a declared plugin's tools in `required`; identity, so the same tool is `==` across `tools.required()`, `tools.extras()`, and `tools.get`, while each list call returns a new array;
    - `tools.get` hit, miss, local alias, and malformed id, with a hit `==` to the same tool in `tools.required()`; `tools.offer(tools.get(id))` matching `tools.offer(id)`;
    - `always_offer` taking an array and an extra, with its prompt-wide override and a section override in today's precedence;
    - `tools.offer_local` creating, offering, and answering a local tool as `tools.add_local` did, with its registration errors under the new name;
    - `calls` keyed by id, with a model call and a script call of one tool counting under it, and a local tool under its alias;
    - the `unbound_tool` and out-of-scope texts; a leftover `tools:` key refused at parse.
    - The fresh-record test `tools_offered_returns_a_fresh_plain_record_per_offered_tool_in_order` (`crates/promptforge-internal/lua/src/tools/tests-offering.rs`) gives way to the identity test.
  - Provider: a `[[model]]` and a `[[local_model]]` with and without `provider`, and a refused one; `/v1/models` with and without the field; client decode with and without it; `handle.provider` nil and set.
  - Plugins: `required` lists a declared plugin with no tools; `extras` excludes declared plugins; `get` hit and miss; `plugins.get(name) == plugins.get(name)`, and the same object appears in `required` or `extras`; a plugin's `tools` holds the shared tool objects in a new array per read and passes to `tools.offer`; assignment to a plugin object refused; every tool object's `plugin` is `==` to its plugin's object; the table exists in the H1 VM; the reserved-name two-way test (`crates/promptforge-internal/engine/src/lua/tests/globals.rs`, lines 86 to 118) passes with `plugins` listed.
- Integration and end-to-end:
  - A run offers a declared plugin with `tools.offer(plugins.get(name).tools)`, and the model calls one of its tools by wire name. Another run offers an extra the same way after checking `plugins.get`.
  - A script calls by id a catalog tool that was left out of the offering, and the call still runs.
  - Both shipped prompts run end to end, and the Workshop run panel shows no tool rows.
- Regression, security, and performance:
  - Tools change migration: about 27 files embed `tools:` frontmatter and about 15 call `tools.add`, and every `tools.add_local` call moves to `tools.offer_local`. The heaviest are `crates/promptforge-internal/lua/src/tests/tool_scoping.rs`, `crates/promptforge-internal/lua/src/tools/tests-offering.rs`, the engine tests `debug_and_counts.rs`, `models_loop.rs`, `chat_record_rebuild.rs`, `model_and_reply.rs`, and `offering.rs` under `crates/promptforge-internal/engine/src/execute/tests/`, and the parser's `contract/tests*.rs`. Tests that build a `ToolSet` move their slot bindings into `offered` under wire names and pass `declared`. The Workshop UI tests `run-api.mjs` and `run-panel.mjs` drop their tool-slot expectations.
  - Role-globals change migration: at `2b53936e6`, 2 method calls on a role-label global, `writer:loop(msgs)` (`crates/harness/tests/suite/host.rs`, line 55) and `fast:infer("yo")` (`crates/promptforge-internal/engine/src/lua/tests/shims.rs`, line 377), and 3 other bare reads of one (`host.rs`, line 43; `crates/promptforge-internal/engine/src/execute/tests/live_infer.rs`, line 160; `shims.rs`, lines 401 and 402), move to `models.get`. The other `writer:` and `other:` calls in the engine tests already run on a local from `models.get` or `models.default`, or on a global the test assigns itself, and stay as they are.
  - Tests that pin the removed error texts (`crates/promptforge-internal/lua/src/tools.rs`, lines 103 to 108, 188, 228, and 298; `crates/promptforge-internal/engine/src/error.rs`, lines 235 and 253) move to the new texts.
- Exit criteria:
  - The repository's verification commands, listed in `AGENTS.md`, pass after each change.
  - The facade surface check shows no diff after the role-globals change and the plugins change, and exactly the Technical Design items after the tools change and the provider change, with the listing regenerated.

</verification-contract>
<decision-record>

## Decision Record

The owner set the direction for each surface in conversation, and the implementation choices follow from building on code that already does most of each job. Superseded options are recorded as rejected alternatives.

- Decisions:
  - Only the Engine and plugin preludes create Lua globals, so tool alias globals and role-label globals both go. User's words: "wait, what alias globals? that wasn't supposed to work that way. the aliases were supposed to come from the bespoke Lua preludes." The user chose removing role globals when asked.
  - The frontmatter declares plugins only, and `tools:` is removed. User's words: "the YAML granularity just needs the plugin level, we dont need to overcomplicate it."
  - Lua names tools by canonical id, and the model sees a generated name. User's words: "the model should see the generated name, and the Lua should use the canonical name (web/fetch) and there's a reason, because we want to rewrite the model-facing name depending on the model, because Grok wants WebSearch while ChatGPT wants web_search for example".
  - `tools.offer` and `tools.always_offer` replace `tools.add` and `tools.always`. User's words: "we should change tools.add to tools.offer and we should change tools.default to tools.always_offer". The prompt-wide function was `tools.always`, and `always_offer` was kept over `default` because prompt-wide tools add to a section's offers instead of serving as a fallback.
  - Whole-plugin offering is `tools.offer(plugins.get(name).tools)`, which works for declared plugins and extras alike. Each MCP server is its own plugin, named by the Host after the server (`crates/plugin-mcp/src/lib.rs`, lines 4 to 7), so `name` is a server name such as `wg21-papers`. User's words: "I would like a way to offer the whole plugin's tool set, tools.offer("mcp/*") or something?", then "do we need "mcp/*" syntax or would it be better as tools.offer(plugins.get("mcp").tools)?" The user chose dropping the pattern when asked.
  - `tools.required()` and `tools.extras()` replace `tools.offered()`, and they compose with `tools.offer` instead of having dedicated `offer_required()` and `offer_extras()` functions. User's words: "I think the syntax should be tools.required() and tools.extras()", then "I agree with what you are saying" to composing them.
  - `tools.offer_local` replaces `tools.add_local`, keeping its arguments and its automatic offer. A local tool that is created but not offered has no real use, so creating and offering stay one call, and a separate name keeps `tools.offer` to one argument list. User's words: "tools.add_local becomes tools.offer_local but my question, should we just use tools.offer and detect if the 1st param is a function?", then "I dont see a point to defining a tool that isn't offered, do you?" The user chose `tools.offer_local` when asked.
  - `tools.get(id)` returns one tool object or nil, mirroring `plugins.get`, and `tools.offer`, `tools.always_offer`, and `tools.call` keep taking id strings beside tool objects. Strings stay because the not-found error names the mistyped id, and local tools are called by alias anyway. User's words: "what about tools.offer(tools.get("web/fetch")) ?" The user chose keeping strings when asked.
  - A `plugins` table with `required`, `extras`, and `get`. `get` covers the common presence check. User's words: "dont we need a `plugins` object that the prompt can inspect to see what plugins are availalbe? and it would be plugins.required() and plugins.extras()?"
  - A plugin's `tools` field holds tool objects instead of id strings, so it passes straight to `tools.offer` and shows each tool's description. User's words: "plugins.tools() to return an array of tools?", then the user chose a field of full tools over a separate function when asked.
  - Each tool and each plugin is one read-only object per VM, and every list returns a fresh array of those same objects. Read-only is what makes sharing safe: a write to a shared plain table would show up in every list that holds it. The pattern already exists in model handles, message-list record views, and today's Tool object. User's words: "they should "point" to the same object, i.e. there is only one tool object and it can exist in multiple contains". The user chose shared objects for plugins too when asked.
  - A tool object's `plugin` field is the plugin's own object, not its name, so `tools.get('web/fetch').plugin == plugins.get('web')`. The getter reads the plugin-object table at access time, which keeps the tools and plugins tables free of any install-order dependency. The user chose this when asked.
  - The model's provider is an optional field on its catalog entry, filled by the Host from its gateway config. The user chose the catalog field over Engine-side guessing, then narrowed the scope: "this plan should just add the model provider do not implement the whole model-facing tool rename feature".
  - Every change extends code that already does most of its job, so the planned functions arrive with the least new Rust. User's words: "see if you can reduce the public API by reusing existing Rust or Lua constructs ... We are looking to preserve the planned functionality while reducing the amount of Rust changes necessary to achieve it." In particular:
    - The tools change reuses the offering. `ToolSet.offered`, `ToolBinding.alias` as the wire name, `ToolRuntime`, `current_tool_bindings`, the advertised map, and dispatch all keep working by wire name, and canonical ids enter through `offered_binding` alone.
    - The provider rides on the gateway's `Capabilities`, which already reaches both model configs, the routing table, and `/v1/models`.
    - `catalog_bindings` stays, so script calls by id to a tool that cannot be offered keep today's path.
    - The tool object is today's `LuaToolHandle` with new fields, and `tool_alias` already decodes it.
    - The new registry tables follow the existing local-handler table.
  - The guide stays as it is. User's words: "it has to be completely rewritten soon anyway so dont waste time with it."
  - This plan holds only the work beyond the models-loop-in-Rust plan. User's words: "leave out anything in models_loop_in_rust_5e81c0d4.plan.md and only plan for the additional stuff". That plan's Phase 0 delivered handle methods (`h:loop`, `h:infer`) and the namespace forms without a handle, so both are excluded here. Removing role-label globals sits outside that plan and stays here.
  - Role-label global removal lands as its own first change, ahead of the tools change. It is small, independent of the tools work, and shrinks the largest change. Tool alias globals stay with the tools change, because removing them alone would migrate their call sites twice. The user chose the split when asked.
- Rejected alternatives:
  - Keeping `tools:` slots, as aliases or as per-tool requirements. Reason: they form a second tool system and install globals, and plugin-level declaration covers what the frontmatter needs to say. Revisit: if prompts need prepare to refuse a run over one missing tool.
  - Keeping role-label globals so `analyst:loop(msgs)` works directly. Reason: they block prelude names like any other frontmatter global. Revisit: never.
  - A new tool list with renamed fields: a `wire_name` field and a `default_wire_name` function on `ToolBinding`, `ToolSet.tools`, `ToolSet::tool`, `ToolSet::is_required`, and `always` as ids. Reason: `alias` already holds the wire name, and everything keyed by it would have to move for no change in behavior. Revisit: with the per-model rename, which replaces the wire name anyway.
  - Translating `tools.calls` keys from id to wire name at read time. Reason: keying the counts by id at the sites that seed and increment them keeps every listed key in the names authors use. Revisit: never.
  - The `plugins` table as a Lua chunk. Reason: tool objects are Rust userdata whose `plugin` getter must return the plugin object, and the crate has been moving table-building chunks into Rust. Revisit: never.
  - Plugin objects as read-only proxy tables (`crates/promptforge-internal/lua/src/proxy.rs`). Reason: a proxy iterated as an array is empty, so a plugin object passed to `tools.offer` by mistake would silently offer nothing, where userdata raises a type error. Revisit: never.
  - Provider fields on `ModelConfig`, `LocalModelConfig`, the routing `Model`, and `ModelInfo`, with their accessors and copies. Reason: `Capabilities` already reaches all four. Revisit: if the provider must stop appearing among capability fields.
  - `plugins` in the prelude sandbox's `VISIBLE_GLOBALS`. Reason: no prelude needs it. Revisit: when a prelude does.
  - `tools.offer_required()` and `tools.offer_extras()`. Reason: they would need `always_offer` twins, four functions to save one call. Revisit: if authors misuse the composed form.
  - `tools.surplus()`, `tools.extra()`, `tools.optional()`, and `tools.available()` for the extras list. Reason: the user chose `extras` to pair with `required`. Revisit: never.
  - An authored model-facing rename at offer time. Reason: the wire name will be generated per model. Revisit: never.
  - Picking a tool-name style by model id pattern. Reason: the user chose a catalog field. Revisit: never.
  - Keeping the old function names as aliases. Reason: prompts are at language version 0, and two spellings would persist. Revisit: if outside prompts must keep running unchanged.
  - `tools.offer` detecting a function argument to create and offer a local tool. Reason: one name would carry two argument lists, and a handler-first form puts a multi-line function before the tool's name. Revisit: never.
  - `tools.define`, returning a local tool object for a separate `tools.offer`. Reason: a created tool that is not offered has no real use. Revisit: if prompts need to create a tool in the shared library and offer it only in some sections.
  - `tools.offer` and `tools.always_offer` taking only tool objects, so ids resolve only through `tools.get`. Reason: a mistyped id would reach `tools.offer` as nil, and the error could no longer name it. Revisit: never.
  - A `plugin/*` string pattern in `tools.offer`. Reason: `tools.offer(plugins.get(name).tools)` covers it in plain Lua, and the pattern would be a second syntax to document, parse, and test. Revisit: never.
  - Fresh plain records `{ id, plugin, description }` per call. Reason: the same tool would be a different value in each list, so a tool could not sit in several containers as one object. Revisit: never.
- Assumptions, risks, and notes:
  - The models-loop-in-Rust plan has landed in full and closed at `2a8e9aedd` on `master`, so this plan builds on its handle methods and its contract tests. At `2b53936e6`, tests read role labels as globals in 5 places in 3 files, 2 of them method calls, which the role-globals change migrates.
  - Every line citation in this plan was checked at `2b53936e6`. The models-loop debt-removal plan, recorded in `vibe/2026-10-09-4-models-loop-debt-removal.md`, closed there on `master`, and the citations into the files its commits touched were re-checked at that commit.
  - The shipped prompts' tools change names for the model: `search` and `fetch` become `web_search` and `web_fetch`.
  - Until the per-model rename lands, list records hold wire names (`web_fetch`) while Lua uses ids (`web/fetch`).
  - An undeclared plugin with no tools is invisible to `plugins.extras()`, because the Engine learns of plugins only through the catalog and the declared list (`crates/promptforge-internal/engine/src/execute/config.rs`, lines 91 to 108).
  - Existing run logs are disposable, so the effect `alias` of a script call by id, which becomes the wire name, takes effect on the next run.
  - `Capabilities` describes "what a model can do rather than how the gateway reaches it" (`metadata.rs`, lines 78 to 83). The provider describes the model too, and its doc says so.
  - The two `ModelBinding` facade items exist only to expose `handle.provider`. Without that field, the provider change would add two facade items, not four.
  - The `promptforge-docs` guide still documents three-segment tool paths, optional plugins, slots, and alias and role globals, and stays stale after these changes.

### Deferred and Out of Scope

- Deferred: the per-model rewrite of model-facing tool names, using the provider. Revisit once the provider lands and a naming convention per provider is decided.
- Deferred: canonical tool ids in message-list records, rendered per model at the wire. Revisit with the per-model rewrite.
- Deferred: filling the provider automatically when the gateway's admin flow adds a cloud model. Revisit when operators start setting the field by hand.
- Deferred: plugin metadata beyond tool ids, such as an MCP server's reported name and version. Revisit when a prompt needs it.
- Out of scope: handle methods and the namespace forms without a handle, already on `master` from the models-loop-in-Rust plan.
- Out of scope: the rest of the models-loop-in-Rust plan's work, and the models-loop debt-removal plan.
- Out of scope: the `promptforge-docs` guide.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <crate>` for any one crate. Plain `cargo build --locked` builds only the default member, the gateway (`crates/gateway/app`). The desktop app builds with `cargo workshop [--release]` (alias for `build-workshop`, which builds the gateway, stages it as the Tauri sidecar, builds Workshop, and removes the staged sidecar). Crates that bundle a UI through `build-ui` (`gateway-config-ui`, `workshop-server`) need `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first. Toolchain is stable (`rust-toolchain.toml`); the Windows target links with `rust-lld` and a static CRT (`.cargo/config.toml`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`; add `--test <binary>` (`suite` or `it`, see test placement) to target one integration binary. For `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features`. CI's own single-test form is `cargo test --locked -p <crate> [--features ...] --test it <exact_test_name>`. cargo-nextest 0.9.128 is installed locally.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, repeating `-p` per crate (for the Engine family: `-p promptforge -p promptforge-engine -p promptforge-lua -p promptforge-parser -p promptforge-plugin`; workshop app crates without `--all-features`). Structural and boundary checks alone: `cargo test -p build-xtask`. Workshop UI: `npm test --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `build-xtask`'s structural checks. CI additionally runs `cargo check -p gateway --no-default-features` (headless gateway shape), `cargo nextest run --locked -p workshop-server --features headless`, the two UI `npm test` runs above, and `cargo +nightly-2026-09-05 xtask api --check` plus `cargo nextest run --locked -p build-xtask --run-ignored only` on the pinned nightly.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with environment variable `CARGO_BUILD_WARNINGS=deny` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`). Never add a standalone `cargo check --workspace`; clippy covers it. Supply chain (CI and pre-push): `cargo deny check`; CI also runs `cargo audit` and `cargo hakari verify`. UI typecheck: `npm run typecheck --workspaces --if-present` in `crates/workshop`, `npm run typecheck` in `crates/gateway/config-ui/ui`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml`: `style_edition = "2024"`; also the pre-commit hook).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with environment variable `RUSTDOCFLAGS=-D warnings`, plus the facade without features: `cargo doc -p promptforge --no-deps` (same flags). CI also builds `cargo doc -p harness --no-deps` and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`. Facade surface check: `cargo +nightly-2026-09-05 xtask api --check`, which compares against the committed `crates/promptforge/public-api.txt`. The user-guide site (`cargo xtask site`) needs an outside `promptforge-docs` checkout named by `PROMPTFORGE_DOCS` and `mdbook`.
- Test placement and naming conventions:
  - Unit tests sit beside their module in a sibling file `<module>-tests.rs`, wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`; large groups split further (`messages-tests-list.rs`, `messages-tests-view.rs`) or become a `tests/` subdirectory inside `src/` once there are three or more files.
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` (`promptforge`, `harness`) or `tests/it/main.rs` (most others), with one module file per topic and a shared `support` module. Prompt fixtures are `.md` files under `tests/prompts/<category>/` (for example `crates/promptforge-internal/engine/tests/prompts/execution/`). Benches use `harness = false` and `required-features = ["test-support"]`.
  - Test functions are full-sentence snake_case behavior statements (`a_run_can_move_between_threads_between_calls`, `each_getter_returns_the_value_the_context_was_built_from`), usually returning `Result<(), Box<dyn Error>>`. `unwrap`/`expect` are allowed only in tests (`clippy.toml`); suite roots opt in with `#![expect(clippy::expect_used, reason = "...")]`.
  - Test-only helpers live behind per-crate features (`test-support` in the Engine crates and `promptforge-plugin`, `test-fixtures` in gateway and workshop crates); `build-xtask` checks they do not leak.
  - UI tests use `node --test`: `crates/workshop/ui/test/*.mjs` and `crates/gateway/config-ui/ui/src/**/*.test.mjs`, with jsdom.
- Directory map:
  - `crates/` holds every crate (Cargo workspace, resolver 3, edition 2024, version 0.3.0). Public crates sit at the top level; the manifestless containers `promptforge-internal/`, `harness-internal/`, `gateway/` (with nested `gateway/stt/`), and `workshop/` hold each family's private crates.
  - Engine: `crates/promptforge` (facade with `examples/greeter`, `public-api.txt`) and `crates/promptforge-internal/{types,parser,lua,engine,vfs,model-client}`; `crates/promptforge-plugin` is the Plugin API.
  - Harness: `crates/harness` (facade), `crates/harness-internal/runner`, `crates/harness-gateway-client`.
  - Plugins: `crates/plugin-web`, `crates/plugin-mcp`, `crates/plugin-user-input`.
  - Gateway: `crates/gateway/{app,config,config-ui,protocol,routing,local,cloud-providers,logging,progress,web-search,stt/*}` plus the public pair `crates/gateway-api-types` and `crates/gateway-api-discovery`.
  - Workshop: `crates/workshop/{desktop,server-api,server,agents,workspace,user-state,run-log,gateway,menu,status,registry,protocol,support}` in Rust, and the npm workspace `crates/workshop/{ui,look,platform}` in TypeScript. `crates/shared-ui` is a TypeScript and CSS package (not a crate) the gateway config UI uses.
  - Shared and tooling: `crates/shared-error-source`, `crates/shared-loopback`, `crates/workspace-hack` (cargo-hakari), and `crates/build-{ceiling,ui,workshop,xtask,user-guide,llama-cuda}`.
  - Outside `crates/`: `guide/` (mdBook config and chrome for the gateway, language, and workshop books, plus the site landing page), `prompts/` (sample prompt files), `tools/` (Node scripts for sidecar staging and a live TTS check), `vibe/` (dated plan files `YYYY-MM-DD-N-slug.md`, plus `scratch/`), `images/` (README banners), `.github/workflows/` (CI, release, nightly, site), `.githooks/` (pre-commit fmt, pre-push clippy and deny), `.config/` (nextest and hakari config). `local/`, `target/`, and `target-msrv/` are gitignored; `cabinet/` is present but untracked.
- Component boundaries:
  - Engine (`promptforge`, `promptforge-*`) depends on no gateway, workshop, Harness, or Plugin crate. Internal direction: `types` and `vfs` at the bottom; `model-client` -> `types`; `lua` -> `model-client`, `types`, `vfs`; `parser` -> `lua`, `types`; `engine` -> all five; `promptforge` facade re-exports `engine` and the rest. Outside crates reach the Engine only through `promptforge` and `promptforge-plugin` (`promptforge-plugin` -> `types`, `vfs`).
  - Harness: `harness-runner` -> `promptforge`, `promptforge-plugin`; `harness` -> `harness-runner`, `promptforge`, `promptforge-plugin`; `harness-gateway-client` -> `harness`, `promptforge`, `plugin-web` (the one allowed Plugin edge, for web's `SearchProvider`). Outside its family the Harness may name only `promptforge`, `promptforge-plugin`, and `workspace-hack`.
  - Plugins depend only on `promptforge-plugin`, `shared-*`, and outside libraries.
  - Gateway depends on no promptforge, workshop, or Harness crate. `gateway` (app) -> config, config-ui, local, logging, progress, protocol, routing, stt, web-search, the public pair, and `shared-loopback`. `gateway-stt` is the only member of `gateway/stt/` the rest of the family may name.
  - Workshop: the desktop app `workshop` -> `workshop-server-api` -> `workshop-server` only. Inside the server, tiers flow one way: server (`workshop-server`) -> features (`agents`, `workspace`, `user-state`, `run-log`) -> services (`gateway`, `menu`, `status`) -> vocabulary (`protocol`, `registry`, `support`); same-tier crates do not depend on each other except `registry` -> `protocol`. Workshop may use gateway crates only through `gateway-api-types` and `gateway-api-discovery`; `workshop-server` composes the Harness, all three Plugins, and `harness-gateway-client`.
  - `shared-*` depend on no product crate. Every crate's build script runs `build-ceiling`'s check, as a build dependency or, where a `build-*` dependency is banned (Harness and Plugin crates), by `#[path]` include; every crate except `promptforge-vfs` depends on `workspace-hack`. `cargo test -p build-xtask` enforces this matrix, container privacy, the Workshop tiers, and lint inheritance.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized defined terms with one meaning each (`AGENTS.md`); Engine crates say "the caller" and never mention the Host. `crates/workshop/ui/test/docs-claims.mjs` enforces the wording in `AGENTS.md`, crate `## Invariants` docs, and `.cursor/rules`.
  - The Engine is sans-IO: a run emits effects (model reply, tool result, timer, file) and the caller answers them through `Run::step`/`Run::resume`; the Harness performs every effect in production.
  - Each crate's `lib.rs` opens with a `//!` crate doc, often with a `## Invariants` section listing allowed dependencies and tier; workshop `lib.rs` files are facade-only (docs, attributes, `mod`, `pub use`). Several crates carry their own `AGENTS.md` (including `promptforge-internal/{engine,lua,types,vfs}`).
  - Strict workspace lints inherited by every crate: clippy `all` and `pedantic` denied, `unwrap_used`/`expect_used` denied outside tests, `allow_attributes` denied so suppressions use `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` and `unreachable_pub` warned (denied in the gate). Process-global installers are banned outside binary entry points (`clippy.toml`).
  - Source directories are flat: one or two related files are kebab siblings `parent-label.rs` wired with `#[path]`, three or more become a subdirectory; `build-ceiling` fails the build on any Rust file over 500 lines.
  - Comments explain only non-obvious constraints; platform or external-bug workarounds cite the upstream issue URL. Error messages are written for model consumption: concise, naming required versus actual.
  - JSON that reaches a recorder or replay round-trips exactly, with canonical sorted keys and finite numbers (`serde_json` `float_roundtrip`).
  - Behavior changes ship with tests in the same change; refactors keep product and behavior tests. Cargo features gate real constraints only (toolchain or heavy native build), never product shape.
  - Workspace dependency versions live in the root `Cargo.toml` with a comment explaining each pin; members use `workspace = true`.
  - Commits use short imperative sentence-case subjects; finished plans close with a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Stop installing role labels as Lua globals [completed]

- Component: Role globals
- Component placement: first of four. It is small, independent of the tools work, and edits `install_captured_bindings`, whose remainder the Tools by id component deletes, so landing it first shrinks that component.
- Piece: Role-label globals, the component's only piece, built as one step. The parser edit and the install edit land together: turning off the reserved-name check while labels still install as raw globals would let a label such as `tools` overwrite an Engine global.
- Depends on: `master` at `2b53936e6`, where the models-loop debt-removal plan closed and every line citation in this plan was checked. It was still `HEAD` when these steps were written.
- Work:
  - `crates/promptforge-internal/lua/src/vm/install.rs`: delete the role-label half of `install_captured_bindings` (lines 48 to 63) and drop model labels from its doc comment and from the module doc. The tool-alias half stays until step 3.
  - `crates/promptforge-internal/parser/src/contract/models.rs` (lines 127 to 142): set `installs_global` to false for model labels, which also turns off their reserved-name checks. The name grammar still applies.
  - `crates/promptforge-internal/parser/src/contract.rs` (lines 255 to 263): remove the tool-and-model clash check.
  - `crates/promptforge-internal/engine/src/execute/context-bound.rs`, `frontmatter_aliases` (lines 115 to 124): drop the model-label chain, so prelude collision checks see tool aliases only.
  - Move the 5 role-label global reads to `models.get(label)`: `crates/harness/tests/suite/host.rs` lines 43 and 55; `crates/promptforge-internal/engine/src/lua/tests/shims.rs` lines 377, 401, and 402; and the `writer` read in `crates/promptforge-internal/engine/src/execute/tests/live_infer.rs` line 160 (the `echo` tool-alias read on the same line moves in step 3). The other `writer:` and `other:` calls in the engine tests already run on locals and stay.
- Tests:
  - No role-label global exists after setup, in a section VM and in the H1 VM.
  - `models.get(label)` returns the role's handle.
  - A role label that is a reserved name, and one equal to a tool alias, now parse.
  - A plugin prelude global with the same name as a role label installs, because labels left the collision list.
- Verification: `cargo nextest run --locked -p promptforge-lua -p promptforge-parser -p promptforge-engine -p harness --all-features`, then the full verification commands listed in Project Survey. The facade surface check shows no diff.
- Commit: `Stop installing role labels as Lua globals`

</step-1>

<step-2>

### Step 2: Drop tool-slot rows from the Workshop run panel [completed]

- Component: Tools by id
- Component placement: second. It deletes the rest of `install_captured_bindings`, which step 1 edits, and the Plugins table component reads its `ToolSet.declared`, `ToolSet.offered`, and tool objects.
- Piece: Workshop consumer, first of the component's two pieces, built sequentially before the Id-keyed tools piece. Step 3 deletes `Frontmatter::tools`, which the Workshop server reads. Removing that read first lets step 3 compile without touching Workshop, and nothing here needs the new surface. Declared plugins already show through the prompt DTO's `plugins` field.
- Depends on: step 1 by order only. There is no code dependency.
- Work:
  - `crates/workshop/server/src/routes/prompts.rs`: delete the prompt DTO's `tools` field (lines 58 and 59), its fill (lines 230 to 234), and `ToolDto` with its impl (from line 100).
  - `crates/workshop/server/src/routes/prompts-tests.rs`: drop the `tools:` fixture (line 46) and every assertion on the field.
  - `crates/workshop/ui/src/services/run-api.ts`: delete `RunContract.tools` (line 72), the `RunContractTool` type, and its parse (near line 251).
  - `crates/workshop/ui/src/parts/run/run-rows.ts`: delete the tool rows (lines 184 to 188) and drop tools from the header comment (line 7).
  - `crates/workshop/ui/test/run-api.mjs` (lines 43, 104 to 107, 151, 184, and 204) and `crates/workshop/ui/test/run-panel.mjs` (lines 130, 198 and 199, and 451): drop the tool-slot fixtures and expectations.
  - Prerequisite fix: the module doc of `crates/promptforge-internal/engine/src/execute/tests/models_loop-author-shapes.rs` (line 3) names the Workshop, which `crates/workshop/ui/test/docs-claims.mjs` refuses in Engine crates, so the Workshop `npm test` this step runs has failed on `master` since `2e15892a6`. Reword it to name the shipped chat agent without naming a Host application.
- Tests:
  - The run panel renders no `ws-run-panel__row--tool` row.
  - The run API parses a contract without `tools`.
  - The prompts route returns a DTO without `tools`.
- Verification: `cargo nextest run --locked -p workshop-server`, then `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present` in `crates/workshop`. The full verification commands and the facade surface check run at the end of the Tools by id component, in step 3.
- Commit: `Drop tool-slot rows from the Workshop run panel`

</step-2>

<step-3>

### Step 3: Name tools by canonical id and remove tool slots [completed]

- Component: Tools by id
- Piece: Id-keyed tools, second of the component's two pieces, built after the Workshop consumer, as one step. Deleting the slots forces every slot-using test, prompt, and consumer to migrate in the same commit, and moving each call site only once requires the final surface (ids, tool objects, `required` and `extras`, counts by id) in that commit. `LuaToolHandle` is built today only for tool alias globals, so removing those globals without making it the tool object would leave it dead under the warnings-denied clippy gate. Any finer split either moves call sites twice or breaks the gate.
- Depends on: step 1, because this step deletes the remainder of `install_captured_bindings`, and step 2, because Workshop no longer reads `Frontmatter::tools`.
- Work:
  - Tool set, `crates/promptforge-internal/lua/src/handles.rs`:
    - `ToolSet` swaps `bindings` for `declared: Vec<PluginId>`. `offered` keeps its name and type and now holds every offerable catalog tool in id order under its wire name. `always` keeps holding wire names.
    - `ToolSet::for_test` and `ToolSet::from_parts` take `(declared, always, offered)`. Add `declared()`. `offered_binding` (lines 209 to 213) matches a wire name or a canonical id. `ToolSet::bindings` and `ToolSet::binding` go.
    - `ToolView::bindings` becomes `ToolView::declared`, which `RunState::tool_set_snapshot` reads (`crates/promptforge-internal/engine/src/execute/context.rs`, lines 303 to 309). `ToolBinding` changes only in its docs.
  - Offering, `crates/promptforge-internal/engine/src/execute/context-bound.rs`: `bound_tool_set` drops the slot loop and fills `declared`. `offered_bindings` drops the declared-plugin filter (line 50) and the alias set (lines 45 and 46). `offer_refusal` drops its alias rule and becomes `offer_refusal(name, tool, taken)`. `frontmatter_aliases` goes. `catalog_bindings` and `RunState::catalog_binding` stay.
  - Lua `tools` table, `crates/promptforge-internal/lua/src/tools.rs`, `tools/decode.rs`, and `tools/userdata.rs`:
    - `tools.offer` comes from `add` (lines 177 to 214), and `tools.always_offer` from `always` (lines 216 to 244). Both decode through `collect_tools_add_entries(call, args)`, which names the calling function in every argument-shape error and accepts a description override only with a single id or tool object.
    - Each string entry must parse as a tool id before `offered_binding` resolves it, so a wire name, a malformed id, or a local alias raises `{call}: "{id}" is not a catalog tool in this run`. Every entry is checked before any is recorded.
    - `offer` records the binding's wire name in section scope and the section's overrides. `always_offer` sets the prompt-wide override on the binding in `offered` and records its wire name in `always`. Precedence stays: section override, then prompt-wide override, then catalog text. Scope order stays: prompt-wide tools first, then the section's offers in first-offer order, each tool once.
    - Tool objects: `TOOL_OBJECTS_REGISTRY` and `tool_object(lua, id)` hold one `LuaToolHandle` per offered tool, built in `install_tools`. `LuaToolHandle` becomes `{ id: ToolId, description: String }` with `from_binding(&ToolBinding)` and `id()`. Its field getters become `id` and `description` (the catalog text), and it keeps its frozen-assignment refusal. The `name`, `parameters`, `wire_name`, and `untrusted` getters go. `tool_alias` (decode.rs lines 36 to 54) decodes it through `id()`. The record branch and `record_name` (decode.rs lines 18 to 23) go.
    - `tools.required()` and `tools.extras()` come from `install_offered` (lines 252 to 273), split by whether the tool's plugin is in `declared`. Each returns a fresh array of the shared tool objects in id order. `tools.get(id)` reads the registry and returns the object or nil, and raises `tools.get takes a tool id, got {type}` for a non-string.
    - `tools.offer_local` comes from `install_add_local` (lines 275 to 318) without the slot-duplicate check, with `tools.offer_local` in its error texts. A local alias may equal a catalog tool's wire name, and the local tool wins in scope as today (`crates/promptforge-internal/engine/src/execute/scope.rs`, lines 66 to 74).
    - `tools.call` resolves a local alias first, then a parsed catalog id through `offered_binding` and then `RunState::catalog_binding`. Anything else, a wire name included, raises kind `unbound_tool`.
    - `tools.calls`: `install_tool_call_counts` seeds from offered ids in place of slot aliases. Counts key by `binding.id()` at `tools.rs` line 146, `crates/promptforge-internal/engine/src/execute/section_context.rs` line 233, and `crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 190 and 191, while local tools keep their alias. The unseeded-key diagnostic (`tools.rs`, lines 99 to 110) reads exactly `tools.calls: {key:?} has no seeded count; seeded names: {names:?}`, followed by ` (a catalog tool that was neither offered in this section nor called with tools.call)` when the key is the id of a tool the run can offer, by nothing when no name is seeded, and otherwise by ` - check for typos or offer it with tools.offer`.
    - `tools.add`, `tools.always`, `tools.offered`, and `tools.add_local` are no longer installed. `tools.allow_tasks` and the task built-ins are unchanged.
    - Keep `tools.rs` (384 lines at `2b53936e6`) under the 500-line ceiling by placing the tool-object registry and the list functions in a sibling under the existing `tools/` directory if it would pass the ceiling.
  - Engine dispatch and errors: remove `.binding(alias)` from `scheduler/tool_call.rs` (line 179) and from `binding_for_scope` (`crates/promptforge-internal/lua/src/vm/state.rs`, line 237). `unbound_tool_call` (`crates/promptforge-internal/engine/src/execute/scheduler/dispatch.rs`, lines 33 to 42) lists the ids in `ToolSet.offered`, every catalog tool the run can offer, and its error at `crates/promptforge-internal/engine/src/error.rs` lines 253 to 260 keeps kind `unbound_tool` and reads exactly `tool {name:?} is not a tool in this run; catalog tools: {ids:?}`. The out-of-scope check (`scheduler/chat.rs`, lines 269 to 271) asks `offered_binding`, and the bound-slot suffix at `error.rs` line 235 becomes exactly ` (a catalog tool that was not offered in this section)`. Effect and event `alias` fields keep their shape.
  - Deletions:
    - `install_captured_bindings` and its call (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, line 180).
    - `fill_tool_bindings` (`crates/promptforge-internal/engine/src/execute/fill.rs`) and its call (`environment.rs`, line 123), and `ToolBindings` (`bindings.rs`).
    - `RunContext.tool_bindings` and its accessor (`config.rs`, lines 102, 142, 308 to 310, and 366).
    - `Requirements.missing_tools` with its uses in `requirements.rs` (the emptiness check, the merge, and the notice text) and in `requirements-tests.rs`.
    - The parser's `tools:` key, `ToolSlot`, `ToolSlots`, `Frontmatter::tools`, and `ContractKeys.installs_global`.
    - The alias-list parameter of `install_preludes` (`crates/promptforge-internal/lua/src/prelude.rs`).
    - The slot chain in Host requirements (`crates/harness-internal/runner/src/host-run.rs`, lines 128 to 167). Keep the declared-plugin loop, so a missing declared plugin still lands in `missing_required`.
  - Consumers:
    - `prompts/research-person.md` and `crates/workshop/agents/agents/chat.md` drop `tools:` and offer by id with `tools.offer` and `tools.always_offer`.
    - The fixture in `crates/harness-gateway-client/src/wire/request-tests.rs`, and the shim comment at `crates/promptforge-internal/lua/src/__impl_coro.lua` line 71.
    - Docs that describe slots or the old functions: `crates/promptforge-internal/lua/src/lib.rs` lines 36 and 37, `crates/promptforge-internal/lua/src/tools/decode.rs` lines 75 to 78, `crates/promptforge-internal/lua/src/vm/run.rs` line 369, `crates/plugin-web/src/lib.rs` lines 5 to 14, `crates/plugin-user-input/src/lib.rs` lines 59 to 63, and the `tools.always` comments in `context.rs` lines 93 and 345.
  - Facade: regenerate `crates/promptforge/public-api.txt`. The diff is exactly the removals in Technical Design: `ToolSlot`, `ToolSlots` and its methods, `ToolBindings` and its methods, `RunContext::tool_bindings`, `Frontmatter::tools`, `Requirements::missing_tools`, and `ToolSlot::Exact`.
- Tests:
  - Tool objects: `id` and `description`, with the catalog text kept when an override is set; assignment refused.
  - Offering: decoding ids, tool objects, and arrays; all-or-nothing recording; the not-found error for an absent id, a malformed string, a wire name, and a local alias; `offered_binding` resolving a wire name and an id to the same binding; `tools.offer(tools.get(id))` matching `tools.offer(id)`; `always_offer` with an array and an extra, with its prompt-wide override and a section override in today's precedence.
  - Lists: `required` and `extras` membership, with a declared plugin's tools in `required`; identity, so the same tool is `==` across `tools.required()`, `tools.extras()`, and `tools.get`, while each list call returns a new array. This identity test replaces `tools_offered_returns_a_fresh_plain_record_per_offered_tool_in_order` in `crates/promptforge-internal/lua/src/tools/tests-offering.rs`. `tools.get` hit, miss, local alias, malformed id, and non-string argument.
  - Local tools and counts: `tools.offer_local` creating, offering, and answering a local tool as `tools.add_local` did, with its registration errors under the new name; `calls` keyed by id, with a model call and a script call of one tool counting under it, and a local tool under its alias.
  - Errors and removals: the `unbound_tool`, unseeded `tools.calls`, and out-of-scope texts; a leftover `tools:` key refused at parse; `tools.add`, `tools.always`, `tools.offered`, and `tools.add_local` are nil; no tool alias global after setup, so the global table holds only Lua, Engine, and prelude names.
  - Integration: a script calls by id a catalog tool that was left out of the offering, and the call runs; both shipped prompts run end to end.
  - Migration: the roughly 27 files that embed `tools:` frontmatter and the roughly 15 that call `tools.add`; every `tools.add_local`, `tools.always`, and `tools.offered` call; every test that builds a `ToolSet` with slot bindings, which moves them into `offered` under wire names and passes `declared`; every tool alias global read, including `echo.name` in `live_infer.rs` line 160; and the tests that pin the old texts (`tools.rs` lines 103 to 108, 188, 228, and 298; `error.rs` lines 235 and 253).
  - The heaviest migrations: `crates/promptforge-internal/lua/src/tests/tool_scoping.rs`, `lua/src/tools/tests-offering.rs`, and `lua/src/tests/shared_replay.rs`; `crates/promptforge-internal/engine/src/execute/tests.rs`; the engine tests under `crates/promptforge-internal/engine/src/execute/tests/`, namely `debug_and_counts.rs`, `models_loop.rs`, `models_loop_contract-rounds.rs`, `chat_record_rebuild.rs`, `model_and_reply.rs`, `offering.rs`, `local_tools.rs`, `tool_call_arm-local-handlers.rs`, `full_id_calls.rs`, `tool_scoping.rs`, `unified_pipeline.rs`, and `suite/exec_flow/run_setup.rs`; the parser's `contract/tests*.rs`; `crates/promptforge/tests/suite/prepare.rs` (line 371); and the Harness runner tests `prepare.rs`, `prepare-ready.rs`, and `harness-stop.rs` under `crates/harness-internal/runner/tests/it/`.
- Verification: `cargo nextest run --locked -p promptforge -p promptforge-engine -p promptforge-lua -p promptforge-parser -p promptforge-plugin -p harness -p harness-runner -p harness-gateway-client --all-features`, then the full verification commands listed in Project Survey, then `npm test --workspaces --if-present` in `crates/workshop`, which holds the `docs-claims` wording check. The facade surface check matches exactly the removals above.
- Commit: `Name tools by canonical id and remove tool slots`

</step-3>

<step-4>

### Step 4: Add the plugins table [completed]

- Component: Plugins table
- Component placement: third, after Tools by id, because its plugin objects list that component's tool objects and split plugins by `ToolSet.declared`.
- Piece: Plugin objects, the component's only piece, built as one step. The `plugins` global, the tool object's `plugin` getter, and the `plugins` reserved name are checked together by the identity tests, and the reserved-name two-way test fails if the global installs without its `RESERVED_NAMES` entry.
- Depends on: step 3.
- Work:
  - New `crates/promptforge-internal/lua/src/plugins.rs`, a flat sibling with its tests in `plugins-tests.rs` wired by `#[path]`, declared as `mod plugins` in the lua crate's `lib.rs`:
    - `PLUGIN_OBJECTS_REGISTRY`, the registry table keyed by plugin name.
    - `LuaPluginHandle { name: PluginId, tools: Vec<String> }` with field getters `name` and `tools`, where `tools` reads a fresh array of the shared tool objects from `tool_object`, in id order. Its `__newindex` raises `Plugin objects are frozen: cannot assign field {key:?}`.
    - `plugin_object(lua, name)`, read by `plugins.get` and by the tool object's `plugin` getter.
    - `install_plugins(lua, globals, set)`, which builds one object per declared plugin, even one with no tools, and one per other plugin with offered tools, then installs `plugins.required()` (declared plugins, name order), `plugins.extras()` (the others, name order), and `plugins.get(name)` (the object or nil). Each list returns a fresh array.
  - `crates/promptforge-internal/lua/src/vm/install.rs`: call `install_plugins` right after the `install_tools` call (line 135). That path sets up section VMs and the H1 VM alike.
  - `crates/promptforge-internal/lua/src/tools/userdata.rs`: `LuaToolHandle` gains a `plugin` getter that reads `plugin_object` at access time.
  - `crates/promptforge-internal/lua/src/globals.rs` (line 157): `RESERVED_NAMES` becomes `[(&str, Reserved); 61]` with `("plugins", Reserved::EngineGlobal)`. `VISIBLE_GLOBALS` in `prelude.rs` stays unchanged.
- Tests:
  - `plugins.required()` lists a declared plugin with no tools; `plugins.extras()` excludes declared plugins; `plugins.get` hit and miss.
  - `plugins.get(name) == plugins.get(name)`, and the same object appears in `required` or `extras`.
  - A plugin's `tools` holds the shared tool objects in a new array per read and passes to `tools.offer`.
  - Assignment to a plugin object is refused.
  - Every tool object's `plugin` is `==` to its plugin's object.
  - The table exists in the H1 VM.
  - The reserved-name two-way test (`crates/promptforge-internal/engine/src/lua/tests/globals.rs`, lines 86 to 118) passes with `plugins` listed.
  - Integration: a run offers a declared plugin with `tools.offer(plugins.get(name).tools)`, and the model calls one of its tools by wire name. Another run offers an extra the same way after checking `plugins.get`.
- Verification: `cargo nextest run --locked -p promptforge -p promptforge-engine -p promptforge-lua -p promptforge-parser -p promptforge-plugin --all-features`, then the full verification commands listed in Project Survey, then `npm test --workspaces --if-present` in `crates/workshop`, which holds the `docs-claims` wording check. The facade surface check shows no diff.
- Commit: `Add the plugins table over shared plugin objects`

</step-4>

<step-5>

### Step 5: Report an optional model provider in the gateway catalog [completed]

- Component: Model provider
- Component placement: fourth. It depends on no other component and none depends on it, so it follows the dependent chain. It may land earlier, or in parallel with steps 1 to 4, without reordering them.
- Piece: Gateway catalog field, first of the component's two pieces, built sequentially before Provider pass-through. The pieces share no Rust types, because `harness-gateway-client` decodes its own `ModelsListEntry`, so the order is not a compile dependency. Landing the gateway first means the field exists on the wire before any client reads it, and each commit can be checked on its own.
- Depends on: nothing.
- Work:
  - `crates/gateway-api-types/src/metadata.rs`: `Capabilities` (lines 78 to 117) gains `provider: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` and a doc saying the provider describes the model. The struct is already flattened into `[[model]]` and `[[local_model]]` (`crates/gateway/config/src/config.rs`, lines 317 and 387), copied into the routing table (`crates/gateway/app/src/routing.rs`, line 144; `crates/gateway/local/src/runtime/start.rs`, line 245), and flattened into `ModelInfo` on `/v1/models` (`metadata.rs`, line 194), so none of those sites changes.
  - `crates/gateway/config/src/config/validate.rs`, `validate_capabilities` (lines 212 to 259): refuse a provider outside `[a-z0-9._-]+` with `{label} {name} provider must use lowercase letters, digits, '.', '_', or '-'`, for both model kinds.
- Tests:
  - A `[[model]]` and a `[[local_model]]` with and without `provider`, and a refused one, in `crates/gateway/config/src/config/tests/validation/capabilities.rs`.
  - `/v1/models` with and without the field, in `crates/gateway/app/tests/it/chat/catalog.rs`.
- Verification: `cargo nextest run --locked -p gateway-api-types -p gateway-config -p gateway --all-features` and `cargo check -p gateway --no-default-features`. The full verification commands and the facade surface check run at the end of the Model provider component, in step 6.
- Commit: `Report an optional model provider in the gateway catalog`

</step-5>

<step-6>

### Step 6: Expose the model provider on model handles [completed]

- Component: Model provider
- Piece: Provider pass-through, second of the component's two pieces, built after the gateway catalog field.
- Depends on: step 5 for the wire field, not for compilation.
- Work:
  - `crates/harness-gateway-client/src/catalog.rs`: `ModelsListEntry` (lines 20 to 28) gains `#[serde(default)] provider: Option<String>` and passes it to the descriptor through `ModelDescriptor::with_provider` when set.
  - `crates/promptforge-internal/types/src/models.rs`: `ModelDescriptor` (line 147) gains `provider: Option<String>`, `with_provider`, and `provider()`. `ModelDescriptor::new` keeps its signature.
  - `crates/promptforge-internal/model-client/src/model/options.rs`: `ModelBinding` (lines 89 to 99) gains the same field, `with_provider`, and `provider()`. `ModelBinding::new` keeps its signature.
  - `crates/promptforge-internal/engine/src/execute/context-bound.rs`, `bound_model_set` (lines 148 to 183): call `with_provider` when the descriptor names one.
  - `crates/promptforge-internal/lua/src/models-userdata.rs`: `LuaModelHandle` gains a `provider` field getter returning `Option<String>` from `binding().provider()`.
  - `EffectRecord::Chat` (`crates/promptforge-internal/engine/src/execute/run/effect.rs`, lines 172 to 198 and 230 to 256) stays without the provider.
  - Facade: regenerate `crates/promptforge/public-api.txt`. The diff adds exactly `ModelDescriptor::with_provider`, `ModelDescriptor::provider`, `ModelBinding::with_provider`, and `ModelBinding::provider`.
- Tests:
  - Client catalog decode with and without `provider`.
  - `handle.provider` is nil when the catalog names none and the provider id when it does, in a section VM.
- Verification: `cargo nextest run --locked -p harness-gateway-client -p promptforge-types -p promptforge-model-client -p promptforge-engine -p promptforge-lua -p promptforge --all-features`, then the full verification commands listed in Project Survey, then `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`. The facade surface check matches exactly the four additions above.
- Commit: `Expose the model provider on model handles`

</step-6>

</execution-plan>
