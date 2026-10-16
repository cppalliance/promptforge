---
name: Remove Environment and MCP retry
overview: Removes the Engine's Environment type. Its three per-run inputs (tool catalog, Plugin roster, preludes) and its prepare step move onto RunContext, so the Engine takes every run input on one per-run object, and the Harness's HostContext stays the long-lived object the Host builds. A small plugin-mcp change keeps retrying a server that failed to start.
todos:
  - id: remove-env
    content: "D1-1: remove Environment; add the RunContext builders tools, plugins, and preludes and RunContext::prepare; rename the getter RunContext::tools to catalog; Harness composes the context in prepare.rs; migrate every caller and test helper, and rename the tests, helpers, and docs that refer to the removed type; RETIRED_SEEDS gains Environment; bless the public API"
    status: completed
  - id: mcp-retry
    content: "plugin-mcp: after a failed start, retry in the background (1 s doubling to 60 s) until the server answers; runs never wait after the first attempt; switchable fixture flag and tests; crate docs; the Plugin trait's ready docs in runvil-plugin say a failure holds for that run, not forever"
    status: completed
isProject: false
---

# Remove Environment and retry MCP starts

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\runvil` (git). Paths are relative to it.
- Baseline: `7502bf8b4` (`Close plan: plugin-offering`), which is also `origin/master`. Citations were checked there; search by name if a line has moved. "Today" and "as today" in this plan mean the code at this commit.
- Target work analyzed: `2286da760..7502bf8b4`, eight commits:
  - the plugin-offering plan (`vibe/2026-10-13-1-plugin-offering.md`): `a01be9154`, `6a4e414bc`, `1c82e5d51`, `1ec8b5215`, `ba6a741f4`, `7502bf8b4`
  - `fb84079f1` (`Close plan: require-run-debt-removal`, rewritten after a push)
  - `ba4a63beb` (Lua doc links, no `Plan:` trailer)
- Origin: a debt review of that target work. `D1-1` through `D1-24` are its finding IDs, defined in the Debt Inventory below. The review's working files aren't needed to run this plan.
- Shell: PowerShell on Windows.
  - `$env:NAME='value'; <command>` sets the variable for the rest of the session, not only that command. Clear it with `Remove-Item Env:NAME` when a later command must not see it.
  - Run cargo commands one at a time, because they share the target directory's lock. Don't append `2>&1` to them, because PowerShell turns cargo's stderr into error records. Judge a cargo step by its result lines and `$LASTEXITCODE`.
- Before editing, read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. The Harness, `plugin-mcp`, and `runvil-plugin` crates have no `AGENTS.md` of their own.
- Plan record and commits:
  - When execution starts, copy this plan into `vibe/` under the next free `YYYY-MM-DD-N-remove-environment.md` name for that day, and create `vibe/ACTIVE` naming it. Mark each step `[completed]` in that copy as it lands.
  - Each commit message has a one-line summary, a body, `Design:` lines in the format the plugin-offering commits use (`git log -6 --format=%B 7502bf8b4`), and a `Plan: vibe/<file>` trailer.
  - One commit per work item, with the summary its Execution Instructions name. The first commit also adds the `vibe/` copy and `vibe/ACTIVE`, and the close commit removes `vibe/ACTIVE`, as the plugin-offering plan's commits did. On 2026-10-13 the next free name is `vibe/2026-10-13-2-remove-environment.md`.
  - Close with a commit `Close plan: remove-environment` that carries the same trailer. Nothing is pushed.
- Terms:
  - Engine, Harness, Host, and Plugin are as `AGENTS.md` defines them.
  - *`RunContext`*: the Engine's per-run input object (`crates/runvil-internal/engine/src/execute/config.rs`), built with `RunContext::new` and builder methods such as `model` and `vfs`.
  - *prepare*: the step before a run is built. It binds the prompt's model roles to the context's current model and returns a `Requirements` report that the caller merges with its own and checks for a refusal.
  - *stage 2*: the next stage of the late-bound Plugin offering design, which follows the plugin-offering plan. It isn't part of this plan.
  - *`HostContext`*: the Harness's long-lived object. The Host builds it once at startup, installs every Plugin into it permanently, and every run shares it (`crates/harness-internal/runner/src/host.rs` lines 1-17). It isn't the Engine's `Environment`, and this plan doesn't change it.
  - *the catalog*: the tool descriptors a run may offer.
  - *the roster*: each installed Plugin, usable or unavailable with a reason, for one run.
  - *a prelude*: Lua source a Plugin package ships. Every section VM of a run installs the preludes of the Plugins the prompt declares.

<product-contract>

## Product Requirements

- Scope and target work:
  - The one piece of debt the plugin-offering work added (D1-1). The Engine's `Environment` is documented as shared across runs with nothing per-run on it, but it carries three per-run inputs: the catalog, the roster, and the declared Plugins' preludes. The Harness builds a fresh one for every run from its `HostContext` snapshot.
  - User decision, once it was clear that the long-lived object is the Harness's `HostContext` and not the Engine's `Environment`: "I confused HostContext with Engine::Environment. If we can remove `Environment` completely, that would be best."
  - User, on the approach: "reuse what is there, be conservative adding new structs, optimize for reducing complexity and keep everything working".
  - A small `plugin-mcp` improvement. User: "If you can put a little work into the MCP to make it smoother, that's also ok."
- Cleanup goals:
  - The Engine has one per-run input object, `RunContext`, holding every run input beside the seed, model, filesystem, and limits already there.
  - The long-lived, Host-built object stays the Harness's `HostContext`, which already says so truthfully.
  - An MCP server that was down when the app started is picked up once it answers, without restarting the app.
- Non-goals:
  - Any change to what a run sees: its catalog, roster, preludes, model bindings, Lua objects, errors, and the refusal notice stay as they are for the same caller inputs.
  - New structs or traits.
  - Any change to `HostContext` beyond `begin_run`'s return value.
  - Changing which Plugins count as unavailable, or reconnecting an MCP session that ends after a successful start.
  - The exposed CI gap D1-2, and every rejected candidate.
- Success criteria:
  - No `Environment` type exists in the Engine or the facade, and `RETIRED_SEEDS` keeps it from returning.
  - Two runs whose contexts carry different rosters each see their own.
  - An MCP server that rejected its first connection has its tools listed once it accepts, and a run never waits for a server that has already failed once.
  - Every existing test passes, migrated where it named `Environment`, and every exit check in the Testing Plan passes.

## Functional Specification

### Debt Inventory

- Debt added: D1-1 (worsened), from `ba6a741f4`.
  - Evidence:
    - `crates/runvil-internal/engine/src/execute/environment.rs` lines 21-22: "It is safe to share across concurrent runs (it is `Sync`). Everything that can change per run sits on the [`RunContext`]."
    - The same commit added `Environment::plugins` (lines 71-96), whose doc says "Sets the run's roster", and listed it in `crates/runvil/public-api.txt` line 391.
    - The sharing rule is the 2026-09-13 decision in `vibe/2026-09/2026-09-13-1-capabilities-global-naming.md` lines 600 and 634.
  - Relationship to the target work:
    - The roster is per-run: the Harness builds it from each run's services and `ready` results (`crates/harness-internal/runner/src/host-run.rs` lines 111-134).
    - The catalog is per-run for the same reasons (lines 91-109), and the preludes depend on the prompt's declarations (lines 136-150).
    - `begin_run` builds a fresh `Environment` per run (`crates/harness-internal/runner/src/host.rs` lines 199-205), which hid the contradiction in-tree.
  - Impact: an embedder that follows the doc and shares one `Environment` hands every run the catalog, roster, and preludes of the run it was built for.
  - Target state: `Environment` no longer exists. Every run input sits on `RunContext`, and `RunContext::prepare` binds the prompt's model roles.
- Cheap fixes: none.
- Exposed pre-existing debt, reported here and not remediated: D1-2. No gate builds the Lua crate's docs without features or with private items, so test-gated and private-module doc links break unnoticed. `ba4a63beb` fixed three; `6c818272f` fixed the same kinds in the Engine, and `9c96f123e` then gated only `runvil-engine` (`.github/workflows/ci.yml` lines 126-138).
- Rejected candidates, 22 in all:
  - 9 residual-but-acceptable: the plan's intended design, or a cheap local refactor with no consequence shown.
  - 7 weak or speculative: no caller reaches them.
  - 4 false: resolved deferrals, a lock that still has a writer, and reasons that hold no credentials.
  - 2 unrelated and already there before this work: `tools.offer()` with no argument is a pinned no-op, and a wire-name clash drops a tool, deferred to stage 2.

### Behavior

- A caller builds a run's context with `RunContext::new` and builder methods, now including three new ones:
  - `tools(catalog)`: the run's tool catalog, as `Environment::tools` set it before
  - `plugins(usable, unavailable)`: the run's roster, folded into one map exactly as `Environment::plugins` did
  - `preludes(preludes)`: the preludes every section VM of the run installs, as `Environment::preludes` set them before
- `ctx.prepare(&prompt)` replaces `Environment::new().prepare(&prompt, ctx)`. It binds the prompt's model roles to the context's current model and returns the context with the requirements report, exactly as before.
- The caller still checks before `prepare` that each declared Plugin can serve, merges that report into the one `prepare` returns, and checks the refusal before building the run.
- The Harness sets each run's catalog, roster, and declared preludes on that run's context from its per-run `HostContext` snapshot, using the snapshot code it has today, then calls `prepare`.
- `plugin-mcp`:
  - When its first connection attempt fails, it tries again 1 second later, doubling the wait up to 60 seconds, until an attempt succeeds. Then it stays connected as today.
  - Each attempt keeps today's 2-minute window. Only the first attempt holds the Plugin in its starting state, so only the runs that start during that attempt wait for it.
  - While it keeps failing, `ready` answers at once with the latest reason, the Plugin lists no tools, and the Harness treats it as unavailable for that run, as today. A run that starts after it connects offers its tools.
  - The reason texts, and the rule that a reason never holds any part of the URL or a header value, stay as they are.
- The `Plugin` trait's documented contract for `ready` changes to match: an `Err` means the Plugin can't serve now, and the run that sees it treats the Plugin as unavailable. A Plugin that recovers answers `Ok` to a later run's `ready`. Today the docs say an `Err` means the Plugin never will be ready, which the retry makes false for MCP. The Harness already asks every Plugin on every run, so it needs no change.

</product-contract>
<implementation-contract>

## Technical Design

Nearly everything is a move. `RunContext` already has the `tools`, `roster`, and `preludes` fields and the `model_bindings` field that `prepare` fills (`config.rs` lines 87-110). `Environment::prepare` only copied its own three fields into the first three and called `fill_model_bindings` for the last. Moving the three builders and `prepare` onto `RunContext` deletes the copy step and the type. The Harness already computes all three values on its per-run snapshot, so it hands them to the context directly. The one new piece of logic is the MCP retry loop.

- Engine:
  - Delete `crates/runvil-internal/engine/src/execute/environment.rs` (173 lines).
  - `crates/runvil-internal/engine/src/execute.rs`: drop `mod environment;` (line 42) and `pub use environment::Environment;` (line 59), and rewrite the module-map line (line 15) to say `config` holds `RunContext` and its `prepare`.
  - `crates/runvil-internal/engine/src/lib.rs` line 37: drop `Environment` from the re-export list.
  - `crates/runvil-internal/engine/src/execute/config.rs` (367 lines, to about 450):
    - Add the builders `tools`, `plugins`, and `preludes` and the method `prepare` to `impl RunContext`, with the bodies and docs moved from `Environment` (see Declarations).
    - Add the imports `crate::parser::Prompt`, `super::fill::fill_model_bindings`, and `super::requirements::{Requirements, UnavailablePlugin}`.
    - The catalog builder takes the field's name, `tools`, the way `model`, `vfs`, and the other builders do, so every migrated call keeps `.tools(c)` and only moves to the context.
    - The getter `tools(&self) -> &ToolCatalog` (lines 291-300) is renamed `catalog(&self) -> &ToolCatalog`, following the `vfs`/`vfs_handle` and `model`/`current_model` pairs. Its known callers are `crates/runvil/tests/suite/prepare.rs` lines 295 and 314, and the compiler finds any other.
    - Rewrite the type doc (lines 23-29). Update every doc that links `Environment::prepare` to link `RunContext::prepare`: the field docs (lines 87-110), `model` (line 223), `vfs` (line 240), `model_bindings` (line 283), and the renamed getter (line 293), which now says the `tools` builder sets the catalog.
  - Doc links that name `Environment`, each rewritten to the matching `RunContext` item:
    - `execute/bindings.rs` line 10
    - `execute/context.rs` line 128
    - `execute/run.rs` line 100
    - `execute/requirements.rs` lines 11, 23, 60, and 154
    - `test_support/fixture.rs` line 100
  - `crates/runvil-internal/engine/src/test_support.rs`: `run_with_fixture` (lines 131-156) drops its `env: &Environment` parameter and calls `ctx.prepare(prompt)`. Its doc (lines 134-136) says the run offers the context's catalog, set with `RunContext::tools`.
- Facade, `crates/runvil/src/lib.rs`:
  - Drop `pub use runvil_engine::Environment;` (line 13).
  - Lines 8-9 say: before the run starts, the caller puts the tools a run may offer on its [`RunContext`], and [`RunContext::prepare`] binds the prompt's model roles.
  - `crates/runvil/public-api.txt` loses `Environment`, its five functions, and the getter `RunContext::tools(&self)`. It gains the builders `RunContext::tools(self, ToolCatalog)`, `RunContext::plugins`, and `RunContext::preludes`, the method `RunContext::prepare`, and the getter `RunContext::catalog(&self)`. Regenerate it with `cargo +nightly-2026-09-05 xtask api --bless`.
- Guard, `crates/build-xtask/src/engine_guards.rs`: `RETIRED_SEEDS` gains `Environment`, and its length goes from 31 to 32. The scan matches whole identifiers in live Engine source only, meaning the `runvil` facade, `runvil-plugin`, and every crate under `crates/runvil-internal/`, after stripping comments, string literals, and test code (`crates/build-xtask/src/engine_guards.rs` lines 1-10, `crates/build-xtask/src/retired_symbols.rs` lines 1-10). So prose such as the refusal notice's "the environment cannot satisfy this prompt" can't trip it, and no live code in those crates names `Environment` once the type is gone.
- Harness:
  - `crates/harness-internal/runner/src/host.rs`: `begin_run` (lines 174-206) stops building an `Environment` and returns `(HostRunContext, Requirements)`. Drop `Environment` from the import on line 26. Its doc says the snapshot gives the run's catalog, roster, and declared preludes and performs the run's tool calls.
  - `crates/harness-internal/runner/src/host-run.rs`: `catalog`, `roster`, and `preludes` (lines 92, 114, and 138) widen from `pub(super)` to `pub(crate)` so `prepare.rs` can call them. Their bodies don't change.
  - `crates/harness-internal/runner/src/prepare.rs`: lines 328-331 become the snippet under Declarations.
    - The module doc (lines 8-18) says "the ceremony `RunContext::prepare` expects of the Harness", and that the snapshot yields the catalog, the roster, the preludes, and the tool performer, the first three set on the context before it is prepared.
    - The doc of `prepare` (lines 190-199) follows.
- Callers and test helpers. Removing the type makes the compiler list every code use. It can't see comments, docs, or names that only mention the type. Those are listed under "Stale names and prose" below, and the exit check's `rg` catches any left over. The patterns:
  - `Environment::new().prepare(&prompt, ctx)` becomes `ctx.prepare(&prompt)`.
  - `Environment::new().tools(c).preludes(p).prepare(&prompt, ctx)` becomes `ctx.tools(c).preludes(p).prepare(&prompt)`.
  - A read of the catalog, `ctx.tools()`, becomes `ctx.catalog()`.
  - `test_support::run_with_fixture` drops its `env: &Environment` parameter, and its 14 calls follow.
  - In `execute/tests/context.rs`:
    - `env_run` (line 322) drops its `env: &Environment` parameter and is renamed `run_assembled`, after its doc's "the context and fixture `to_context` assembled". Its 8 calls, all in `live_infer.rs`, change anyway, and that file's `let env = Environment::new();` lines go.
    - `run` (line 280) and `run_with_context` (line 336) build `Environment::new()` themselves (lines 287 and 340). `run` puts the catalog on the context with `ctx.tools(catalog)` in place of `env.tools(catalog)` (line 298), and both stop passing an environment to `run_with_fixture`.
    - The stale `Environment::run` doc at line 217 names `run_with_fixture`.
  - `bench_env` in `engine/benches/models_loop.rs` (lines 117-121) returns an empty `Environment` and goes, along with its two uses (lines 133 and 171) and `Environment` in the import on line 26.
  - The `Send + Sync` assertions for `Environment` go: `crates/runvil/tests/suite/auto_traits.rs` line 22 (and `Environment` in its import on line 8) and `execute/tests/context.rs` line 45.
  - The 13 `begin_run` destructurings in `crates/harness-internal/runner/src/host-tests.rs` drop `_env`. The prelude tests (starting at lines 266, 331, and 354, with their `run.preludes` calls at lines 274, 345-346, and 365-366) keep calling `run.preludes(&prompt)`.
- Stale names and prose. Each names the removed type, its old `run`, or its copy step, and the compiler doesn't flag it. Assertions don't change:
  - facade `tests/suite/prepare.rs`: `prepare_copies_the_caller_supplied_catalog_onto_the_context` (line 279) becomes `a_prepared_context_keeps_the_caller_supplied_catalog`, since `prepare` no longer copies anything.
  - facade `tests/suite/greeter.rs`: `shout_environment` (lines 91-92) becomes `shout_catalog` and returns the `ToolCatalog`. In `tests/suite/event.rs`, `shout_offer` (lines 74-88) returns the model and the catalog. The `environment` locals there (lines 125-205), in `tests/suite/effect.rs` (line 72), and in `examples/greeter.rs` (line 93) become `catalog`.
  - facade `tests/suite/plugins.rs`: the module doc (lines 1-3) and the prelude test's name, as the Testing Plan says.
  - Engine `execute/tests/suite/prepare.rs`: `env_run_refuses_an_unsatisfiable_prompt_with_a_model_readable_notice` (line 95) and `env_run_prepares_implicitly_and_runs_a_satisfiable_prompt` (line 134) start with `run_with_fixture_` instead, after the helper they call.
  - Engine `execute/tests/suite/exec_flow/run_setup.rs`: `default_environment_runs_a_plugin_free_prompt` (line 76) becomes `a_default_context_runs_a_plugin_free_prompt`. The module doc (line 1), the comments at lines 18-19 and 77-78, and the `expect` text at line 86 say "a default context".
  - Engine docs and comments that say "the default environment" or name the type: `execute/tests/context.rs` line 40, `execute/tests/suite/support.rs` line 58, `execute/tests/suite/require/failures.rs` line 16, `execute/tests/live_infer.rs` line 31, and the module doc of `execute/tests/preludes.rs` (line 2).
- The files that name `Environment` today:
  - facade: `examples/greeter.rs`, and `tests/suite/` `auto_traits.rs`, `effect.rs`, `event.rs`, `greeter.rs`, `ids.rs`, `model.rs`, `plugins.rs`, `prepare.rs`, `prompt.rs`, `shipped.rs`
  - Engine: `benches/models_loop.rs`, `test_support.rs`, `execute/run/tests.rs`, `execute/run/tests-drops.rs`, `execute/tests.rs`, and `execute/tests/` `context.rs`, `full_id_calls.rs`, `live_infer.rs`, `model_tasks.rs`, `observations.rs`, `offering.rs`, `preludes.rs`, `suite/exec_flow/run_setup.rs`, `suite/prepare.rs`, `suite/require/failures.rs`, `suite/support.rs`
  - Harness: `src/host.rs`, `src/prepare.rs`, and `tests/it/` `effect_loop-broker.rs`, `effect_loop-cap.rs`, `support.rs`
  - others: `crates/harness-gateway-client/src/wire/request-tests.rs`
  - Unrelated uses of the word don't change:
    - the refusal notice "the environment cannot satisfy this prompt" and its "the environment provides none" lines, which a run shows the model, and the error docs that describe them
    - the Harness's own `environment` module (`crates/harness-internal/runner/src/environment.rs`), which the `harness` facade re-exports for `HostSnapshot` and `CurrentModelError`
    - Lua's per-VM and per-prelude environment tables (`lua/src/prelude.rs`, `vm.rs`, `require.rs`, and their tests)
    - `minijinja::Environment` in `crates/gateway/local/src/chat_templates/tests.rs` (gateway crates depend on no Engine crate), `BuildEnvironment` in `crates/build-workshop/src/main.rs`, and the "environment variable" prose in the gateway and `harness-gateway-client/src/transport` docs
- `plugin-mcp`:
  - `crates/plugin-mcp/src/connect.rs` (110 lines): `run` (lines 61-106) wraps its body in a loop, as the Declarations show.
    - The transport is built from clones of `entry.url` and `entry.headers`, which are a `String` and a `HashMap<HeaderName, HeaderValue>` (`entry.rs` lines 20-23). Cloning keeps header values marked sensitive. `RemoteEntry` stays as it is and doesn't derive `Clone`. Only its fields are cloned.
    - Today's comment above the transport (lines 67-69: the headers go through the config, never a caller-built client, and never `auth_header`) moves into the loop with it.
    - A success publishes `Ready` and holds the session as today.
    - A failure publishes `Failed` with today's reason text, sleeps for `retry_delay`, and tries again.
    - The doc of `run` (lines 54-60) says it retries until the first success (see Declarations).
  - `crates/plugin-mcp/src/server.rs`: only the `State` doc (lines 26-27) changes. `ready` already waits only while the state is `Starting`, so after the first attempt every run's `ready` answers at once.
  - `crates/plugin-mcp/src/lib.rs`: the crate doc (lines 26-30) replaces "A server that fails to start is unavailable, with the reason" with: a server that fails to start is unavailable to each run that starts while it is down, with the reason, and the Plugin keeps trying in the background, waiting 1 second after the first failure and doubling up to a minute, so a later run offers its tools once it answers.
  - The retry loop needs no drop check: `Server`'s `Drop` (`server.rs` lines 126-132) aborts the connection task, which ends a loop that is sleeping or mid-attempt.
- `runvil-plugin`, docs only (see Declarations):
  - `crates/runvil-plugin/src/plugin.rs`: the `ready` doc's summary (lines 38-39), its first invariant (lines 49-50), and its `# Errors` section (lines 59-61) stop saying an `Err` means the Plugin never will be ready. The invariant that `ready` answers at once after its first resolution unless the Plugin's state has changed (lines 54-56) already allows a recovery and stays.
  - `crates/runvil-plugin/src/lib.rs`: the crate doc's `ready` bullet (lines 37-40) follows.
  - The Harness docs that say a Plugin whose `ready` fails is unavailable to that run (`host.rs` line 11, `prepare.rs` line 197) are already accurate and don't change.
- Constraints:
  - Every `.rs` file stays under 500 lines.
  - Engine crates name the caller and never the Host. Plugin-facing docs say installed, snapshotted, or declared (`crates/workshop/ui/test/docs-claims.mjs`).
  - `plugin-mcp` adds no dependency: `tokio`'s `time` feature is already on.

### Declarations

`crates/runvil-internal/engine/src/execute/config.rs`, the `RunContext` type doc after:

```rust
/// The inputs for one run of a prompt.
///
/// The caller builds a context with `new` and the builder methods, among
/// them the run's tool catalog ([`tools`](RunContext::tools)), Plugin
/// roster ([`plugins`](RunContext::plugins)), and Plugin preludes
/// ([`preludes`](RunContext::preludes)). Then it calls
/// [`prepare`](RunContext::prepare), which binds the prompt's model roles
/// and reports what the caller must still satisfy. The Engine owns the
/// context for the run. Each context belongs to exactly one run.
///
/// (The rest of today's doc, on the caller's clock and randomness, stays.)
#[non_exhaustive]
pub struct RunContext { /* fields unchanged */ }
```

Added to `impl RunContext`. Each body is the removed `Environment` code, writing the existing field:

```rust
/// Sets the run's tool catalog: the descriptors of every tool of every
/// Plugin the caller can serve this run, declared by the prompt or not.
/// The run offers every tool in it, and the caller performs every call.
/// Read it back with [`catalog`](RunContext::catalog).
#[must_use]
pub fn tools(mut self, tools: ToolCatalog) -> RunContext {
    self.tools = tools;
    self
}

/// Returns the run's tool catalog, set with [`tools`](RunContext::tools).
/// It is empty by default.
#[must_use]
pub fn catalog(&self) -> &ToolCatalog {
    &self.tools
}

/// Sets the run's roster: every installed Plugin that can serve the run,
/// declared by the prompt or not, and every one that can't, with the
/// reason. Each usable Plugin gets an object in the run's Lua, with or
/// without tools. An unavailable one gets none, and offering it raises
/// its reason. A name in both lists counts as unavailable. Without a
/// roster, the run's Lua sees the declared Plugins and the Plugins with a
/// tool in the catalog.
#[must_use]
pub fn plugins(mut self, usable: Vec<PluginId>, unavailable: Vec<UnavailablePlugin>) -> RunContext {
    self.roster = usable
        .into_iter()
        .map(|plugin| (plugin, None))
        .chain(unavailable.into_iter().map(|entry| (entry.plugin, Some(entry.reason))))
        .collect();
    self
}

/// Sets the Plugin preludes every section VM of the run installs: the
/// preludes of the Plugins the prompt declares, in declaration order.
/// The rest of the doc moves from `Environment::preludes`
/// (`environment.rs` lines 99-113): install order, and the
/// `RunErrorKind::Lua` failure before any effect.
#[must_use]
pub fn preludes(mut self, preludes: Vec<Prelude>) -> RunContext {
    self.preludes = preludes;
    self
}

/// Binds `prompt`'s model roles to the context's current model and reports
/// what the caller must still satisfy, returning the context beside the
/// report. The rest of the doc moves from `Environment::prepare`
/// (`environment.rs` lines 120-139), minus its sentence about copying the
/// catalog, the roster, and the preludes.
#[must_use]
pub fn prepare(mut self, prompt: &Prompt) -> (RunContext, Requirements) {
    let mut requirements = Requirements::default();
    self.model_bindings = fill_model_bindings(prompt, self.model.as_ref(), &mut requirements);
    (self, requirements)
}
```

Unchanged and reused: the fields `tools: ToolCatalog`, `roster: BTreeMap<PluginId, Option<String>>`, `preludes: Vec<Prelude>`, and `model_bindings: ModelBindings` on `RunContext`, `fill_model_bindings`, `Requirements`, `UnavailablePlugin`, and `bound_tool_set` (`context-bound.rs`), which reads the context's fields as today.

`crates/runvil-internal/engine/src/test_support.rs`, after:

```rust
pub async fn run_with_fixture(
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
    fixture: RunFixture,
) -> RunResult {
    let (ctx, requirements) = ctx.prepare(prompt);
    if let Some(refusal) = requirements.refusal() {
        return RunResult::Failure(refusal);
    }
    run_prepared(prompt, args, ctx, fixture).await
}
```

`crates/harness-internal/runner/src/host.rs`, after:

```rust
pub(crate) fn begin_run(
    &self,
    services: HostServices,
    failures: BTreeMap<PluginId, String>,
    prompt: &Prompt,
) -> (HostRunContext, Requirements) {
    let run = HostRunContext::snapshot(
        self.installed
            .iter()
            .map(|installed| (&installed.name, installed.package, &installed.plugin)),
        services,
        failures,
    );
    let requirements = run.requirements(prompt);
    (run, requirements)
}
```

`crates/harness-internal/runner/src/host-run.rs`, the visibility change only:

```rust
pub(crate) fn catalog(&self) -> ToolCatalog
pub(crate) fn roster(&self) -> (Vec<PluginId>, Vec<UnavailablePlugin>)
pub(crate) fn preludes(&self, prompt: &Prompt) -> Vec<Prelude>
```

`crates/harness-internal/runner/src/prepare.rs`, lines 328-331 after:

```rust
let failures = host.wait_ready(&cancel).await;
let (snapshot, unmet) = host.begin_run(services, failures, &prompt);
let (usable, unavailable) = snapshot.roster();
let (ctx, mut requirements) = ctx
    .tools(snapshot.catalog())
    .plugins(usable, unavailable)
    .preludes(snapshot.preludes(&prompt))
    .prepare(&prompt);
requirements.merge(unmet);
```

`crates/build-xtask/src/engine_guards.rs`:

```rust
const RETIRED_SEEDS: [&str; 32] = [
    // ...the 31 existing names...
    "Environment",
];
```

`crates/plugin-mcp/src/connect.rs`, after. The `start` future and the success arm keep today's bodies:

```rust
/// The wait before the next attempt after `failures` failed ones: one
/// second, doubling, at most a minute.
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(1 << failures.min(6)).min(Duration::from_mins(1))
}

/// Connects to `entry` and publishes `Ready` or `Failed` on `state`,
/// giving each attempt `deadline`. After a failure it waits
/// [`retry_delay`] and tries again, until an attempt succeeds. Then it
/// keeps the connection open until the task is aborted or the transport
/// ends. Neither end changes the published state.
///
/// (today's paragraph on rmcp's missing timeouts stays)
pub(crate) async fn run(
    plugin: PluginId,
    entry: RemoteEntry,
    deadline: Duration,
    state: watch::Sender<State>,
) {
    let mut failures = 0;
    loop {
        // (today's comment on the headers, lines 67-69)
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(entry.url.clone())
                .custom_headers(entry.headers.clone()),
        );
        let start = async { /* today's handshake and list_all_tools */ };
        let reason = match tokio::time::timeout(deadline, start).await {
            Ok(Ok((service, tools))) => {
                // today's success arm: publish Ready, then hold the session
                return;
            }
            Ok(Err(reason)) => reason,
            Err(_) => format!(
                "the MCP server did not finish starting within {} seconds",
                deadline.as_secs()
            ),
        };
        state.send_replace(State::Failed(reason));
        tokio::time::sleep(retry_delay(failures)).await;
        failures = failures.saturating_add(1);
    }
}
```

`crates/plugin-mcp/src/server.rs`, the `State` doc after:

```rust
/// Where a server is. It leaves `Starting` when the first attempt settles
/// and never returns to it. A `Failed` server moves to `Ready` when a later
/// attempt succeeds, and a `Ready` one stays `Ready`.
```

`crates/runvil-plugin/src/plugin.rs`, the changed parts of the `ready` doc after. Lines 41-45 and the other invariants stay:

```rust
/// Resolves once the Plugin's tool list is complete, or with the
/// reason it can't serve now.
///
/// (lines 41-45 unchanged)
///
/// # Invariants
///
/// - `ready` resolves `Ok` once [`tools`](Plugin::tools) is complete,
///   or `Err` with a reason a model can read when the Plugin can't serve
///   now. A Plugin that recovers answers `Ok` to a later call.
/// - (the bound and cancellation invariants unchanged)
///
/// # Errors
/// Returns a [`ToolError`], whose message is safe to show the model,
/// when the Plugin can't offer its tools now, such as when the backend
/// it connects to refuses it or is down.
```

`crates/runvil-plugin/src/lib.rs`, the `ready` bullet after:

```rust
//! - [`Plugin::ready`] resolves `Ok` once [`Plugin::tools`] is complete,
//!   or `Err` with a reason a model can read when the Plugin can't serve
//!   now. The Harness awaits it before each run's snapshot, and a Plugin
//!   whose `ready` fails is unavailable to that run. A Plugin that
//!   recovers serves the runs that start after it does.
```

`crates/plugin-mcp/tests/it/fixture.rs`: the existing reject flag becomes switchable.

```rust
pub(crate) struct Shared {
    // ...unchanged fields...
    /// Answers every `POST` with `401` while set.
    reject: AtomicBool,
}

impl Fixture {
    /// Stops rejecting, so the next connection attempt succeeds.
    pub(crate) fn accept(&self) {
        self.shared.reject.store(false, Ordering::SeqCst);
    }
}
```

The check at line 154 reads `shared.reject.load(Ordering::SeqCst)`, and `spawn` builds the flag with `AtomicBool::new(reject)`.

</implementation-contract>
<verification-contract>

## Testing Plan

- D1-1:
  - New, in `crates/runvil/tests/suite/plugins.rs`: `two_runs_whose_contexts_carry_different_rosters_each_see_their_own`. Two contexts prepare the same prompt, which returns `tostring(plugins.get('kit') ~= nil)`.
    - The first carries `.plugins(vec![kit], vec![])` and returns `"true"`.
    - The second carries `.plugins(vec![], vec![UnavailablePlugin::new(kit, "down")])` and returns `"false"`.
    - Drive each run with the same `step` loop as the prelude test in that file. The prompt declares no Plugins and calls no tools, so the run finishes without effects.
    - This fails to compile at the baseline, which has no `RunContext::plugins`.
  - Changed by the migration only, with their assertions kept:
    - every caller and helper the Technical Design lists, and every test it renames under "Stale names and prose"
    - `a_prelude_on_the_environment_defines_a_function_the_section_calls` in `crates/runvil/tests/suite/plugins.rs`, renamed `a_prelude_on_the_context_defines_a_function_the_section_calls`, along with the module doc (lines 1-3)
    - the facade's `tests/suite/prepare.rs` and the Engine's `execute/tests/suite/prepare.rs`, which now prepare through `RunContext::prepare` and keep their model-binding and requirements assertions. The facade file's two catalog reads (lines 295 and 314) become `ctx.catalog().tools()`. They now pin that `prepare` leaves the caller's catalog in place.
  - Removed: the `Environment` lines of the `Send + Sync` assertions. `RunContext`'s own lines stay.
  - Regression, passing unchanged:
    - `execute/tests/preludes.rs`, apart from building its context with `.tools` and `.preludes`
    - `lua/src/plugins-tests.rs` and `engine/src/execute/tests/offering-plugins.rs`
    - the Harness's `tests/it/prepare.rs` and `prepare-ready.rs`, including `an_undeclared_plugin_whose_ready_fails_raises_its_reason_when_the_prompt_offers_it` and `an_undeclared_installed_native_with_no_tools_has_an_object_the_prompt_can_offer`, which prove the roster still reaches Lua through the context
    - the Harness prelude tests in `host-tests.rs` (starting at lines 266, 331, and 354)
  - The `nextest` runs include `build-xtask`, whose retired-symbol scan now has `Environment` seeded.
- `plugin-mcp`:
  - New, in `crates/plugin-mcp/tests/it/main.rs`: `a_server_that_rejects_its_first_connection_lists_its_tools_once_it_accepts`.
    - Over `Fixture::rejecting()`: `ready` fails and `tools()` is empty.
    - After `fixture.accept()`, polling for at most 30 seconds, `tools()` lists `remote/echo`, and `call(&plugin, "remote/echo", json!({ "q": "hello" }))` returns its text, as in `a_call_sends_the_original_tool_name_and_returns_untrusted_text` (line 131). Build the Plugin with the file's `install` helper (line 39).
    - At the baseline, `Failed` is final, so `tools()` stays empty and the poll times out.
  - Extended: `a_rejecting_server_fails_ready_with_a_reason_that_holds_no_header_value` (line 230) also asserts that a second `ready` resolves with an error within 1 second, which pins that retries never make a run wait.
  - Changed: `a_server_that_never_answers_fails_within_the_startup_deadline` (`crates/plugin-mcp/src/connect-tests.rs` line 15) ends with `task.abort()` in place of `task.await.expect("the task ends once it has published")` (line 53), since a failing `run` now keeps retrying and would never end. The other two `run` tests (lines 58 and 87) spawn `run` without awaiting it and stay as they are; the test runtime drops the retrying task when each test ends.
  - New, in `connect-tests.rs`: `retry_delay_doubles_from_one_second_to_a_minute`, asserting failures 0 through 7 give 1, 2, 4, 8, 16, 32, 60, and 60 seconds.
  - Passing unchanged: every other `plugin-mcp` test, including `dropping_the_plugin_ends_the_session_...` (line 250).
- Exit checks:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, which includes `build-xtask`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - The Workshop tests and the Workshop clippy need the gateway sidecar staged: `cargo build --locked -p gateway --no-default-features` and `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe` before them, and `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc` after them, always.
  - With `$env:CARGO_BUILD_WARNINGS='deny'`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, and `cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - With `$env:RUSTDOCFLAGS='-D warnings'`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p runvil --no-deps`, `cargo doc -p harness --no-deps`, and `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo +nightly-2026-09-05 xtask api --check`, after the bless
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n -e "\bEnvironment\b" -e "default environment" -e "environment's" -e "shout_environment" -e "env_run" -e "bench_env" -e "prepare_copies" crates/runvil crates/runvil-internal crates/harness crates/harness-internal crates/harness-gateway-client/src/wire` finds nothing
  - `rg -n -e "fails to start is unavailable, with the reason" -e "never will be" -e "will never offer" crates/plugin-mcp crates/runvil-plugin` finds nothing

</verification-contract>
<decision-record>

## Decision Record

The Engine's `Environment` goes. Its three per-run inputs and its `prepare` step move onto `RunContext`, where the matching fields already exist, so the Engine takes every run input on one per-run object. The long-lived object the Host builds and keeps stays the Harness's `HostContext`. The removal changes nothing a run sees, and no structs are added. `plugin-mcp` keeps retrying a failed start. The main risk is the facade change: an embedder that names `Environment` moves its calls to the context.

- User-resolved architecture choices:
  - Remove `Environment`: "I confused HostContext with Engine::Environment. If we can remove `Environment` completely, that would be best." This supersedes the 2026-09-13 decision that split the Engine's inputs into a shareable `Environment` and a per-run `RunContext`. The shareable, Host-built object is `HostContext`, and the Engine needs only the per-run one.
  - The user's model of the long-lived side, which this plan leaves in place for later work:
    - the Host builds one long-lived object and installs every Plugin into it permanently, including packages a planned DLL loader adds
    - a shared VFS base (worktree view, overlays, grants) is reused by every run
    - each run gets its own VFS on top of that base, with its own store and mounts, and may get per-run Plugin configs
  - Lean over complete: "reuse what is there, be conservative adding new structs, optimize for reducing complexity and keep everything working".
  - The MCP work stays small: "If you can put a little work into the MCP to make it smoother, that's also ok."
- Reversible decisions, each open to override:
  - `prepare` becomes a `RunContext` method that consumes and returns the context, matching the builder chain, so a caller writes `ctx.tools(c).plugins(u, x).preludes(p).prepare(&prompt)`.
  - All three per-run inputs move, preludes included, so nothing about a run changes and every prelude test keeps its prompt.
  - The catalog builder is `RunContext::tools`, matching its field and the other builders, and the getter it displaces is renamed `RunContext::catalog`. User: "renaming a public getter is fine".
  - `begin_run` returns the snapshot and the requirements. `prepare.rs` composes the context from the snapshot's existing `catalog`, `roster`, and `preludes`, widened to `pub(crate)`, instead of a new method or a context parameter that would touch all 13 test calls.
  - `RETIRED_SEEDS` gains `Environment`, as earlier retirements seeded their symbols. No written rule requires it. It keeps the name, and the confusion it caused, from coming back.
  - `plugin-mcp` retries only until its first success. That keeps the change inside `run`, with no new state, signal, or struct, and the `Failed` state keeps its name.
  - The `Plugin` trait's `ready` docs say an `Err` means the Plugin can't serve now, not that it never will. The retry makes a failure clear for MCP, and the Harness already asks every run, so only the wording changes. The alternative, keeping the strict contract, would rule out the retry.
  - The backoff runs from 1 second, doubling to a 60-second cap.
  - The test fixture's existing reject flag becomes switchable, instead of a port rebind, which can fail on Windows while old sockets linger.
- Rejected alternatives:
  - Keeping an empty `Environment` that holds only `prepare`. Reason: a type with no data whose name suggests the long-lived object, which is really `HostContext`.
  - Rewriting `Environment`'s docs to say it's per-run. Reason: it keeps two per-run input objects and a copy step in the Engine.
  - Keeping every installed Plugin's prelude on `Environment` and having `prepare` pick the declared ones. Reason: it adds Engine logic, changes what the prelude input means, and needs new `plugins:` declarations in about ten test prompts.
  - A free function `prepare(prompt, ctx)`, or `Prompt::prepare(ctx)`. Reason: a `RunContext` method continues the builder chain every caller already writes, and keeps the per-run input in one place.
  - Making `Run::new` prepare the context itself. Reason: callers must merge their own report and refuse before building the run, which the separate `prepare` step allows.
  - Making a failed `ready` leave a Plugin usable, with a full reconnect design. Reason: more than the minimum. See Deferred.
- Assumptions, risks, and notes:
  - Risk: external embedders that name `Environment` break and move to `RunContext::tools`, `plugins`, `preludes`, and `prepare`, and a caller of the getter `RunContext::tools()` moves to `RunContext::catalog()`. `Environment::plugins` landed in `ba6a741f4` on 2026-10-13, so it has had no time to gain outside callers.
  - Note: a run that starts while an MCP server is still failing treats it as unavailable, as today. The retry only makes later runs recover.
  - Note: a server that never accepts, such as one with a wrong URL or a rejected token, is retried once a minute for as long as the `HostContext` holding it lives. Each attempt is one connection.

### Deferred and Out of Scope

- D1-2, gating the Lua crate's docs in CI with a private-items build and a no-features build. Revisit if the user expands the scope.
- The user's long-lived design, where each item belongs on the Harness side:
  - a shared VFS base on `HostContext`, with the Harness composing each run's VFS from it plus the run's store and mounts. Today `RunRequest::vfs` (`crates/harness-internal/runner/src/harness.rs` line 76) carries each run's whole filesystem, and Workshop passes a fresh `VfsRef::default()` (`crates/workshop/server/src/agents.rs` line 349).
  - per-run Plugin configs on the run request
  - DLL-loaded packages registered in the `PackageRegistry`
  - Revisit each when its design is taken up. The Engine keeps receiving the composed per-run values on `RunContext`.
- Letting a run use a Plugin whose `ready` failed, for example by waiting inside the run for a retry to succeed, reconnecting an MCP session that ends after a successful start, showing a down Plugin's reason to the model, reloading MCP settings, and refreshing a run's tools during the run. Revisit together when the user picks up the transient-failure design.
- The 22 rejected candidates, D1-3 through D1-24. Revisit any of them only on new evidence of a consequence.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the gateway (package `gateway` in `crates/gateway/app`, binary `runvil-gateway`), the workspace's one default member. Any other crate builds with `cargo build --locked -p <package>`, and the desktop app with `cargo build --locked -p workshop`; `cargo workshop` (alias for `build-workshop`) builds the gateway, stages it as the Tauri sidecar, builds Workshop, and removes the staged copy. Crate build scripts bundle the TypeScript UIs, so a fresh checkout first needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` (both `node_modules` trees exist on this machine). Toolchain is `stable` per `rust-toolchain.toml`; Windows links with `rust-lld` and the static CRT per `.cargo/config.toml`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-substring>`; leave out `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file runs with `node --test <path-to-file.mjs>`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, with the same `--all-features` exception for the three Workshop crates. CI also runs `cargo nextest run --locked -p workshop-server --features headless` and `cargo nextest run --locked -p workshop-workspace --all-features` on their own. Workshop UI packages (npm workspaces `ui`, `look`, `platform`): `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` and `npm run typecheck` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs`. The workspace run includes `build-xtask`, whose tests are the boundary and structural checks (`cargo xtask tidy` prints the same report on demand); its nightly-only fixtures run with `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`. `.config/nextest.toml` sets a 60-second slow timeout with termination after three periods, a 250 ms leak timeout, and a `heavy` test group for `gateway-stt` and `gateway-stt-backend-whisper`.
- Linter command: with `CARGO_BUILD_WARNINGS=deny` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`), run `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features` for the headless build shape. Never run a standalone `cargo check --workspace` beside clippy. Supply chain: `cargo deny check`, `cargo audit`, and `cargo hakari verify` (all three installed here). The pre-push hook runs docs-claims, the headless check, workspace clippy, and `cargo deny --log-level error check`.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; `core.hooksPath` is `.githooks`, whose pre-commit hook runs it).
- Docs command: with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` for the two facades with default features, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items` and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. When the `runvil` facade's public surface changes, also run `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed here), which compares against `crates/runvil/public-api.txt`; `--bless` rewrites the listing.
- Test placement and naming conventions:
  - Unit tests live in a sibling file wired from the module as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (183 such files, for example `runvil-plugin/src/context.rs` beside `context-tests.rs`, `harness-internal/runner/src/packages.rs` beside `packages-tests.rs`, `plugin-web/src/web.rs` beside `web-tests.rs`). A module with three or more test files uses a `tests/` subdirectory under `src` instead (for example `runvil-internal/lua/src/tests/`, `runvil-internal/engine/src/execute/tests/`). Older code, mostly `gateway/cloud-providers` and `gateway/stt`, keeps an inline `#[cfg(test)] mod tests { }`, as do a few `plugin-web` files (`url_policy.rs`, `response.rs`, `address.rs`).
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` for the `runvil` and `harness` facades, `tests/it/main.rs` elsewhere (`runvil-plugin`, `plugin-mcp`, `plugin-user-input`, `harness-runner`, `gateway`, `gateway-stt`, `gateway-api-discovery`, `build-ui`, and the Workshop crates `agents`, `menu`, `protocol`, `registry`, `run-log`, `server`, `status`, `support`, `user-state`, `workspace`). Each topic is a `mod` file beside `main.rs`. `plugin-web` and `harness-gateway-client` have unit tests only. Prompt fixtures live under `crates/runvil/tests/prompts/` and `crates/runvil-internal/engine/tests/prompts/`.
  - Test functions are snake_case sentences stating the behavior (for example `construct_fails_with_a_tool_error_when_a_host_wide_service_is_missing`); a `test_` prefix appears on about 21 of roughly 5,460 tests.
  - Test-only hooks sit behind a `test-support` feature (`runvil-plugin`, `runvil-lua`, `runvil-parser`, `runvil-engine`) or `test-fixtures` (gateway crates, `gateway-api-discovery`, and Workshop crates). Only `[dev-dependencies]` may enable `test-support`, which `build-xtask`'s leak guard enforces.
  - Doctests are banned: `build-xtask` fails on any compiled code block in a doc comment, so examples belong in tests or `examples/` (`crates/runvil/examples`, `crates/harness/examples`). Benches use criterion with `harness = false` (`runvil-lua`, `runvil-engine`). JS tests are `*.test.mjs` beside the source or `*.mjs` under `test/`, run by `node --test`; each `tools/*.mjs` script has a sibling `.test.mjs`.
- Directory map:
  - `crates/`: all code. The top level holds the public and shared layer: the Engine facade `runvil` and the Plugin contract `runvil-plugin`; the Harness facade `harness` and `harness-gateway-client`; the Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`; the gateway's client-facing `gateway-api-types` and `gateway-api-discovery`; `shared-error-source`, `shared-loopback`, and `shared-ui` (a TypeScript and CSS package excluded from the workspace, not a crate); tooling `build-xtask` (`cargo xtask tidy`, `api`, `site`, and `new-crate`, which scaffolds `workshop-*` crates), `build-workshop` (`cargo workshop`), `build-ui`, `build-user-guide`, `build-ceiling`, and `build-llama-cuda`; and `workspace-hack` for cargo-hakari.
  - `crates/runvil-internal/`: the private Engine crates `types`, `vfs`, `model-client`, `lua`, `parser`, and `engine`.
  - `crates/harness-internal/runner`: `harness-runner`, the private Harness crate behind the `harness` facade, which re-exports `Harness`, `RunRequest`, `RunReport`, `RunControl`, and the `plugin` (`HostContext`, `InstallError`, `PackageRegistry`, `PluginKind`, `HostServices`, `PluginId`), `record`, and `vfs` modules.
  - `crates/gateway/`: private gateway crates; `app` is the binary, beside `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the `stt/` speech subsystem (`api` is the public `gateway-stt`, plus `engine`, `backend-whisper`, and `whisper-ffi`).
  - `crates/workshop/`: the Workshop Host: `desktop` (Tauri app, package `workshop`), `server` and `server-api`, the tier crates `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, and `workspace`, and the npm workspaces `ui`, `look`, and `platform`. `workshop-server`'s `src/agents.rs` (`with_plugins`) builds the Host's `PackageRegistry` from `plugin_web::PACKAGE`, `plugin_user_input::PACKAGE`, and `plugin_mcp::PACKAGE`, then installs `web`, `user-input`, `runvil/subagents`, and one `plugin-mcp` install per configured MCP server.
  - Repository root: `guide/` (docs site sources `books`, `chrome`, `landing`), `prompts/` (example prompts), `tools/` (Node scripts for the gateway sidecar and live TTS checks), `vibe/` (tracked dated plan and run records, `YYYY-MM-DD-N-slug.md`, older months filed under `2026-09/` and `2026-10/`), `local/` (gitignored machine-local gateway config, profiles, and fixtures), `images/`, and an empty untracked `cabinet/`. Root config: `Cargo.toml` (workspace, lints, and every dependency pin), `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml`, `deny.toml`, `dist-workspace.toml` (cargo-dist), and `gateway.local.example.toml`. `.github/workflows/` holds CI (`ci.yml`, gated by `ci-green`), release, nightly, and site jobs; `.githooks/` holds pre-commit and pre-push; `.config/` holds nextest and hakari config; `.cargo/config.toml` holds the `workshop` and `xtask` aliases; `.cursor/rules/` holds `workshop-architecture.mdc` and `workshop-spa.mdc`. 21 crates carry their own `AGENTS.md` and 23 a `README.md`.
- Component boundaries (enforced by `cargo test -p build-xtask`):
  - Engine internal graph: `types` and `vfs` depend on no Engine crate; `model-client` depends on `types`; `lua` on `types`, `vfs`, and `model-client`; `parser` on `lua` and `types`; `engine` on all five. The `runvil` facade re-exports from all six. `runvil-plugin` depends only on `types` and `vfs`. Engine manifests may not name `tokio`, `tokio-util`, `async-trait`, or `reqwest` outside dev tables (except an optional entry only `test-support` enables). Other families reach the Engine only through `runvil` and `runvil-plugin`.
  - Harness crates depend outside their family only on `runvil`, `runvil-plugin`, and `workspace-hack`, never on gateway, shared, Workshop, or `build-*` crates, and declare no normal `tokio` or `tokio-util` dependency. `harness-runner` is private to `harness`; outsiders use `harness` and `harness-gateway-client`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*` crates, `workspace-hack`, and outside libraries. Engine, gateway, and Harness crates must not depend on a Plugin crate, except `harness-gateway-client`, which depends on `plugin-web` to implement its `SearchProvider`. The Host depends on Plugin crates directly (`workshop-server` on all three, `workshop-agents` on `plugin-user-input`).
  - Gateway crates depend on no Engine, Workshop, or Harness crate; the container is private to the family, `crates/gateway/stt/` is a nested subsystem whose only public member is `gateway-stt`, and Workshop sees the gateway only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop tiers flow one way: the server (`workshop-server`), then features (`agents`, `run-log`, `user-state`, `workspace`), then services (`gateway`, `menu`, `status`), then vocabulary (`protocol`, `registry`, `support`). A crate depends only on lower tiers; in the vocabulary tier `workshop-registry` depends on `workshop-protocol`. The desktop app's only Workshop dependency is `workshop-server-api`, which depends only on `workshop-server`.
  - `shared-*` crates depend on no product crate. Every `workshop-*`, `harness-*`, and `plugin-*` crate root must carry a `//! ## Invariants` section. Only `gateway`, `gateway-whisper-ffi`, `gateway-api-discovery`, and `workshop` may relax `unsafe_code`.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized terms with exactly one meaning each (root `AGENTS.md` Definitions). A Plugin is never a capability, pack, or addon, and its lifecycle words are installed, snapshotted, and declared. Engine crates say "the caller" and never mention the Host. `docs-claims.mjs` enforces this wording in every `AGENTS.md`, `## Invariants` crate doc, and `.cursor/rules` file.
  - Every member inherits strict workspace lints (`[lints] workspace = true`, checked by `build-xtask`): clippy `all` and `pedantic` at deny, `unwrap_used` and `expect_used` denied outside tests, `#[allow]` banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` and `unreachable_pub` warned, broken and private intra-doc links denied. `clippy.toml` bans installing process-global state (panic hook, logger, tracing default, rustls default provider) outside binary entry points.
  - Crate roots open with a `//!` doc. A Plugin crate exports one `pub const PACKAGE: Package` built with `Package::new("<vendor>/<name>", construct)` (`runvil/user-input`, for example), and its crate doc names the default install name and the services it needs.
  - Source directories are flat: a subdirectory needs three or more files, otherwise use `parent-label.rs` siblings wired with `#[path]`, converting in either direction when touching a group on the wrong side of the line. No Rust file under a crate's `src`, `tests`, `benches`, or `examples` may exceed 500 lines: every build script runs `build_ceiling::check()`, and Harness and Plugin crates (plus `runvil-vfs`) include its source through `#[path]` because they may name no `build-*` crate.
  - A removed Engine symbol is added to `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`, and the retired-symbol scan then fails if live Engine source names it again.
  - Dependencies are declared once in `[workspace.dependencies]`, with comments explaining each pin and disabled feature, and members name them with `.workspace = true`. The gateway links exactly one rustls backend (aws-lc-rs); CI fails if `ring` enters its normal closure.
  - Reuse an existing facility before adding machinery (root `AGENTS.md` Principles). Error and status messages are written for model consumption: concise, naming what is missing with required versus actual. Comments explain only non-obvious constraints, and platform workarounds cite the upstream issue URL.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers).
  - Workshop UI CSS uses only `@workshop/look` and `ui/src/tokens/component.css` tokens, and persisted UI state goes through `ui-storage` to `ui-state.json` or the workspace file.
  - Commit subjects are short imperative sentences with no type prefix (for example "Offer a whole Plugin to a section with plugins.offer"); a finished plan is recorded under `vibe/` and closed with a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Two steps, no components (Bounded path). Each step is one commit that leaves its tests passing. Step 1 removes `Environment` (D1-1). Step 2 adds the `plugin-mcp` start retry, then runs the rest of the Testing Plan's exit checks, which cover both steps. Step 2 needs none of Step 1's code, but it runs second because cargo commands share the target directory's lock.

<step-1>

### Step 1: Move every run input onto RunContext and remove Environment [completed]

- Component: `none`
- Tests:
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo nextest run --locked -p runvil-engine -p runvil -p harness-runner -p harness -p harness-gateway-client -p build-xtask --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine -p runvil -p harness-runner -p harness -p harness-gateway-client --all-targets --all-features`. `--all-targets` also builds the Engine's bench (`benches/models_loop.rs`), which no test run builds.
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine -p runvil -p harness-runner`, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo fmt --all --check`
  - `rg -n -e "\bEnvironment\b" -e "default environment" -e "environment's" -e "shout_environment" -e "env_run" -e "bench_env" -e "prepare_copies" crates/runvil crates/runvil-internal crates/harness crates/harness-internal crates/harness-gateway-client/src/wire` finds nothing, so rg exits 1
- Commit: one commit with the summary `Move every run input onto RunContext and remove Environment`, a body, `Design:` lines in the format the plugin-offering commits use (`git log -6 --format=%B 7502bf8b4`), and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: the Technical Design's Engine, Facade, Guard, and Harness lists, its "Callers and test helpers" and "Stale names and prose" lists, and the Declarations for `config.rs`, `test_support.rs`, `host.rs`, `host-run.rs`, `prepare.rs`, and `engine_guards.rs`. The `plugin-mcp` and `runvil-plugin` items are Step 2's.
- Before editing: read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. The Harness crates have no `AGENTS.md` of their own.
- Baseline: `7502bf8b4` (`Close plan: plugin-offering`, also `origin/master`). Line citations were checked there. Search by name if a line has moved.
- Shell: PowerShell on Windows. `$env:NAME='value'; <command>` sets the variable for the rest of the session. Run cargo commands one at a time, since they share the target directory's lock. Don't append `2>&1` to them, because PowerShell turns cargo's stderr into error records. Judge each by its result lines and `$LASTEXITCODE`.
- Terms:
  - `RunContext`: the Engine's per-run input object (`crates/runvil-internal/engine/src/execute/config.rs`), built with `RunContext::new` and builder methods such as `model` and `vfs`.
  - prepare: the step before a run is built. It binds the prompt's model roles to the context's current model and returns a `Requirements` report that the caller merges with its own and checks for a refusal.
  - `HostContext`: the Harness's long-lived object. The Host builds it once at startup and installs every Plugin into it, and every run shares it. It isn't the Engine's `Environment`, and this step changes only `begin_run`'s return value.
  - the catalog: the tool descriptors a run may offer. The roster: each installed Plugin, usable or unavailable with a reason, for one run. A prelude: Lua source a Plugin package ships, which every section VM of a run installs for the Plugins the prompt declares.

Artifacts:

- Removed: `crates/runvil-internal/engine/src/execute/environment.rs`, its `mod` and `pub use` in `execute.rs`, and the `Environment` re-exports in the Engine's `lib.rs` and the facade's `lib.rs`.
- `crates/runvil-internal/engine/src/execute/config.rs`: `impl RunContext` gains the builders `tools`, `plugins`, and `preludes` and the method `prepare`, with the bodies and docs moved from `Environment`. The getter `tools(&self)` becomes `catalog(&self)`. The type doc and every doc that links `Environment::prepare` are rewritten as the Technical Design and Declarations show.
- Doc links that name `Environment`, in `execute/bindings.rs`, `execute/context.rs`, `execute/run.rs`, `execute/requirements.rs`, and `test_support/fixture.rs`, and the module-map line in `execute.rs`.
- `test_support::run_with_fixture` without its `env: &Environment` parameter, and its 14 calls.
- The facade's `crates/runvil/src/lib.rs` lines 8-9, and `crates/runvil/public-api.txt`, regenerated with `cargo +nightly-2026-09-05 xtask api --bless` as part of the change so the commit includes it.
- `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`, from 31 to 32 names with `Environment`.
- Harness: `HostContext::begin_run` returning `(HostRunContext, Requirements)`, `HostRunContext::catalog`, `roster`, and `preludes` widened to `pub(crate)` with unchanged bodies, the context composition in `prepare.rs`, and that file's module doc and `prepare` doc.
- Every helper, bench, and test file the Technical Design lists, including `env_run` renamed `run_assembled`, `bench_env` removed, the 13 `begin_run` destructurings in `host-tests.rs` without `_env`, and the `Environment` lines of the `Send + Sync` assertions removed.

Behavior:

- A caller builds a run's context with `RunContext::new` and builder methods, now including three new ones:
  - `tools(catalog)`: the run's tool catalog, as `Environment::tools` set it before
  - `plugins(usable, unavailable)`: the run's roster, folded into one map exactly as `Environment::plugins` did, where a name in both lists counts as unavailable
  - `preludes(preludes)`: the preludes every section VM of the run installs, as `Environment::preludes` set them before
- `ctx.prepare(&prompt)` replaces `Environment::new().prepare(&prompt, ctx)`. It binds the prompt's model roles to the context's current model and returns the context with the requirements report, exactly as before.
- The caller still checks before `prepare` that each declared Plugin can serve, merges that report into the one `prepare` returns, and checks the refusal before building the run.
- The Harness sets each run's catalog, roster, and declared preludes on that run's context from its per-run `HostContext` snapshot, using the snapshot code it has today, then calls `prepare`.
- Nothing a run sees changes for the same caller inputs: its catalog, roster, preludes, model bindings, Lua objects, errors, and the refusal notice stay as they are.
- No new structs or traits.
- Public API: `Environment`, its five functions, and the getter `RunContext::tools(&self)` leave. The builders `RunContext::tools(self, ToolCatalog)`, `RunContext::plugins`, and `RunContext::preludes`, the method `RunContext::prepare`, and the getter `RunContext::catalog(&self)` arrive. An external embedder that names `Environment` moves to these, and a caller of the old getter moves to `catalog()`.
- Unrelated uses of the word stay: the refusal notice "the environment cannot satisfy this prompt", its "the environment provides none" lines, and the error docs that describe them; the Harness's own `environment` module (`HostSnapshot`, `CurrentModelError`); Lua's per-VM and per-prelude environment tables; `minijinja::Environment`; `BuildEnvironment`; and "environment variable" prose. The retired-symbol scan matches whole identifiers in live Engine source after stripping comments, string literals, and test code, so that prose can't trip it.

Test cases, in this order:

- First, and confirm it fails to compile at the baseline, which has no `RunContext::plugins`: `two_runs_whose_contexts_carry_different_rosters_each_see_their_own` in `crates/runvil/tests/suite/plugins.rs`. Two contexts prepare the same prompt, which returns `tostring(plugins.get('kit') ~= nil)`.
  - The first is built with `.plugins(vec![kit], vec![])` and returns `"true"`.
  - The second is built with `.plugins(vec![], vec![UnavailablePlugin::new(kit, "down")])` and returns `"false"`.
  - Drive each run with the same `step` loop as the prelude test in that file. The prompt declares no Plugins and calls no tools, so the run finishes without effects.
- Then make the change, migrate the callers, and make the renames. Migrated tests keep their assertions:
  - every caller and helper the Technical Design lists, and every test it renames under "Stale names and prose"
  - `a_prelude_on_the_environment_defines_a_function_the_section_calls` in `crates/runvil/tests/suite/plugins.rs` becomes `a_prelude_on_the_context_defines_a_function_the_section_calls`, and the module doc (lines 1-3) follows
  - the facade's `tests/suite/prepare.rs` and the Engine's `execute/tests/suite/prepare.rs` prepare through `RunContext::prepare` and keep their model-binding and requirements assertions. The facade file's two catalog reads (lines 295 and 314) become `ctx.catalog().tools()`, which now pin that `prepare` leaves the caller's catalog in place.
- Removed: the `Environment` lines of the `Send + Sync` assertions in `crates/runvil/tests/suite/auto_traits.rs` and `execute/tests/context.rs`. `RunContext`'s own lines stay.
- Regression, passing unchanged:
  - `execute/tests/preludes.rs`, apart from building its context with `.tools` and `.preludes`
  - `lua/src/plugins-tests.rs` and `engine/src/execute/tests/offering-plugins.rs`
  - the Harness's `tests/it/prepare.rs` and `prepare-ready.rs`, including `an_undeclared_plugin_whose_ready_fails_raises_its_reason_when_the_prompt_offers_it` and `an_undeclared_installed_native_with_no_tools_has_an_object_the_prompt_can_offer`, which prove the roster still reaches Lua through the context
  - the Harness prelude tests in `host-tests.rs` (starting at lines 266, 331, and 354), which keep calling `run.preludes(&prompt)`
- The nextest run includes `build-xtask`, whose retired-symbol scan now has `Environment` seeded.

Decisions:

- `prepare` is a `RunContext` method that consumes and returns the context, continuing the builder chain: `ctx.tools(c).plugins(u, x).preludes(p).prepare(&prompt)`.
- All three per-run inputs move, preludes included, so nothing about a run changes and every prelude test keeps its prompt.
- The catalog builder is `RunContext::tools`, matching its field and the other builders, and the getter it displaces becomes `RunContext::catalog`, following the `vfs`/`vfs_handle` and `model`/`current_model` pairs. User: "renaming a public getter is fine".
- `prepare.rs` composes the context from the snapshot's existing `catalog`, `roster`, and `preludes`, instead of a new method or a context parameter that would touch all 13 `begin_run` test calls.
- `RETIRED_SEEDS` gains `Environment` to keep the name, and the confusion with `HostContext` it caused, from coming back.
- Rejected: an empty `Environment` holding only `prepare`; rewriting `Environment`'s docs to say it's per-run; keeping every installed Plugin's prelude on `Environment` and having `prepare` pick the declared ones; a free function `prepare(prompt, ctx)` or `Prompt::prepare(ctx)`; and making `Run::new` prepare the context itself, since callers must merge their own report and refuse before building the run.

Constraints: every `.rs` file stays under 500 lines (`config.rs` goes from 367 to about 450). Engine crates name the caller and never the Host. Plugin-facing docs say installed, snapshotted, or declared (`crates/workshop/ui/test/docs-claims.mjs`). Reuse what is there and add no structs.

</step-1>

<step-2>

### Step 2: Keep retrying an MCP server that failed to start [completed]

- Component: `none`
- Tests:
  - `cargo nextest run --locked -p plugin-mcp -p runvil-plugin --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p plugin-mcp -p runvil-plugin --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p plugin-mcp -p runvil-plugin`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`, which checks the Plugin wording in `runvil-plugin`
  - `cargo fmt --all --check`
  - `rg -n -e "fails to start is unavailable, with the reason" -e "never will be" -e "will never offer" crates/plugin-mcp crates/runvil-plugin` finds nothing, so rg exits 1
  - Then the rest of the Testing Plan's exit checks, which cover both steps, in this order:
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, which includes `build-xtask`
    - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
    - `$env:CARGO_BUILD_WARNINGS='deny'; cargo check -p gateway --no-default-features`
    - `cargo build --locked -p gateway --no-default-features`
    - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
    - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
    - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
    - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, always, even when an earlier command failed
    - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p runvil --no-deps`, then `cargo doc -p harness --no-deps`, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
    - `cargo +nightly-2026-09-05 xtask api --check`
    - `rg -n -e "\bEnvironment\b" -e "default environment" -e "environment's" -e "shout_environment" -e "env_run" -e "bench_env" -e "prepare_copies" crates/runvil crates/runvil-internal crates/harness crates/harness-internal crates/harness-gateway-client/src/wire` finds nothing, so rg exits 1
- Commit: one commit with the summary `Keep retrying an MCP server that failed to start`, a body, `Design:` lines in the format the plugin-offering commits use (`git log -6 --format=%B 7502bf8b4`), and a `Plan: vibe/<file>` trailer naming the run's copy of this plan.
- Contract items: the Technical Design's `plugin-mcp` and `runvil-plugin` lists, and the Declarations for `connect.rs`, `server.rs`, `runvil-plugin`'s `plugin.rs` and `lib.rs`, and `tests/it/fixture.rs`.
- Before editing: read the root `AGENTS.md`. The `plugin-mcp` and `runvil-plugin` crates have no `AGENTS.md` of their own.
- Baseline: Step 1's commit. Step 1 doesn't touch `plugin-mcp` or `runvil-plugin`, so their line citations, checked at `7502bf8b4`, still hold. Search by name if a line has moved.
- Shell: PowerShell on Windows. `$env:NAME='value'; <command>` sets the variable for the rest of the session. Run cargo commands one at a time, since they share the target directory's lock. Don't append `2>&1` to them, because PowerShell turns cargo's stderr into error records. Judge each by its result lines and `$LASTEXITCODE`.
- Terms: `ready` is the `Plugin` trait method the Harness awaits before each run's snapshot. A Plugin whose `ready` fails is unavailable to that run. `State` is `plugin-mcp`'s published connection state: `Starting`, `Ready`, or `Failed` with a reason. `HostContext` is the Harness's long-lived object that holds every installed Plugin for as long as the Host keeps it.

Artifacts:

- `crates/plugin-mcp/src/connect.rs` (110 lines): a private `retry_delay(failures: u32) -> Duration`, and `run` (lines 61-106) wrapping its body in a loop, as the Declarations show.
  - Each attempt builds its transport from clones of `entry.url` (a `String`) and `entry.headers` (a `HashMap<HeaderName, HeaderValue>`, `entry.rs` lines 20-23). Cloning keeps header values marked sensitive. `RemoteEntry` stays as it is and doesn't derive `Clone`.
  - Today's comment above the transport (lines 67-69: the headers go through the config, never a caller-built client, and never `auth_header`) moves into the loop with it.
  - A success publishes `Ready`, holds the session as today, and returns. A failure publishes `Failed` with today's reason text, sleeps `retry_delay(failures)`, and tries again.
  - The doc of `run` (lines 54-60) says it retries until the first success. Today's paragraph on rmcp's missing timeouts stays.
- `crates/plugin-mcp/src/server.rs`: only the `State` doc (lines 26-27) changes. `ready` already waits only while the state is `Starting`. The loop needs no drop check: `Server`'s `Drop` (lines 126-132) aborts the connection task, which ends a loop that is sleeping or mid-attempt.
- `crates/plugin-mcp/src/lib.rs`: the crate doc (lines 26-30) replaces "A server that fails to start is unavailable, with the reason" with: a server that fails to start is unavailable to each run that starts while it is down, with the reason, and the Plugin keeps trying in the background, waiting 1 second after the first failure and doubling up to a minute, so a later run offers its tools once it answers.
- `crates/runvil-plugin/src/plugin.rs`: the `ready` doc's summary (lines 38-39), its first invariant (lines 49-50), and its `# Errors` section (lines 59-61), as the Declarations show. Lines 41-45 and the invariant that `ready` answers at once after its first resolution unless the Plugin's state has changed (lines 54-56) stay.
- `crates/runvil-plugin/src/lib.rs`: the crate doc's `ready` bullet (lines 37-40).
- Unchanged: the Harness docs that say a Plugin whose `ready` fails is unavailable to that run (`host.rs` line 11, `prepare.rs` line 197), which are already accurate.
- `crates/plugin-mcp/tests/it/fixture.rs`: `Shared.reject` becomes an `AtomicBool`, documented as answering every `POST` with `401` while set. `Fixture::accept` clears it, the check at line 154 reads it with `Ordering::SeqCst`, and `spawn` builds it with `AtomicBool::new(reject)`.
- Tests in `crates/plugin-mcp/tests/it/main.rs` and `crates/plugin-mcp/src/connect-tests.rs`, listed below.

Behavior:

- When its first connection attempt fails, `plugin-mcp` tries again 1 second later, doubling the wait up to 60 seconds, until an attempt succeeds. Then it stays connected as today.
- Each attempt keeps today's 2-minute window. Only the first attempt holds the Plugin in its `Starting` state, so only the runs that start during that attempt wait for it.
- While it keeps failing, `ready` answers at once with the latest reason, the Plugin lists no tools, and the Harness treats it as unavailable for that run, as today. A run that starts after it connects offers its tools.
- The reason texts, including `the MCP server did not finish starting within {} seconds` for a timed-out attempt, and the rule that a reason never holds any part of the URL or a header value, stay as they are.
- The `Plugin` trait's documented `ready` contract: an `Err` means the Plugin can't serve now, and the run that sees it treats the Plugin as unavailable. A Plugin that recovers answers `Ok` to a later run's `ready`. The Harness already asks every Plugin on every run, so it needs no change.
- A server that never accepts, such as one with a wrong URL or a rejected token, is retried once a minute for as long as the `HostContext` holding it lives. Each attempt is one connection.
- Out of scope: changing which Plugins count as unavailable, reconnecting an MCP session that ends after a successful start, having a run wait inside itself for a retry to succeed, showing a down Plugin's reason to the model, reloading MCP settings, and refreshing a run's tools during a run.

Test cases, in this order:

- First the fixture: make the reject flag switchable and add `Fixture::accept`, so the next test can run against today's `connect.rs`.
- Then the recovery test, and confirm it fails before the `connect.rs` change, because `Failed` is final there, so `tools()` stays empty and the poll times out: `a_server_that_rejects_its_first_connection_lists_its_tools_once_it_accepts` in `tests/it/main.rs`.
  - Over `Fixture::rejecting()`: `ready` fails and `tools()` is empty.
  - After `fixture.accept()`, polling for at most 30 seconds, `tools()` lists `remote/echo`, and `call(&plugin, "remote/echo", json!({ "q": "hello" }))` returns its text, as in `a_call_sends_the_original_tool_name_and_returns_untrusted_text` (line 131).
  - Build the Plugin with the file's `install` helper (line 39).
- Then the change, and with it:
  - Extended: `a_rejecting_server_fails_ready_with_a_reason_that_holds_no_header_value` (line 230) also asserts that a second `ready` resolves with an error within 1 second, which pins that retries never make a run wait.
  - Changed: `a_server_that_never_answers_fails_within_the_startup_deadline` (`connect-tests.rs` line 15) ends with `task.abort()` in place of `task.await.expect("the task ends once it has published")` (line 53), since a failing `run` now keeps retrying and never ends. The other two `run` tests (lines 58 and 87) spawn `run` without awaiting it and stay as they are. The test runtime drops the retrying task when each test ends.
  - New, in `connect-tests.rs`: `retry_delay_doubles_from_one_second_to_a_minute`, asserting failures 0 through 7 give 1, 2, 4, 8, 16, 32, 60, and 60 seconds.
- Passing unchanged: every other `plugin-mcp` test, including `dropping_the_plugin_ends_the_session_...` (line 250).

Decisions:

- `plugin-mcp` retries only until its first success. That keeps the change inside `run`, with no new state, signal, or struct, and `Failed` keeps its name.
- The `ready` docs say an `Err` means the Plugin can't serve now, not that it never will. Keeping the strict contract would rule out the retry.
- The backoff runs from 1 second, doubling to a 60-second cap.
- The fixture's existing reject flag becomes switchable, instead of a port rebind, which can fail on Windows while old sockets linger.
- Rejected: making a failed `ready` leave a Plugin usable, with a full reconnect design, as more than the minimum.

Constraints: `plugin-mcp` adds no dependency, since `tokio`'s `time` feature is already on. Every `.rs` file stays under 500 lines. Plugin-facing docs say installed, snapshotted, or declared, which `crates/workshop/ui/test/docs-claims.mjs` enforces.

</step-2>

</execution-plan>
