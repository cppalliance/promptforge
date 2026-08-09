//! Section lifecycle execution and fall-through.
//!
//! The run walks top-level sections in file order, creating one isolated
//! [`crate::lua::SectionVm`] for each. Shared Lua loads before host injection,
//! then ordered section blocks use that same VM. Lua before the first prose is
//! prologue-style; Lua after is epilog-style. Non-final prose is single-shot;
//! final prose runs the full tool loop. A scalar early Lua return ends the
//! section; a scalar late Lua return ends the run.
//!
//! Running off the last section ends the run: the result is `default_return`
//! from the frontmatter, else the last model reply, else a generic completion.
//!
//! One run-scoped [`StoreRef`] is created once by the caller and threaded through
//! every section (both its Lua prologue and, later, the model's file tools), so
//! bulk state persists across the context-clearing transitions even though a
//! section's conversation never does.
//!
//! A run reports itself as it goes: [`RunOptions::observer`] receives a
//! borrowed `(execution, section, detail)` record when the run starts and ends, at each
//! section boundary, model turn, tool call, and harness-mediated store
//! operation. Reporting is a side channel and never
//! a decision, so passing [`crate::observe::NullObserver`] changes nothing but
//! the silence.
//!
//! Bound tool declarations replay into each section VM. Prompt-wide aliases
//! and H2 additions form the effective model-visible scope, which is checked
//! for semantic near-duplicates before concrete tools are advertised under
//! their local aliases and dispatched by stable identity.
//!
//! Lua `execute()` runs a named top-level section as a subroutine (fresh VM,
//! fresh conversation, recursion capped at 8) and returns that section's reply.
//! Lua `jump(target)` transfers control to a named section and clears
//! cross-section reply context.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

use promptforge_tool_picker::ToolPicker;

use crate::bind::{BoundPrompt, RuntimeResolution};
use crate::cancel;
use crate::client::{CompletionResult, GatewayClient, Message, ToolSchema};
use crate::debug::{DebugCapture, DebugEvent};
use crate::dialects::{ToolDialect, ToolDialectRegistry};
use crate::fanout;
use crate::lua::{
    LiveBindingProducer, LuaBlockResult, LuaSectionHandle, ModelInferHook, SectionVm, ToolBindings,
    ToolCallCounts, ToolRuntime, ToolScope, install_lua_tool_calls, resolve_section_target,
    snapshot_tool_scope,
};
use crate::model::{CompletionOptions, ModelBinding, ModelBindings, ModelCatalog};
use crate::observe::{NullObserver, Observer, detail};
use crate::parser::{Block, Prompt, Section};
use crate::store::StoreRef;
use crate::subst;
use crate::tools::{SharedTools, Tool, ToolId, ToolRegistry};
use crate::untrusted;
use crate::{Error, NearDuplicateDiagnostic, Result};
use mlua::Value as LuaValue;

/// The default maximum number of model round trips a single section's
/// tool-call loop will take before giving up, applied when a prompt's
/// frontmatter does not declare its own `max_tool_iterations`.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// Maximum nested `execute()` depth (inclusive of the first call).
const MAX_EXECUTE_DEPTH: usize = 8;

/// The prompt language major this executor implements.
const SUPPORTED_MAJOR: u32 = 1;

/// Cached schemas/dispatch for one tool-bag generation.
struct CachedToolState {
    generation: u64,
    scope: ToolScope,
    schemas: Vec<ToolSchema>,
    dispatch: BTreeMap<String, ToolId>,
}

/// Result of preparing the model-visible tool set for one `model:infer` call.
pub(crate) struct PreparedTools {
    /// Effective bindings in model-advertisement order.
    pub(crate) scope: ToolScope,
    /// Schemas advertised to the model for this infer.
    pub(crate) schemas: Vec<ToolSchema>,
    /// Alias-to-identity dispatch map for this infer.
    pub(crate) dispatch: BTreeMap<String, ToolId>,
    /// Whether schemas/dispatch were served from the generation cache.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "asserted by tool_bag_caches_on_unchanged_generation"
        )
    )]
    pub(crate) reused: bool,
}

/// Effective tool set with a generation-tracked schema/dispatch cache.
///
/// Mutations via `tools.add` bump [`ToolRuntime::generation`]. Each
/// [`Self::prepare`] call rebuilds schemas and dispatch only when that
/// generation no longer matches the cache. Used by `model:infer`; the
/// implicit prose path still builds scope through `prepare_effective_scope`.
pub(crate) struct ToolBag {
    bindings: ToolBindings,
    runtime: Arc<Mutex<ToolRuntime>>,
    cached: Option<CachedToolState>,
}

impl ToolBag {
    /// Creates a bag over frozen bindings and the live H2 addition runtime.
    #[must_use]
    pub(crate) fn new(bindings: ToolBindings, runtime: Arc<Mutex<ToolRuntime>>) -> Self {
        Self {
            bindings,
            runtime,
            cached: None,
        }
    }

    /// Returns frozen prompt-level bindings for diagnostics and `tools.calls`.
    #[must_use]
    pub(crate) fn bindings(&self) -> &ToolBindings {
        &self.bindings
    }

    /// Snapshot-reads the live bag; rebuilds schemas/dispatch on generation mismatch.
    ///
    /// # Errors
    /// Returns tool-scope or registry errors from snapshot/validation/schema build.
    pub(crate) fn prepare(
        &mut self,
        bound: Option<&BoundPrompt>,
        registry: &ToolRegistry<'_>,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<PreparedTools> {
        let generation = {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
            runtime.generation()
        };
        if let Some(cached) = &self.cached
            && cached.generation == generation
        {
            return Ok(PreparedTools {
                scope: cached.scope.clone(),
                schemas: cached.schemas.clone(),
                dispatch: cached.dispatch.clone(),
                reused: true,
            });
        }

        let scope = snapshot_tool_scope(&self.bindings, &self.runtime)?;
        let (schemas, dispatch) = match bound {
            Some(bound) => {
                prepare_effective_scope(bound, &scope, registry, execution, observer, section)?
            }
            None => prepare_scoped_tools(&scope, registry)?,
        };
        self.cached = Some(CachedToolState {
            generation,
            scope: scope.clone(),
            schemas: schemas.clone(),
            dispatch: dispatch.clone(),
        });
        Ok(PreparedTools {
            scope,
            schemas,
            dispatch,
            reused: false,
        })
    }
}

/// Shared context for `model:infer` from Lua.
///
/// Carries the gateway client, tool pool, store, observer, and the live tool bag
/// so each infer call can snapshot-read the current effective set.
pub(crate) struct InferContext {
    client: GatewayClient,
    shared_tools: SharedTools,
    #[allow(dead_code, reason = "reserved for store-backed tools in later steps")]
    store: StoreRef,
    observer: Arc<dyn Observer>,
    execution: String,
    section: String,
    max_tool_iterations: usize,
    turns: Arc<AtomicU32>,
    bound: Option<BoundPrompt>,
    live_bindings: Option<LiveBindingProducer>,
    tool_bag: Mutex<ToolBag>,
    counts_slot: Arc<Mutex<Option<ToolCallCounts>>>,
    /// Live sealed `sys` JSON so infer can publish `reply_finish_reason`.
    sys_live: Arc<Mutex<Option<serde_json::Value>>>,
}

impl InferContext {
    fn prepare_tools(
        &self,
        registry: &ToolRegistry<'_>,
    ) -> mlua::Result<(PreparedTools, Vec<String>)> {
        if let Some(live) = &self.live_bindings {
            let bindings = live
                .bindings()
                .map_err(|error| mlua::Error::external(error.to_string()))?
                .0;
            let scope = ToolScope::from_bindings(
                bindings
                    .always()
                    .iter()
                    .filter_map(|alias| {
                        bindings
                            .bindings()
                            .iter()
                            .find(|binding| binding.alias() == alias)
                            .cloned()
                    })
                    .collect(),
            );
            let (schemas, dispatch) = prepare_scoped_tools(&scope, registry)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            let declared = bindings
                .bindings()
                .iter()
                .map(|binding| binding.alias().to_owned())
                .collect();
            return Ok((
                PreparedTools {
                    scope,
                    schemas,
                    dispatch,
                    reused: false,
                },
                declared,
            ));
        }

        let mut bag = self
            .tool_bag
            .lock()
            .map_err(|_| mlua::Error::external("tool bag mutex was poisoned"))?;
        let prepared = bag
            .prepare(
                self.bound.as_ref(),
                registry,
                &self.execution,
                self.observer.as_ref(),
                &self.section,
            )
            .map_err(|error| mlua::Error::external(error.to_string()))?;
        let declared = bag
            .bindings()
            .bindings()
            .iter()
            .map(|binding| binding.alias().to_owned())
            .collect();
        Ok((prepared, declared))
    }

    /// Snapshot-reads the tool bag, runs the tool loop, sets `reply`, returns text.
    fn infer(
        self: &Arc<Self>,
        lua: &mlua::Lua,
        binding: &ModelBinding,
        prompt: &str,
    ) -> mlua::Result<String> {
        let registry = self.shared_tools.registry();
        let (prepared, declared) = self.prepare_tools(&registry)?;
        let counts = {
            let mut slot = self
                .counts_slot
                .lock()
                .map_err(|_| mlua::Error::external("tool call counts mutex was poisoned"))?;
            if let Some(existing) = slot.as_ref() {
                for tool in prepared.scope.bindings() {
                    existing
                        .ensure(tool.alias())
                        .map_err(|error| mlua::Error::external(error.to_string()))?;
                }
                existing.clone()
            } else {
                let created = ToolCallCounts::new(
                    prepared
                        .scope
                        .bindings()
                        .iter()
                        .map(|b| b.alias().to_owned()),
                );
                *slot = Some(created.clone());
                created
            }
        };
        install_lua_tool_calls(lua, &counts, &declared)
            .map_err(|error| mlua::Error::external(error.to_string()))?;

        let completion_options = binding.completion_options();
        let handle = tokio::runtime::Handle::current();
        let (text, finish_reason) = tokio::task::block_in_place(|| {
            handle.block_on(run_tool_loop(
                &self.client,
                &prepared.schemas,
                &prepared.dispatch,
                &registry,
                prompt.to_owned(),
                self.max_tool_iterations,
                SectionProgress {
                    execution: &self.execution,
                    observer: self.observer.as_ref(),
                    section: &self.section,
                    turns: self.turns.as_ref(),
                    debug: None,
                    completion_options: &completion_options,
                },
                Some(&counts),
                self.bound.as_ref().map(BoundPrompt::alias_to_id),
            ))
        })
        .map_err(|error| mlua::Error::external(error.to_string()))?;

        lua.globals()
            .raw_set("reply", text.as_str())
            .map_err(|error| mlua::Error::external(error.to_string()))?;
        {
            let mut live = self
                .sys_live
                .lock()
                .map_err(|_| mlua::Error::external("sys live slot was poisoned"))?;
            if let Some(sys) = live.as_mut() {
                *sys = crate::lua::enrich_sys_reply_finish_reason(sys, finish_reason.as_deref());
                let table = crate::lua::seal_sys(lua, sys)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                lua.globals()
                    .raw_set("sys", table)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
            }
        }
        Ok(text)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "infer hook installation threads the same borrowed run context fanout already carries"
)]
fn attach_infer_hook(
    vm: &SectionVm,
    client: &GatewayClient,
    shared_tools: &SharedTools,
    store: &StoreRef,
    execution: &str,
    section: &str,
    max_tool_iterations: usize,
    turns: &Arc<AtomicU32>,
    bound: Option<&BoundPrompt>,
    live_bindings: Option<LiveBindingProducer>,
) {
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let ctx = Arc::new(InferContext {
        client: client.clone(),
        shared_tools: shared_tools.clone(),
        store: store.clone(),
        // Section lifecycle still uses the borrowed observer; infer reports
        // through a shared handle. Null keeps the seam Send+'static without
        // changing RunOptions.
        observer: Arc::new(NullObserver),
        execution: execution.to_owned(),
        section: section.to_owned(),
        max_tool_iterations,
        turns: Arc::clone(turns),
        bound: bound.cloned(),
        live_bindings,
        tool_bag: Mutex::new(ToolBag::new(tool_bindings, tool_runtime)),
        counts_slot: vm.counts_slot(),
        sys_live: vm.sys_live_handle(),
    });
    let hook: ModelInferHook =
        Arc::new(move |lua, binding, prompt| ctx.infer(lua, binding, prompt));
    vm.set_infer_hook(hook);
}

/// Everything a run needs beyond the prompt, its input, its tools, and its
/// store: where progress is reported, and which gateway it talks to.
///
/// # Examples
/// ```
/// use promptforge_core::execute::RunOptions;
/// use promptforge_core::observe::NullObserver;
///
/// let opts = RunOptions {
///     execution: "example-run",
///     observer: &NullObserver,
///     client: None,
///     debug: None,
/// };
/// assert!(opts.client.is_none());
/// assert!(opts.debug.is_none());
/// ```
pub struct RunOptions<'a> {
    /// The caller-provided identifier shared by every report in this execution.
    pub execution: &'a str,
    /// Where the run reports its progress. Pass
    /// [`NullObserver`](crate::observe::NullObserver) to discard it.
    pub observer: &'a dyn Observer,
    /// The gateway client the run's model calls go through. `None` builds one
    /// from the process environment on the first call that needs it, which is
    /// what the CLI uses; a caller configured from a file (rather than from the
    /// environment) passes its own.
    pub client: Option<GatewayClient>,
    /// Opt-in raw request/response capture for model turns. `None` (the
    /// production default) skips the seam entirely so hosts pay nothing.
    pub debug: Option<&'a dyn DebugCapture>,
}

/// Live capability inputs for the parse-to-run execution path.
#[derive(Debug, Clone, Copy)]
pub struct ResolutionContext<'a> {
    /// Semantic picker used by executed H1 capability calls.
    pub picker: &'a ToolPicker,
    /// Live model catalog used by executed H1 model calls.
    pub models: &'a ModelCatalog,
}

/// A prompt accepted by [`run`].
///
/// A [`BoundPrompt`] supplies frozen H1 declaration replay data. A parsed
/// [`Prompt`] remains accepted as a temporary input for tool-free hosts that
/// have not yet adopted the separate binding phase; it executes on the same
/// validated path as a bound prompt with empty frozen bindings, so any
/// `tools.need` or `tools.add` in it fails loudly, and its shared Lua runs
/// under full replay rules, where a scalar return is an error.
#[derive(Debug, Clone, Copy)]
pub struct RunPrompt<'a> {
    prompt: &'a Prompt,
    bound: Option<&'a BoundPrompt>,
}

impl<'a> From<&'a BoundPrompt> for RunPrompt<'a> {
    fn from(bound: &'a BoundPrompt) -> Self {
        Self {
            prompt: bound.prompt(),
            bound: Some(bound),
        }
    }
}

impl<'a> From<&'a Prompt> for RunPrompt<'a> {
    fn from(prompt: &'a Prompt) -> Self {
        Self {
            prompt,
            bound: None,
        }
    }
}

impl fmt::Debug for RunOptions<'_> {
    /// Formats the options without the observer or debug capture, which are
    /// trait objects and carry no `Debug`; their presence is reported instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunOptions")
            .field("execution", &self.execution)
            .field("observer", &"<dyn Observer>")
            .field("client", &self.client)
            .field(
                "debug",
                &if self.debug.is_some() {
                    "<dyn DebugCapture>"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

/// Execute a prompt and return the run's result.
///
/// `args` is the single raw input string, exposed to Lua and to `{{ args }}`.
///
/// `prompt` normally receives a [`BoundPrompt`], whose frozen H1 declarations
/// replay in each section VM. Parsed [`Prompt`] values remain accepted so
/// existing tool-free hosts keep compiling until they adopt the separate
/// binding phase; they run through the same validated VM with empty frozen
/// bindings, so declaring or scoping any tool fails loudly and shared Lua
/// replays under the same rules as a bound prompt.
///
/// `tools` is the complete callable pool for this run. A bound section exposes
/// only aliases in its effective `tools.always` plus `tools.add` scope, and
/// dispatches a returned alias through its frozen [`crate::tools::ToolId`].
///
/// `store` is the run's virtual-file handle. Create it once (typically with
/// [`StoreRef::memory`]) and pass it in; the same handle is given to every
/// section's Lua prologue, so files persist across sections even though each
/// section's context is cleared on entry. It is a shared handle, so passing
/// `&store` (not a fresh store per section) is what makes the state durable.
///
/// `opts` carries the run's [`Observer`] and, optionally, the
/// [`GatewayClient`] to use. A run that clears the version gate reports
/// [`detail::RUN_STARTED`] before its first section and either
/// [`detail::RUN_SUCCEEDED`] or [`detail::RUN_FAILED`] however it ends. A run
/// refused by the gate reports nothing because it never started.
///
/// # Errors
/// Returns [`crate::Error::UnsupportedVersion`] if the prompt declares a
/// `promptforge:` major this build does not support,
/// [`crate::Error::Parse`] if the file has no `promptforge:` version (it is not
/// a promptforge prompt), [`crate::Error::Lua`] if a Lua prologue fails,
/// [`crate::Error::Substitution`] if a `{{ }}` path cannot be resolved,
/// [`crate::Error::MissingEnv`] if the gateway client cannot be built when a
/// model call is needed, [`crate::Error::UnknownScopedTool`] if a section
/// scopes a tool name absent from `tools`, [`crate::Error::UnknownTool`] if the
/// model calls an alias absent from its effective scope,
/// [`crate::Error::NearDuplicateTools`] if two effective tools meet the picker's
/// duplicate threshold, [`crate::Error::ToolLoopExhausted`] if a section's
/// tool-call loop does not converge within its iteration cap, or any
/// transport/backend error from a model call.
pub async fn run<'a>(
    prompt: impl Into<RunPrompt<'a>>,
    args: &str,
    tools: &[Arc<dyn Tool>],
    store: &StoreRef,
    opts: RunOptions<'_>,
) -> Result<String> {
    let run_prompt = prompt.into();
    let prompt = run_prompt.prompt;
    // Gate on the declared engine major before doing any work: promptforge runs
    // only its own prompts, and refuses an unsupported major rather than
    // silently degrading. A file with no `promptforge:` version is not a
    // promptforge prompt at all, which is the caller's concern, not ours.
    match prompt.frontmatter.promptforge {
        Some(SUPPORTED_MAJOR) => {}
        Some(other) => return Err(Error::UnsupportedVersion(other)),
        None => {
            return Err(Error::Parse(
                "not a promptforge prompt: no promptforge version".into(),
            ));
        }
    }

    let RunOptions {
        execution,
        observer,
        client,
        debug,
    } = opts;
    let shared_tools = SharedTools::new(tools);
    let registry = shared_tools.registry();
    let prompt_section = prompt.title.as_str();
    observer.observe(execution, prompt_section, detail::RUN_STARTED);

    // The turn count is threaded through the whole run so `RunFinished` can
    // report the total even when a section fails part way through it.
    let turns = Arc::new(AtomicU32::new(0));
    let result = run_sections(
        prompt,
        run_prompt.bound,
        run_prompt.bound.map(BoundPrompt::bindings),
        run_prompt.bound.map(BoundPrompt::models),
        None,
        false,
        args,
        &registry,
        &shared_tools,
        store,
        execution,
        observer,
        client,
        debug,
        Arc::clone(&turns),
    )
    .await;

    observer.observe(
        execution,
        prompt_section,
        if result.is_ok() {
            detail::RUN_SUCCEEDED
        } else {
            detail::RUN_FAILED
        },
    );
    result
}

/// Execute a parsed prompt through the single-pass live H1 path.
///
/// H1 Lua and prose blocks run once in source order with full host access.
/// Capability calls resolve when executed, and the resulting bindings plus the
/// final H1 `var` snapshot are captured for the section walk.
///
/// # Errors
/// Returns the same lifecycle errors as [`run`], plus live tool or model
/// resolution failures raised by executed H1 code.
pub async fn run_prompt(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
    tools: &[Arc<dyn Tool>],
    store: &StoreRef,
    opts: RunOptions<'_>,
) -> Result<String> {
    match prompt.frontmatter.promptforge {
        Some(SUPPORTED_MAJOR) => {}
        Some(other) => return Err(Error::UnsupportedVersion(other)),
        None => {
            return Err(Error::Parse(
                "not a promptforge prompt: no promptforge version".into(),
            ));
        }
    }

    let RunOptions {
        execution,
        observer,
        client,
        debug,
    } = opts;
    let shared_tools = SharedTools::new(tools);
    let registry = shared_tools.registry();
    observer.observe(execution, &prompt.title, detail::RUN_STARTED);
    let turns = Arc::new(AtomicU32::new(0));

    let result = async {
        let h1 = execute_live_h1(
            prompt,
            args,
            resolution,
            &registry,
            &shared_tools,
            store,
            execution,
            observer,
            client.as_ref(),
            debug,
            Arc::clone(&turns),
        )
        .await?;
        if let Some(value) = h1.returned {
            return Ok(value);
        }
        run_sections(
            prompt,
            None,
            Some(&h1.bindings),
            Some(&h1.models),
            Some(&h1.var),
            true,
            args,
            &registry,
            &shared_tools,
            store,
            execution,
            observer,
            client,
            debug,
            Arc::clone(&turns),
        )
        .await
    }
    .await;

    observer.observe(
        execution,
        &prompt.title,
        if result.is_ok() {
            detail::RUN_SUCCEEDED
        } else {
            detail::RUN_FAILED
        },
    );
    result
}

struct LiveH1State {
    bindings: ToolBindings,
    models: ModelBindings,
    var: serde_json::Value,
    returned: Option<String>,
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "H1 mirrors the ordered section block walk over explicit run context"
)]
async fn execute_live_h1(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
    registry: &ToolRegistry<'_>,
    shared_tools: &SharedTools,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    client: Option<&GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    turns: Arc<AtomicU32>,
) -> Result<LiveH1State> {
    let runtime = RuntimeResolution::new(resolution.picker, registry, resolution.models)?;
    let sys = json!({
        "when": now_rfc3339(),
        "now": now_rfc3339(),
        "id": 0,
        "section_name": prompt.title,
        "execution": execution,
        "section_count": prompt.sections.len(),
    });
    let mut vm = SectionVm::new(None, execution, observer, &prompt.title)?;
    vm.inject_host(args, &sys, store, None)?;
    macro_rules! h1_try {
        ($expression:expr) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    vm.teardown(observer, &prompt.title);
                    return Err(error);
                }
            }
        };
    }
    let mut active_client = client.cloned();
    if active_client.is_none() {
        active_client = GatewayClient::from_env().ok();
    }
    if let Some(infer_client) = active_client.as_ref() {
        attach_infer_hook(
            &vm,
            infer_client,
            shared_tools,
            store,
            execution,
            &prompt.title,
            prompt
                .frontmatter
                .max_tool_iterations
                .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
            &turns,
            None,
            Some(runtime.producer()),
        );
    }

    let mut conversation = Vec::new();
    let mut reply: Option<String> = None;
    let mut returned = None;
    for block in &prompt.h1_blocks {
        match block {
            Block::Lua(program) => {
                if let Some(value) =
                    h1_try!(vm.run_live_h1_block(program, &runtime, observer, &prompt.title))
                {
                    returned = Some(value);
                    break;
                }
            }
            Block::Prose { text, loop_capable } => {
                let (tool_bindings, model_bindings) = h1_try!(runtime.bindings());
                let Some(alias) = model_bindings.always() else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::ModelRequired {
                        section: prompt.title.clone(),
                    });
                };
                let Some(model) = model_bindings.binding(alias) else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::ModelRequired {
                        section: prompt.title.clone(),
                    });
                };
                let mut scope = Vec::new();
                for alias in tool_bindings.always() {
                    if let Some(binding) = tool_bindings
                        .bindings()
                        .iter()
                        .find(|binding| binding.alias() == alias)
                    {
                        scope.push(binding.clone());
                    }
                }
                let scope = ToolScope::from_bindings(scope);
                let (schemas, dispatch) = h1_try!(prepare_scoped_tools(&scope, registry));
                let var = h1_try!(vm.var());
                let prose = h1_try!(subst::substitute(
                    text,
                    args,
                    reply.as_deref(),
                    None,
                    &var,
                    &sys
                ));
                if prose.trim().is_empty() {
                    continue;
                }
                if active_client.is_none() {
                    active_client = Some(h1_try!(GatewayClient::from_env()));
                }
                let Some(active_client) = active_client.as_ref() else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::Lua(
                        "gateway client was not initialized for H1 prose".to_owned(),
                    ));
                };
                let counts = ToolCallCounts::new(
                    scope
                        .bindings()
                        .iter()
                        .map(|binding| binding.alias().to_owned()),
                );
                let mode = if *loop_capable {
                    ProseMode::Loop {
                        max_tool_iterations: prompt
                            .frontmatter
                            .max_tool_iterations
                            .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
                    }
                } else {
                    ProseMode::SingleShot
                };
                let completion_options = model.completion_options();
                let outcome = h1_try!(
                    run_prose_inference(
                        active_client,
                        &schemas,
                        &dispatch,
                        registry,
                        &mut conversation,
                        prose,
                        mode,
                        SectionProgress {
                            execution,
                            observer,
                            section: &prompt.title,
                            turns: turns.as_ref(),
                            debug,
                            completion_options: &completion_options,
                        },
                        Some(&counts),
                        None,
                    )
                    .await
                );
                if let Some(text) = outcome.text {
                    h1_try!(vm.set_global_string("reply", &text));
                    reply = Some(text);
                }
            }
        }
    }
    let var = h1_try!(vm.var());
    let (bindings, models) = h1_try!(runtime.bindings());
    vm.teardown(observer, &prompt.title);
    Ok(LiveH1State {
        bindings,
        models,
        var,
        returned,
    })
}

/// Walk the prompt's top-level sections, reporting each boundary, and return
/// the run's result.
///
/// Split out of [`run`] so that every way the walk can end - a Lua return, an
/// error, running off the last section - passes through one place that emits
/// the run's final observation.
///
/// # Errors
/// Returns the same errors as [`run`], which documents them.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the lifecycle keeps its borrowed run inputs explicit and linear so every early failure can tear down its owned section VM before returning"
)]
async fn run_sections(
    prompt: &Prompt,
    bound: Option<&BoundPrompt>,
    live_bindings: Option<&ToolBindings>,
    live_models: Option<&ModelBindings>,
    initial_var: Option<&serde_json::Value>,
    live_path: bool,
    args: &str,
    registry: &ToolRegistry<'_>,
    shared_tools: &SharedTools,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    mut client: Option<GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    turns: Arc<AtomicU32>,
) -> Result<String> {
    let when = now_rfc3339();
    let mut last_reply: Option<String> = None;

    // A parsed prompt without a binding pass runs with empty frozen bindings,
    // on the same validated VM path as a bound prompt: there is exactly one
    // `tools.add`, and it rejects every undeclared alias.
    let empty_bindings = ToolBindings::default();
    let empty_models = ModelBindings::default();
    let bindings =
        live_bindings.unwrap_or_else(|| bound.map_or(&empty_bindings, BoundPrompt::bindings));
    let models = live_models.unwrap_or_else(|| bound.map_or(&empty_models, BoundPrompt::models));

    // Resolve the tool-loop cap once: the prompt's declared budget, or the
    // runtime default when it declares none.
    let max_tool_iterations = prompt
        .frontmatter
        .max_tool_iterations
        .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS);

    let task_handles = section_handles(&prompt.sections);
    let mut index = 0usize;
    while index < prompt.sections.len() {
        let section = &prompt.sections[index];
        let sys = json!({
            "when": when,
            "now": now_rfc3339(),
            "id": index + 1,
            "section_name": section.name,
            "execution": execution,
            "section_count": prompt.sections.len(),
        });

        // `completed` counts sections entered, so the first is 1. It only ever
        // grows, which is what the progress contract requires.
        observer.observe(execution, &section.name, detail::SECTION_STARTED);

        let mut vm = if live_path {
            SectionVm::new_for_section(
                prompt.replay.as_ref(),
                bindings,
                models,
                execution,
                observer,
                &section.name,
            )?
        } else {
            match prompt.replay.as_ref() {
                Some(shared) => SectionVm::new_with_shared_bindings(
                    shared,
                    bindings,
                    models,
                    execution,
                    observer,
                    &section.name,
                )?,
                None => SectionVm::new(None, execution, observer, &section.name)?,
            }
        };
        if let Err(error) =
            vm.inject_host_with_var(args, &sys, store, last_reply.as_deref(), initial_var)
        {
            vm.teardown(observer, &section.name);
            return Err(error);
        }

        // Ensure a gateway client exists before Lua may call model:infer.
        // Offline Lua-only prompts declare no models and skip this.
        if client.is_none()
            && !models.bindings().is_empty()
            && let Ok(new_client) = GatewayClient::from_env()
        {
            client = Some(new_client);
        }
        if let Some(infer_client) = client.as_ref() {
            attach_infer_hook(
                &vm,
                infer_client,
                shared_tools,
                store,
                execution,
                &section.name,
                max_tool_iterations,
                &turns,
                bound,
                None,
            );
        }

        let has_children = !section.children.is_empty();
        let mut conversation: Vec<Message> = Vec::new();
        let mut scopes_ready = false;
        let mut counts: Option<ToolCallCounts> = None;
        let mut model_binding: Option<ModelBinding> = None;
        let mut schemas: Vec<ToolSchema> = Vec::new();
        let mut dispatch: BTreeMap<String, ToolId> = BTreeMap::new();
        let mut completion_options: Option<CompletionOptions> = None;
        let mut sys = sys;
        let mut early_return: Option<String> = None;
        let mut jump_heading: Option<String> = None;

        for block in &section.blocks {
            match block {
                Block::Lua(program) => {
                    let returned = run_section_lua(
                        &vm,
                        program,
                        !scopes_ready,
                        has_children,
                        section,
                        store,
                        args,
                        execution,
                        observer,
                        debug,
                        prompt.replay.as_ref(),
                        bindings,
                        models,
                        bound,
                        shared_tools,
                        client.as_ref(),
                        max_tool_iterations,
                        last_reply.as_deref(),
                        &when,
                        index + 1,
                        &task_handles,
                        &prompt.sections,
                        &turns,
                        0,
                    );
                    match returned {
                        Ok(LuaBlockResult::Returned(Some(value))) => {
                            early_return = Some(value);
                            break;
                        }
                        Ok(LuaBlockResult::Returned(None)) => {}
                        Ok(LuaBlockResult::Jump(heading)) => {
                            jump_heading = Some(heading);
                            break;
                        }
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    }
                }
                Block::Prose { text, loop_capable } => {
                    if !scopes_ready {
                        let scopes = match vm.close_scopes(observer, &section.name) {
                            Ok(scopes) => scopes,
                            Err(error) => {
                                vm.teardown(observer, &section.name);
                                return Err(error);
                            }
                        };
                        counts = match vm.install_tool_call_counts(&scopes.tools) {
                            Ok(c) => Some(c),
                            Err(error) => {
                                vm.teardown(observer, &section.name);
                                return Err(error);
                            }
                        };
                        if let Some(binding) = scopes.model.as_ref() {
                            let enriched =
                                crate::lua::enrich_sys_model(&vm.current_sys(&sys), binding);
                            if let Err(error) = vm.re_seal_sys(&enriched) {
                                vm.teardown(observer, &section.name);
                                return Err(error);
                            }
                            sys = enriched;
                            completion_options = Some(binding.completion_options());
                        }
                        model_binding = scopes.model;
                        let (prepared_schemas, prepared_dispatch) = match bound {
                            Some(bound_prompt) => {
                                match prepare_effective_scope(
                                    bound_prompt,
                                    &scopes.tools,
                                    registry,
                                    execution,
                                    observer,
                                    &section.name,
                                ) {
                                    Ok(prepared) => prepared,
                                    Err(error) => {
                                        vm.teardown(observer, &section.name);
                                        return Err(error);
                                    }
                                }
                            }
                            None => (Vec::new(), BTreeMap::new()),
                        };
                        schemas = prepared_schemas;
                        dispatch = prepared_dispatch;
                        let _ = scopes.tools;
                        scopes_ready = true;
                    }

                    let var = match vm.var() {
                        Ok(var) => var,
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    };
                    let prose = match subst::substitute(
                        text,
                        args,
                        last_reply.as_deref(),
                        None,
                        &var,
                        &sys,
                    ) {
                        Ok(prose) => prose,
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    };
                    if prose.trim().is_empty() {
                        continue;
                    }
                    if model_binding.is_none() {
                        vm.teardown(observer, &section.name);
                        return Err(Error::ModelRequired {
                            section: section.name.clone(),
                        });
                    }
                    if client.is_none() {
                        match GatewayClient::from_env() {
                            Ok(new_client) => client = Some(new_client),
                            Err(error) => {
                                vm.teardown(observer, &section.name);
                                return Err(error);
                            }
                        }
                    }
                    let Some(active_client) = client.as_ref() else {
                        continue;
                    };
                    let Some(options) = completion_options.as_ref() else {
                        vm.teardown(observer, &section.name);
                        return Err(Error::ModelRequired {
                            section: section.name.clone(),
                        });
                    };
                    let global_aliases = bound.map(BoundPrompt::alias_to_id);
                    let mode = if *loop_capable {
                        ProseMode::Loop {
                            max_tool_iterations,
                        }
                    } else {
                        ProseMode::SingleShot
                    };
                    let outcome = match run_prose_inference(
                        active_client,
                        &schemas,
                        &dispatch,
                        registry,
                        &mut conversation,
                        prose,
                        mode,
                        SectionProgress {
                            execution,
                            observer,
                            section: &section.name,
                            turns: turns.as_ref(),
                            debug,
                            completion_options: options,
                        },
                        counts.as_ref(),
                        global_aliases,
                    )
                    .await
                    {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    };
                    sys = crate::lua::enrich_sys_reply_finish_reason(
                        &sys,
                        outcome.finish_reason.as_deref(),
                    );
                    if let Err(error) = vm.re_seal_sys(&sys) {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                    if let Some(text) = outcome.text {
                        if let Err(error) = vm.bind_reply(&text, observer, &section.name) {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                        last_reply = Some(text);
                    }
                }
            }
        }

        // Classic prologue-only early return / jump tears down without closing
        // scope. A lua-only section that falls through still closes, matching today.
        if !scopes_ready
            && early_return.is_none()
            && jump_heading.is_none()
            && let Err(error) = vm.close_scopes(observer, &section.name)
        {
            vm.teardown(observer, &section.name);
            return Err(error);
        }

        vm.teardown(observer, &section.name);
        observer.observe(execution, &section.name, detail::SECTION_FINISHED);
        if let Some(heading) = jump_heading {
            let target = resolve_h2_index(&heading, &prompt.sections)?;
            last_reply = None;
            index = target;
            continue;
        }
        if let Some(value) = early_return {
            return Ok(value);
        }
        index += 1;
    }

    // Ran off the end.
    Ok(prompt
        .frontmatter
        .default_return
        .clone()
        .or(last_reply)
        .unwrap_or_else(|| "done".to_string()))
}

/// Runs one section Lua block with tasks/execute/jump and optional fanout.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors make_fanout_callback's borrowed run context"
)]
#[expect(
    clippy::too_many_lines,
    reason = "builds execute and fanout closures that share the full run context"
)]
fn run_section_lua(
    vm: &SectionVm,
    program: &crate::lua::LuaProgram,
    before_prose: bool,
    has_children: bool,
    section: &Section,
    store: &StoreRef,
    args: &str,
    execution: &str,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    shared: Option<&crate::lua::LuaProgram>,
    bindings: &ToolBindings,
    models: &ModelBindings,
    bound: Option<&BoundPrompt>,
    shared_tools: &SharedTools,
    client: Option<&GatewayClient>,
    max_tool_iterations: usize,
    last_reply: Option<&str>,
    when: &str,
    parent_id: usize,
    tasks: &[LuaSectionHandle],
    top_sections: &[Section],
    turns: &Arc<AtomicU32>,
    execute_depth: usize,
) -> Result<LuaBlockResult> {
    let fanout_store = store.clone();
    let fanout_args = args.to_string();
    let fanout_execution = execution.to_string();
    let fanout_when = when.to_string();
    let fanout_last_reply = last_reply.map(str::to_owned);
    let fanout_shared = shared.cloned();
    let fanout_bindings = bindings.clone();
    let fanout_models = models.clone();
    let fanout_client = client.cloned();
    let fanout_tools = shared_tools.clone();
    let children = section.children.clone();
    let fanout_callback = if has_children {
        Some(move |worker_heading: String, list_heading: String| {
            make_fanout_callback(
                &worker_heading,
                &list_heading,
                &children,
                &fanout_args,
                &fanout_store,
                &fanout_execution,
                observer,
                fanout_client.as_ref(),
                debug,
                fanout_shared.as_ref(),
                &fanout_bindings,
                &fanout_models,
                bound,
                &fanout_tools,
                max_tool_iterations,
                fanout_last_reply.as_deref(),
                &fanout_when,
                parent_id,
                top_sections.len(),
            )
        })
    } else {
        None
    };

    let exec_store = store.clone();
    let exec_args = args.to_string();
    let exec_execution = execution.to_string();
    let exec_when = when.to_string();
    let exec_last_reply = last_reply.map(str::to_owned);
    let exec_shared = shared.cloned();
    let exec_bindings = bindings.clone();
    let exec_models = models.clone();
    let exec_client = client.cloned();
    let exec_tools = shared_tools.clone();
    let exec_sections = top_sections.to_vec();
    let exec_turns = Arc::clone(turns);
    let exec_bound = bound.cloned();
    let execute_callback =
        move |target: LuaValue, input: Option<String>| -> std::result::Result<String, String> {
            let heading = resolve_section_target(target).map_err(|e| e.to_string())?;
            let next_depth = execute_depth + 1;
            if next_depth > MAX_EXECUTE_DEPTH {
                return Err(format!(
                    "execute recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
                ));
            }
            let worker = resolve_h2_section(&heading, &exec_sections).map_err(|e| e.to_string())?;
            let call_args = input.as_deref().unwrap_or(&exec_args);
            let handle = tokio::runtime::Handle::current();
            tokio::task::block_in_place(|| {
                handle.block_on(run_execute_section(
                    worker,
                    call_args,
                    &exec_store,
                    &exec_execution,
                    observer,
                    debug,
                    exec_shared.as_ref(),
                    &exec_bindings,
                    &exec_models,
                    exec_bound.as_ref(),
                    &exec_tools,
                    exec_client.as_ref(),
                    max_tool_iterations,
                    exec_last_reply.as_deref(),
                    &exec_when,
                    &exec_turns,
                    next_depth,
                    &exec_sections,
                ))
            })
            .map_err(|e| e.to_string())
        };

    if before_prose {
        vm.run_prologue_with_control(
            program,
            observer,
            &section.name,
            tasks,
            Some(&execute_callback),
            fanout_callback.as_ref(),
        )
    } else {
        vm.run_epilog_with_control(
            program,
            observer,
            &section.name,
            tasks,
            Some(&execute_callback),
            fanout_callback.as_ref(),
        )
    }
}

fn section_handles(sections: &[Section]) -> Vec<LuaSectionHandle> {
    sections
        .iter()
        .map(|section| {
            let has_prose = section
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Prose { .. }));
            LuaSectionHandle::new(&section.name, has_prose)
        })
        .collect()
}

fn resolve_h2_section<'a>(heading: &str, sections: &'a [Section]) -> Result<&'a Section> {
    let stripped = heading.trim();
    if !stripped.starts_with("##") || stripped.starts_with("###") {
        return Err(Error::Lua(format!(
            "section heading must use ## markers, got: {stripped}"
        )));
    }
    let name = stripped.trim_start_matches('#').trim();
    if name.is_empty() {
        return Err(Error::Lua(format!(
            "section heading has no name: {stripped}"
        )));
    }
    sections
        .iter()
        .find(|section| section.name == name)
        .ok_or_else(|| {
            let available: Vec<String> =
                sections.iter().map(|s| format!("## {}", s.name)).collect();
            Error::Lua(format!(
                "section `{stripped}` not found; available: {}",
                available.join(", ")
            ))
        })
}

fn resolve_h2_index(heading: &str, sections: &[Section]) -> Result<usize> {
    let section = resolve_h2_section(heading, sections)?;
    sections
        .iter()
        .position(|s| s.name == section.name)
        .ok_or_else(|| Error::Lua(format!("section `{heading}` index missing")))
}

/// Runs a named section as an `execute()` subroutine and returns its reply.
#[expect(
    clippy::too_many_arguments,
    reason = "subroutine shares the full run context with the top-level walker"
)]
#[expect(
    clippy::too_many_lines,
    reason = "mirrors the top-level section block walk for one isolated section"
)]
async fn run_execute_section(
    section: &Section,
    args: &str,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    shared: Option<&crate::lua::LuaProgram>,
    bindings: &ToolBindings,
    models: &ModelBindings,
    bound: Option<&BoundPrompt>,
    shared_tools: &SharedTools,
    client: Option<&GatewayClient>,
    max_tool_iterations: usize,
    last_reply: Option<&str>,
    when: &str,
    turns: &Arc<AtomicU32>,
    execute_depth: usize,
    top_sections: &[Section],
) -> Result<String> {
    let registry = shared_tools.registry();
    let task_handles = section_handles(top_sections);
    let sys = json!({
        "when": when,
        "now": now_rfc3339(),
        "id": 0,
        "section_name": section.name,
        "execution": execution,
        "section_count": top_sections.len(),
    });
    observer.observe(execution, &section.name, detail::SECTION_STARTED);

    let mut vm = if bound.is_none() {
        SectionVm::new_for_section(shared, bindings, models, execution, observer, &section.name)?
    } else {
        match shared {
            Some(program) => SectionVm::new_with_shared_bindings(
                program,
                bindings,
                models,
                execution,
                observer,
                &section.name,
            )?,
            None => SectionVm::new(None, execution, observer, &section.name)?,
        }
    };
    if let Err(error) = vm.inject_host(args, &sys, store, last_reply) {
        vm.teardown(observer, &section.name);
        return Err(error);
    }
    let mut client = client.cloned();
    if client.is_none()
        && !models.bindings().is_empty()
        && let Ok(new_client) = GatewayClient::from_env()
    {
        client = Some(new_client);
    }
    if let Some(infer_client) = client.as_ref() {
        attach_infer_hook(
            &vm,
            infer_client,
            shared_tools,
            store,
            execution,
            &section.name,
            max_tool_iterations,
            turns,
            bound,
            None,
        );
    }

    let has_children = !section.children.is_empty();
    let mut conversation: Vec<Message> = Vec::new();
    let mut scopes_ready = false;
    let mut counts: Option<ToolCallCounts> = None;
    let mut model_binding: Option<ModelBinding> = None;
    let mut schemas: Vec<ToolSchema> = Vec::new();
    let mut dispatch: BTreeMap<String, ToolId> = BTreeMap::new();
    let mut completion_options: Option<CompletionOptions> = None;
    let mut sys = sys;
    let mut section_reply: Option<String> = None;
    let mut early_return: Option<String> = None;

    for block in &section.blocks {
        match block {
            Block::Lua(program) => {
                let returned = run_section_lua(
                    &vm,
                    program,
                    !scopes_ready,
                    has_children,
                    section,
                    store,
                    args,
                    execution,
                    observer,
                    debug,
                    shared,
                    bindings,
                    models,
                    bound,
                    shared_tools,
                    client.as_ref(),
                    max_tool_iterations,
                    last_reply,
                    when,
                    0,
                    &task_handles,
                    top_sections,
                    turns,
                    execute_depth,
                );
                match returned {
                    Ok(LuaBlockResult::Returned(Some(value))) => {
                        early_return = Some(value);
                        break;
                    }
                    Ok(LuaBlockResult::Returned(None)) => {}
                    Ok(LuaBlockResult::Jump(heading)) => {
                        vm.teardown(observer, &section.name);
                        return Err(Error::Lua(format!(
                            "jump({heading}) is not allowed inside execute()"
                        )));
                    }
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                }
            }
            Block::Prose { text, loop_capable } => {
                if !scopes_ready {
                    let scopes = match vm.close_scopes(observer, &section.name) {
                        Ok(scopes) => scopes,
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    };
                    counts = match vm.install_tool_call_counts(&scopes.tools) {
                        Ok(c) => Some(c),
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    };
                    if let Some(binding) = scopes.model.as_ref() {
                        let enriched = crate::lua::enrich_sys_model(&vm.current_sys(&sys), binding);
                        if let Err(error) = vm.re_seal_sys(&enriched) {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                        sys = enriched;
                        completion_options = Some(binding.completion_options());
                    }
                    model_binding = scopes.model;
                    let (prepared_schemas, prepared_dispatch) = match bound {
                        Some(bound_prompt) => {
                            match prepare_effective_scope(
                                bound_prompt,
                                &scopes.tools,
                                &registry,
                                execution,
                                observer,
                                &section.name,
                            ) {
                                Ok(prepared) => prepared,
                                Err(error) => {
                                    vm.teardown(observer, &section.name);
                                    return Err(error);
                                }
                            }
                        }
                        None => (Vec::new(), BTreeMap::new()),
                    };
                    schemas = prepared_schemas;
                    dispatch = prepared_dispatch;
                    let _ = scopes.tools;
                    scopes_ready = true;
                }

                let var = match vm.var() {
                    Ok(var) => var,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                };
                let prose = match subst::substitute(text, args, last_reply, None, &var, &sys) {
                    Ok(prose) => prose,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                };
                if prose.trim().is_empty() {
                    continue;
                }
                if model_binding.is_none() {
                    vm.teardown(observer, &section.name);
                    return Err(Error::ModelRequired {
                        section: section.name.clone(),
                    });
                }
                if client.is_none() {
                    match GatewayClient::from_env() {
                        Ok(new_client) => client = Some(new_client),
                        Err(error) => {
                            vm.teardown(observer, &section.name);
                            return Err(error);
                        }
                    }
                }
                let Some(active_client) = client.as_ref() else {
                    continue;
                };
                let Some(options) = completion_options.as_ref() else {
                    vm.teardown(observer, &section.name);
                    return Err(Error::ModelRequired {
                        section: section.name.clone(),
                    });
                };
                let global_aliases = bound.map(BoundPrompt::alias_to_id);
                let mode = if *loop_capable {
                    ProseMode::Loop {
                        max_tool_iterations,
                    }
                } else {
                    ProseMode::SingleShot
                };
                let outcome = match run_prose_inference(
                    active_client,
                    &schemas,
                    &dispatch,
                    &registry,
                    &mut conversation,
                    prose,
                    mode,
                    SectionProgress {
                        execution,
                        observer,
                        section: &section.name,
                        turns,
                        debug,
                        completion_options: options,
                    },
                    counts.as_ref(),
                    global_aliases,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                };
                sys = crate::lua::enrich_sys_reply_finish_reason(
                    &sys,
                    outcome.finish_reason.as_deref(),
                );
                if let Err(error) = vm.re_seal_sys(&sys) {
                    vm.teardown(observer, &section.name);
                    return Err(error);
                }
                if let Some(text) = outcome.text {
                    if let Err(error) = vm.bind_reply(&text, observer, &section.name) {
                        vm.teardown(observer, &section.name);
                        return Err(error);
                    }
                    section_reply = Some(text);
                }
            }
        }
    }

    if !scopes_ready
        && early_return.is_none()
        && let Err(error) = vm.close_scopes(observer, &section.name)
    {
        vm.teardown(observer, &section.name);
        return Err(error);
    }

    vm.teardown(observer, &section.name);
    observer.observe(execution, &section.name, detail::SECTION_FINISHED);
    Ok(early_return.or(section_reply).unwrap_or_default())
}

#[expect(
    clippy::too_many_arguments,
    reason = "fanout callback threads all borrowed run context through to the arm executor"
)]
fn make_fanout_callback(
    worker_heading: &str,
    list_heading: &str,
    children: &[crate::parser::Section],
    args: &str,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    client: Option<&GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    shared: Option<&crate::lua::LuaProgram>,
    bindings: &ToolBindings,
    models: &ModelBindings,
    bound: Option<&BoundPrompt>,
    shared_tools: &SharedTools,
    max_tool_iterations: usize,
    last_reply: Option<&str>,
    when: &str,
    parent_id: usize,
    section_count: usize,
) -> std::result::Result<Vec<crate::lua::LuaFanoutResult>, String> {
    let worker = fanout::resolve_sibling(worker_heading, children).map_err(|e| e.to_string())?;
    let list = fanout::resolve_sibling(list_heading, children).map_err(|e| e.to_string())?;
    if list.items.is_empty() {
        return Err(format!("section `{}` has no pre-parsed items", list.name));
    }
    if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items.is_empty() {
        return Err(format!(
            "section `{}` is a list section, not a worker template",
            worker.name
        ));
    }

    let fanout_client = client.cloned();
    let ctx = fanout::FanoutContext {
        args,
        store,
        execution,
        observer,
        client: &fanout_client,
        debug,
        shared,
        bindings,
        models,
        bound,
        shared_tools,
        max_tool_iterations,
        last_reply,
        when,
        parent_id,
        section_count,
    };

    let handle = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        handle.block_on(fanout::run_fanout_arms(worker, &list.items, &ctx))
    })
    .map_err(|e| e.to_string())
}

pub(crate) fn prepare_effective_scope(
    bound: &BoundPrompt,
    scope: &ToolScope,
    registry: &ToolRegistry<'_>,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    observer.observe(execution, section, detail::TOOL_SCOPE_VALIDATION_STARTED);
    let result = validate_effective_scope_inner(bound, scope)
        .and_then(|()| prepare_scoped_tools(scope, registry));
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::TOOL_SCOPE_VALIDATION_SUCCEEDED
        } else {
            detail::TOOL_SCOPE_VALIDATION_FAILED
        },
    );
    result
}

fn validate_effective_scope_inner(bound: &BoundPrompt, scope: &ToolScope) -> Result<()> {
    let effective = scope
        .bindings()
        .iter()
        .map(crate::lua::ToolBinding::id)
        .collect::<BTreeSet<_>>();
    for pair in bound.near_duplicates() {
        let first_id = ToolId::new(pair.first.id.server(), pair.first.id.name());
        let second_id = ToolId::new(pair.second.id.server(), pair.second.id.name());
        if !effective.contains(&first_id) || !effective.contains(&second_id) {
            continue;
        }
        let first_alias = bound.id_to_alias().get(&first_id).cloned().ok_or_else(|| {
            Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            }
        })?;
        let second_alias = bound
            .id_to_alias()
            .get(&second_id)
            .cloned()
            .ok_or_else(|| Error::ToolScopeAnalysis {
                detail: "selected identity has no frozen alias".to_owned(),
            })?;
        return Err(Error::NearDuplicateTools {
            diagnostic: Box::new(NearDuplicateDiagnostic {
                first_alias,
                first_id,
                first_description: pair.first.description.clone(),
                first_annotations: pair.first.annotations,
                second_alias,
                second_id,
                second_description: pair.second.description.clone(),
                second_annotations: pair.second.annotations,
                similarity: pair.similarity,
            }),
        });
    }
    Ok(())
}

fn prepare_scoped_tools(
    scope: &ToolScope,
    registry: &ToolRegistry<'_>,
) -> Result<(Vec<ToolSchema>, BTreeMap<String, ToolId>)> {
    let mut schemas = Vec::with_capacity(scope.bindings().len());
    let mut dispatch = BTreeMap::new();
    for binding in scope.bindings() {
        let tool = registry
            .get(binding.id())
            .ok_or_else(|| Error::UnknownScopedTool(binding.alias().to_owned()))?;
        // Default model-facing text stays the registry description so bind
        // capability strings never leak into schemas unless the author
        // overrode `.description` on the Tool object before tools.add.
        let description = binding
            .model_description()
            .unwrap_or_else(|| tool.description())
            .to_owned();
        schemas.push(ToolSchema {
            name: binding.alias().to_owned(),
            description,
            parameters: tool.parameters_schema(),
        });
        dispatch.insert(binding.alias().to_owned(), binding.id().clone());
    }
    Ok((schemas, dispatch))
}

/// What one section's tool loop needs to report itself: where observations go, which
/// section they belong to, and the run-wide turn counter it advances.
///
/// Bundled rather than passed as three parameters so the loop's signature stays
/// readable, and so the counter is a run-wide total rather than a per-section
/// one.
pub(crate) struct SectionProgress<'a> {
    /// The identifier every observation from this loop carries.
    pub(crate) execution: &'a str,
    /// Where the loop reports its turns and tool calls.
    pub(crate) observer: &'a dyn Observer,
    /// The heading text every observation from this loop carries.
    pub(crate) section: &'a str,
    /// The run's model-turn total, advanced once per round trip.
    pub(crate) turns: &'a AtomicU32,
    /// Opt-in raw request/response capture for each model turn.
    pub(crate) debug: Option<&'a dyn DebugCapture>,
    /// Per-call model fields from the section's selected binding.
    pub(crate) completion_options: &'a CompletionOptions,
}

/// How many model rounds a prose block may take.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ProseMode {
    /// One model round; tool calls for that round are dispatched, then control
    /// returns even without a final text reply.
    SingleShot,
    /// Keep calling until text or `max_tool_iterations` is exhausted.
    Loop { max_tool_iterations: usize },
}

/// Text and finish reason from one prose or tool-loop inference.
#[derive(Debug, Clone)]
pub(crate) struct ProseInferenceResult {
    /// Model text when the round produced a reply; `None` for single-shot tool rounds.
    pub text: Option<String>,
    /// Backend `finish_reason` from the last completed model round, when present.
    pub finish_reason: Option<String>,
}

/// Drive one section's model call to a final text reply, dispatching any tool
/// calls the model requests along the way.
///
/// Starts a fresh conversation with `prose` as the user turn and runs the full
/// tool loop. Returns the final text and the last round's finish reason.
///
/// # Errors
/// Returns [`Error::UnknownTool`] if the model calls an alias absent from
/// `dispatch`,
/// [`Error::ToolLoopExhausted`] if the cap is hit without a text reply, or any
/// transport/backend error from a model call or a tool's own failure.
#[expect(
    clippy::too_many_arguments,
    reason = "counts and global_aliases extend the loop's borrowed context for per-VM call tracking"
)]
pub(crate) async fn run_tool_loop(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, ToolId>,
    registry: &ToolRegistry<'_>,
    prose: String,
    max_tool_iterations: usize,
    progress: SectionProgress<'_>,
    counts: Option<&ToolCallCounts>,
    global_aliases: Option<&BTreeMap<String, ToolId>>,
) -> Result<(String, Option<String>)> {
    let mut conversation = Vec::new();
    let outcome = run_prose_inference(
        client,
        schemas,
        dispatch,
        registry,
        &mut conversation,
        prose,
        ProseMode::Loop {
            max_tool_iterations,
        },
        progress,
        counts,
        global_aliases,
    )
    .await?;
    match outcome.text {
        Some(text) => Ok((text, outcome.finish_reason)),
        None => Err(Error::ToolLoopExhausted),
    }
}

/// Append `prose` to `conversation` and run model inference under `mode`.
///
/// Returns text when the model produces it. For [`ProseMode::SingleShot`],
/// text may be `None` after one round that only issued tool calls.
/// Conversation history accumulates for later prose blocks.
///
/// # Errors
/// Same failure modes as [`run_tool_loop`], except single-shot does not report
/// [`Error::ToolLoopExhausted`] when the sole round ends without text.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "counts and global_aliases extend the loop's borrowed context for per-VM call tracking"
)]
pub(crate) async fn run_prose_inference(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, ToolId>,
    registry: &ToolRegistry<'_>,
    conversation: &mut Vec<Message>,
    prose: String,
    mode: ProseMode,
    progress: SectionProgress<'_>,
    counts: Option<&ToolCallCounts>,
    global_aliases: Option<&BTreeMap<String, ToolId>>,
) -> Result<ProseInferenceResult> {
    let SectionProgress {
        execution,
        observer,
        section,
        turns,
        debug,
        completion_options,
    } = progress;

    let dialect_registry = ToolDialectRegistry::builtin();
    let dialect: &dyn ToolDialect = dialect_registry
        .get(completion_options.tool_dialect)
        .ok_or(Error::UnknownDialect(completion_options.tool_dialect))?;

    conversation.push(Message::user(prose));
    let tool_arg = if schemas.is_empty() {
        None
    } else {
        Some(schemas)
    };

    let max_tool_iterations = match mode {
        ProseMode::SingleShot => 1,
        ProseMode::Loop {
            max_tool_iterations,
        } => max_tool_iterations,
    };

    // One nonce per prose-inference invocation tags every untrusted result's
    // guard block, so the close tag is unguessable by any fetched content.
    let nonce = untrusted::nonce();

    for _ in 0..max_tool_iterations {
        let completion = tokio::select! {
            biased;
            () = cancel::wait_cancelled() => Err(Error::Interrupted),
            result = client.complete(conversation, tool_arg, completion_options) => result,
        };
        if let Err(Error::Interrupted) = &completion {
            return Err(Error::Interrupted);
        }
        if completion.is_err() {
            observer.observe(execution, section, detail::MODEL_TURN_FAILED);
        }
        let completion = completion?;

        // A round trip that produced a reply is a turn, whether the reply is
        // the section's final text or a batch of tool calls.
        let turn = turns.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        if let Some(capture) = debug {
            capture.on_event(
                execution,
                section,
                turn,
                DebugEvent::Request {
                    body: completion.request_body,
                },
            );
            capture.on_event(
                execution,
                section,
                turn,
                DebugEvent::Response {
                    body: completion.response_body.clone(),
                    finish_reason: completion.finish_reason.clone(),
                    reasoning_content: completion.reasoning_content.clone(),
                },
            );
        }
        observer.observe(execution, section, detail::MODEL_TURN_COMPLETED);

        match completion.result {
            CompletionResult::Text(text) => {
                if completion.finish_reason.as_deref() == Some("length") {
                    observer.observe(execution, section, detail::MODEL_TURN_TRUNCATED);
                }
                return Ok(ProseInferenceResult {
                    text: Some(text),
                    finish_reason: completion.finish_reason,
                });
            }
            CompletionResult::ToolCalls(calls) => {
                let finish_reason = completion.finish_reason.clone();
                // Dispatch each requested tool and collect results.
                let mut results: Vec<(String, String)> = Vec::with_capacity(calls.len());
                for call in &calls {
                    let Some(id) = dispatch.get(&call.name) else {
                        observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                        let global_exists =
                            global_aliases.is_some_and(|g| g.contains_key(&call.name));
                        let in_scope: Vec<String> = dispatch.keys().cloned().collect();
                        return Err(Error::OutOfScopeToolCall {
                            name: call.name.clone(),
                            global_exists,
                            in_scope,
                        });
                    };
                    let Some(tool) = registry.get(id) else {
                        observer.observe(execution, section, detail::TOOL_CALL_FAILED);
                        return Err(Error::UnknownScopedTool(call.name.clone()));
                    };
                    if let Some(counts) = counts {
                        counts.increment(&call.name)?;
                    }
                    let result = tool.call(call.arguments.clone()).await;
                    observer.observe(
                        execution,
                        section,
                        if result.is_ok() {
                            detail::TOOL_CALL_SUCCEEDED
                        } else {
                            detail::TOOL_CALL_FAILED
                        },
                    );
                    let result = result?;
                    let result = if tool.untrusted_output() {
                        untrusted::wrap(&result, &nonce)
                    } else {
                        result
                    };
                    results.push((call.id.clone(), result));
                }

                dialect.echo_tool_results(conversation, &calls, &results);
                if matches!(mode, ProseMode::SingleShot) {
                    return Ok(ProseInferenceResult {
                        text: None,
                        finish_reason,
                    });
                }
            }
        }
    }

    match mode {
        ProseMode::SingleShot => Ok(ProseInferenceResult {
            text: None,
            finish_reason: None,
        }),
        ProseMode::Loop { .. } => Err(Error::ToolLoopExhausted),
    }
}

/// The current UTC time as an RFC 3339 string, or empty on a formatting error.
pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
