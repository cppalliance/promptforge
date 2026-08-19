//! The section-lifecycle engine.
//!
//! One private engine, [`run_one_section`], executes a single section's block
//! lifecycle: VM construction, host injection, the ordered Lua/prose block
//! walk, scope close, teardown, and per-section observation. One recursive
//! chain function, [`walk_siblings`], drives it at every level:
//!
//! - [`run_sections`] starts the top-level chain over the prompt's sections.
//!   It carries the model reply across section boundaries (preserving it
//!   across a jump) and computes the run's final result (the last reply,
//!   else `"done"`). A jump to a child heading recurses into a child-level
//!   chain over the jumper's children under the same rules; when that level
//!   exhausts, the parent chain resumes after the jumper. A scalar return in
//!   the top-level chain ends the run.
//! - An `execute()` call runs a contained chain over the target's sibling
//!   slice from the target's index, with every normal rule (fall-through,
//!   off-walk skips, jumps, child chains). The outer chain never moves while
//!   the contained chain runs. When the contained chain ends - its level
//!   exhausts or a return fires - its final reply is the call's return
//!   value, and a return ends only the chain it fires in. Each contained
//!   chain counts its own `sys.id` from 1 and carries the caller's
//!   `execute_depth` so the recursion cap holds across nesting.
//!
//! The engine parameterizes exactly the divergences the paths require: the
//! `sys.id` index, the host-injection seed (`initial_var`), the
//! `execute_depth`, and the cross-section reply carried in. Everything else
//! (the prose tool loop, scope validation, cancellation, teardown, and every
//! observation) is shared, so a fix lands in one place for every path.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::json;

use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::fanout;
use crate::lua::{
    LuaBlockResult, LuaSectionHandle, SectionVm, ToolBinding, ToolBindings, ToolCallCounts,
    current_tool_bindings, resolve_section_target,
};
use crate::model::{CompletionOptions, ModelBinding, ModelBindings};
use crate::observe::{Observer, detail};
use crate::parser::{Block, Prompt, Section};
use crate::store::StoreRef;
use crate::subst;
use crate::tools::SharedTools;
use crate::{Error, Result};
use mlua::Value as LuaValue;

use super::config::RunLimits;
use super::gateway::{GatewaySource, env_client_with_limits};
use super::scope::{ToolAnalysis, prepare_effective_scope};
use super::support::{MAX_EXECUTE_DEPTH, bridge_blocking, now_rfc3339_checked};
use super::tool_loop::{ProseMode, SectionProgress, run_prose_inference};
use super::tools::attach_infer_hook;

/// How one section's block walk ended.
enum SectionFlow {
    /// Ran off the section end, carrying the reply produced within it (if any).
    FellThrough { reply: Option<String> },
    /// A scalar early return ended the section (and the chain it fired in).
    Returned(String),
    /// A `jump(target)` requested control transfer (any walked level).
    Jumped {
        heading: String,
        reply: Option<String>,
    },
}

/// How one chain ended.
enum WalkEnd {
    /// The level ran off its last section, carrying the final reply produced
    /// within it (if any).
    Exhausted(Option<String>),
    /// A scalar early return ended the chain (the run, for the top-level
    /// chain; the call's return value, for an `execute()` chain).
    Returned(String),
}

/// The borrowed run context every section in one chain shares.
///
/// Bundled so the single-section engine and the chain drivers keep one linear
/// set of borrows rather than threading two dozen parameters each.
struct WalkContext<'a> {
    args: &'a str,
    store: &'a StoreRef,
    execution: &'a str,
    observer: &'a dyn Observer,
    observer_arc: &'a Arc<dyn Observer>,
    debug: Option<&'a dyn DebugCapture>,
    debug_arc: Option<&'a Arc<dyn DebugCapture>>,
    /// The shared library replayed as every section's first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library.
    shared: &'a crate::lua::LuaProgram,
    bindings: &'a ToolBindings,
    models: &'a ModelBindings,
    analysis: &'a ToolAnalysis,
    shared_tools: &'a SharedTools,
    max_tool_iterations: usize,
    limits: RunLimits,
    when: &'a str,
    top_sections: &'a [Section],
    task_handles: &'a [LuaSectionHandle],
    turns: &'a Arc<AtomicU32>,
    /// `var` seeded from an earlier VM (top-level H1 hand-off); `None` for a
    /// contained chain.
    initial_var: Option<&'a serde_json::Value>,
}

/// Walk the prompt's top-level sections, reporting each boundary, and return
/// the run's result.
///
/// Split out of [`run`](super::run) so that every way the walk can end - a Lua
/// return, an error, running off the last section - passes through one place
/// that emits the run's final observation.
///
/// # Errors
/// Returns the same errors as [`run`](super::run), which documents them.
#[expect(
    clippy::too_many_arguments,
    reason = "the walk keeps its borrowed run inputs explicit and linear so the adapter can build one WalkContext"
)]
pub(crate) async fn run_sections(
    prompt: &Prompt,
    bindings: &ToolBindings,
    models: &ModelBindings,
    analysis: &ToolAnalysis,
    initial_var: Option<&serde_json::Value>,
    args: &str,
    shared_tools: &SharedTools,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    observer_arc: &Arc<dyn Observer>,
    mut client: Option<GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    debug_arc: Option<&Arc<dyn DebugCapture>>,
    limits: RunLimits,
    turns: Arc<AtomicU32>,
) -> Result<String> {
    let default_max_tool_iterations = limits.tool_iterations().get() as usize;
    let when = now_rfc3339_checked()?;

    // Resolve the tool-loop cap once: the prompt's declared budget, or the
    // runtime default when it declares none.
    let max_tool_iterations = prompt
        .frontmatter
        .max_tool_iterations
        .resolve(default_max_tool_iterations);

    let task_handles = section_handles(&prompt.sections);
    // Section startup replays the shared library unconditionally; a prompt
    // without one replays an empty compiled chunk instead, so the startup
    // sequence carries no `Option` branch.
    let empty_shared;
    let shared = if let Some(program) = prompt.replay.as_ref() {
        program
    } else {
        empty_shared = crate::lua::LuaProgram::empty()?;
        &empty_shared
    };
    let ctx = WalkContext {
        args,
        store,
        execution,
        observer,
        observer_arc,
        debug,
        debug_arc,
        shared,
        bindings,
        models,
        analysis,
        shared_tools,
        max_tool_iterations,
        limits,
        when: &when,
        top_sections: &prompt.sections,
        task_handles: &task_handles,
        turns: &turns,
        initial_var,
    };

    let mut entered = 0usize;
    match walk_siblings(
        &ctx,
        &prompt.sections,
        0,
        None,
        false,
        0,
        &mut client,
        &mut entered,
    )
    .await?
    {
        WalkEnd::Returned(value) => Ok(value),
        // Ran off the end.
        WalkEnd::Exhausted(reply) => Ok(reply.unwrap_or_else(|| "done".to_string())),
    }
}

/// Walk one sibling slice from `start` and report how the chain ended.
///
/// This is the one chain function at every heading level and for every entry
/// mode: the top-level walk runs the prompt's sections, a jump to a child
/// heading recurses into the jumper's children, and an `execute()` call runs
/// a contained chain over the target's slice - all under the same rules
/// (fall-through order, off-walk skips, the reply rolling forward across
/// sections). When a jump-started sub-walk's level exhausts, the parent
/// chain resumes after the jumper with the sub-walk's last reply. A scalar
/// return ends the chain it fires in: it propagates out of every sub-walk to
/// the chain's root, where the top-level chain turns it into the run's
/// result and an `execute()` chain into the call's return value.
/// `start_addressed` is true when the chain begins on an addressed target (a
/// jump or execute target), which runs even when marked off-walk.
/// `execute_depth` is the chain's nesting depth, threaded into every section
/// so nested `execute()` calls stay under the cap. `entered` counts the
/// sections this chain has entered and seeds each `sys.id`.
#[expect(
    clippy::too_many_arguments,
    reason = "the chain keeps its position, entry mode, and depth explicit and linear beside the shared WalkContext"
)]
async fn walk_siblings(
    ctx: &WalkContext<'_>,
    siblings: &[Section],
    start: usize,
    incoming_reply: Option<String>,
    start_addressed: bool,
    execute_depth: usize,
    client: &mut Option<GatewayClient>,
    entered: &mut usize,
) -> Result<WalkEnd> {
    let mut reply = incoming_reply;
    let mut index = start;
    // Arrival by addressing (a jump target, including a sub-walk's first
    // section) runs the section even when it is marked off-walk; only
    // fall-through arrival (including an unmarked walk start) applies the
    // skip.
    let mut addressed = start_addressed;
    while index < siblings.len() {
        let section = &siblings[index];
        if !addressed && section.is_off_walk() {
            index += 1;
            continue;
        }
        addressed = false;
        *entered += 1;
        match run_one_section(
            ctx,
            section,
            siblings,
            *entered,
            execute_depth,
            reply.as_deref(),
            client,
        )
        .await?
        {
            SectionFlow::Jumped { heading, reply: r } => {
                reply = r;
                match resolve_jump_target(&heading, siblings, section)? {
                    // A child target descends: the child-level chain runs the
                    // jumper's children from the target, and this chain
                    // resumes after the jumper when that level exhausts.
                    JumpTarget::Child(child_index) => match Box::pin(walk_siblings(
                        ctx,
                        &section.children,
                        child_index,
                        reply,
                        true,
                        execute_depth,
                        client,
                        entered,
                    ))
                    .await?
                    {
                        WalkEnd::Exhausted(child_reply) => {
                            reply = child_reply;
                            index += 1;
                        }
                        WalkEnd::Returned(value) => return Ok(WalkEnd::Returned(value)),
                    },
                    JumpTarget::Sibling(target) => {
                        index = target;
                        addressed = true;
                    }
                }
            }
            SectionFlow::Returned(value) => return Ok(WalkEnd::Returned(value)),
            SectionFlow::FellThrough { reply: r } => {
                reply = r;
                index += 1;
            }
        }
    }
    Ok(WalkEnd::Exhausted(reply))
}

/// Execute one section's block lifecycle over the shared [`WalkContext`].
///
/// This is the single engine every chain drives. `siblings` is the caller's
/// own walk slice, from which the section's visible set (its siblings minus
/// itself, plus its direct children) is built for the control globals.
/// `section_id` is the `sys.id` value and the nested-call parent id (the
/// count of sections entered in this chain). `incoming_reply` is the
/// model reply visible to this section's first prose; the engine rolls it
/// forward as later prose in the same section produces text. The returned
/// [`SectionFlow`] tells the chain how the section ended.
#[expect(
    clippy::too_many_lines,
    reason = "one linear section lifecycle: VM setup, ordered block walk, scope close, and teardown, kept together so every chain shares exactly one implementation"
)]
async fn run_one_section(
    ctx: &WalkContext<'_>,
    section: &Section,
    siblings: &[Section],
    section_id: usize,
    execute_depth: usize,
    incoming_reply: Option<&str>,
    client: &mut Option<GatewayClient>,
) -> Result<SectionFlow> {
    let registry = ctx.shared_tools.registry();
    let now = now_rfc3339_checked()?;
    let sys = json!({
        "when": ctx.when,
        "now": now,
        "id": section_id,
        "section_name": section.name,
        "execution": ctx.execution,
        "section_count": ctx.top_sections.len(),
    });

    ctx.observer
        .observe(ctx.execution, &section.name, detail::SECTION_STARTED);

    let mut vm = SectionVm::new_for_section(
        ctx.bindings,
        ctx.models,
        ctx.execution,
        ctx.observer,
        &section.name,
    )?;
    vm.apply_lua_limits(ctx.limits.lua_memory().get(), ctx.limits.lua_logs().get())?;
    if let Err(error) =
        vm.inject_host_with_var(ctx.args, &sys, ctx.store, incoming_reply, ctx.initial_var)
    {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }
    if let Err(error) = vm.install_host_apis(ctx.observer_arc, &section.name) {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }

    // Control globals are installed once for the section's whole lifecycle.
    // Both callbacks own clones of the run context so the persistent Lua
    // closures hold no borrows; the observer and debug captures go through
    // their Arc handles for the same reason.
    let execute_callback = {
        let exec_store = ctx.store.clone();
        let exec_args = ctx.args.to_string();
        let exec_execution = ctx.execution.to_string();
        let exec_when = ctx.when.to_string();
        let exec_shared = ctx.shared.to_owned();
        let exec_bindings = ctx.bindings.clone();
        let exec_models = ctx.models.clone();
        let exec_client = client.clone();
        let exec_tools = ctx.shared_tools.clone();
        let exec_siblings = siblings.to_vec();
        let exec_caller = section.clone();
        let exec_top = ctx.top_sections.to_vec();
        let exec_turns = Arc::clone(ctx.turns);
        let exec_analysis = ctx.analysis.clone();
        let exec_observer = Arc::clone(ctx.observer_arc);
        let exec_debug = ctx.debug_arc.cloned();
        let limits = ctx.limits;
        let max_tool_iterations = ctx.max_tool_iterations;
        move |target: LuaValue, input: Option<String>| -> std::result::Result<String, Error> {
            let heading = resolve_section_target(target).map_err(Error::lua)?;
            let next_depth = execute_depth + 1;
            if next_depth > MAX_EXECUTE_DEPTH {
                return Err(Error::Lua(format!(
                    "execute recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
                )));
            }
            let visible = visible_sections(&exec_siblings, &exec_caller);
            let worker = fanout::resolve_sibling(&heading, &visible)?;
            // The contained chain runs the target's own sibling slice from
            // the target's index: a child target sits beside the caller's
            // children, a sibling target beside the caller's siblings.
            let (chain_slice, start) =
                if let Some(index) = section_position(&exec_caller.children, worker) {
                    (exec_caller.children.as_slice(), index)
                } else {
                    let index = section_position(&exec_siblings, worker)
                        .ok_or_else(|| Error::Lua(format!("section `{heading}` index missing")))?;
                    (exec_siblings.as_slice(), index)
                };
            let call_args = input.as_deref().unwrap_or(&exec_args);
            let task_handles = section_handles(&exec_top);
            let ctx = WalkContext {
                args: call_args,
                store: &exec_store,
                execution: &exec_execution,
                observer: exec_observer.as_ref(),
                observer_arc: &exec_observer,
                debug: exec_debug.as_deref(),
                debug_arc: exec_debug.as_ref(),
                shared: &exec_shared,
                bindings: &exec_bindings,
                models: &exec_models,
                analysis: &exec_analysis,
                shared_tools: &exec_tools,
                max_tool_iterations,
                limits,
                when: &exec_when,
                top_sections: &exec_top,
                task_handles: &task_handles,
                turns: &exec_turns,
                initial_var: None,
            };
            let mut client = exec_client.clone();
            // A contained chain counts its own `sys.id` from 1, like a fresh
            // run; the caller's chain keeps its own count.
            let mut entered = 0usize;
            // Return the structured error directly (LUA-012): the typed error and
            // its source cross the Lua boundary via `mlua::Error::external` rather
            // than being flattened to a string here.
            let end = bridge_blocking(walk_siblings(
                &ctx,
                chain_slice,
                start,
                None,
                true,
                next_depth,
                &mut client,
                &mut entered,
            ))?;
            // A return ends the chain, and its value is the call's return;
            // an exhausted chain returns its final reply.
            Ok(match end {
                WalkEnd::Returned(value) => value,
                WalkEnd::Exhausted(reply) => reply.unwrap_or_default(),
            })
        }
    };
    let fanout_callback = {
        let fanout_store = ctx.store.clone();
        let fanout_args = ctx.args.to_string();
        let fanout_execution = ctx.execution.to_string();
        let fanout_when = ctx.when.to_string();
        let fanout_last_reply = incoming_reply.map(str::to_owned);
        let fanout_shared = ctx.shared.to_owned();
        let fanout_bindings = ctx.bindings.clone();
        let fanout_models = ctx.models.clone();
        let fanout_client = client.clone();
        let fanout_tools = ctx.shared_tools.clone();
        let fanout_analysis = ctx.analysis.clone();
        let fanout_observer = Arc::clone(ctx.observer_arc);
        let fanout_debug = ctx.debug_arc.cloned();
        let visible = visible_sections(siblings, section);
        let section_count = ctx.top_sections.len();
        let limits = ctx.limits;
        let max_tool_iterations = ctx.max_tool_iterations;
        move |worker_heading: String, items: Vec<serde_json::Value>| {
            make_fanout_callback(
                &worker_heading,
                &items,
                &visible,
                &fanout_args,
                &fanout_store,
                &fanout_execution,
                fanout_observer.as_ref(),
                fanout_client.as_ref(),
                fanout_debug.as_deref(),
                &fanout_shared,
                &fanout_bindings,
                &fanout_models,
                &fanout_analysis,
                &fanout_tools,
                max_tool_iterations,
                limits,
                fanout_last_reply.as_deref(),
                &fanout_when,
                section_id,
                section_count,
            )
        }
    };
    let list_callback = {
        let visible = visible_sections(siblings, section);
        move |heading: String| -> std::result::Result<Vec<String>, Error> {
            list_items_from_visible(&heading, &visible)
        }
    };
    if let Err(error) = vm.install_control_globals(
        ctx.task_handles,
        execute_callback,
        fanout_callback,
        list_callback,
    ) {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }

    // The shared library replays as the section's first chunk with the full
    // host environment installed; the captured alias globals install only
    // after the replay, so a declared alias wins over a same-named shared
    // global.
    if let Err(error) = vm.replay_shared(ctx.shared, ctx.observer, &section.name) {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }
    if let Err(error) = vm.install_captured_bindings() {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }

    // The infer hook carries a lazy client source (F5): a nested `model:infer`
    // surfaces a concrete construction error on first use instead of the setup
    // swallowing it. The direct prose path below still builds `client` and
    // propagates its own error.
    attach_infer_hook(
        &vm,
        GatewaySource::from_optional(client.clone(), ctx.limits),
        ctx.shared_tools,
        Arc::clone(ctx.observer_arc),
        ctx.debug_arc.cloned(),
        ctx.execution,
        &section.name,
        ctx.max_tool_iterations,
        ctx.turns,
        Some(ctx.analysis),
        None,
    );

    let mut conversation: Vec<Message> = Vec::new();
    // Gates the one-time model resolution and counts install at the first
    // prose block below. Schemas and dispatch rebuild on EVERY prose block so
    // `tools.add`/`tools.add_local` between blocks reach the next model turn.
    let mut seen_prose = false;
    let mut counts: Option<ToolCallCounts> = None;
    let mut model_binding: Option<ModelBinding> = None;
    let mut completion_options: Option<CompletionOptions> = None;
    let mut sys = sys;
    let mut early_return: Option<String> = None;
    let mut jump_heading: Option<String> = None;
    // The reply visible to this section's prose. It starts at the incoming
    // reply and rolls forward as prose produces text, so both the `{{reply}}`
    // substitution and the Lua `reply` global stay consistent within a section.
    let mut reply: Option<String> = incoming_reply.map(str::to_owned);

    for block in &section.blocks {
        match block {
            Block::Lua(program) => {
                let returned = vm.run_chunk(program, ctx.observer, &section.name);
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
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                }
            }
            Block::Prose { text, loop_capable } => {
                let effective_bindings = match current_tool_bindings(ctx.bindings, &vm.tool_runtime)
                {
                    Ok(bindings) => bindings,
                    Err(error) => {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                };
                if !seen_prose {
                    seen_prose = true;
                    counts = match vm.install_tool_call_counts(&effective_bindings) {
                        Ok(c) => Some(c),
                        Err(error) => {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(error);
                        }
                    };
                    let resolved_model =
                        match crate::lua::resolve_model_binding(ctx.models, &vm.model_runtime) {
                            Ok(model) => model,
                            Err(error) => {
                                vm.teardown(ctx.observer, &section.name);
                                return Err(error);
                            }
                        };
                    if let Some(binding) = resolved_model.as_ref() {
                        let current = match vm.current_sys(&sys) {
                            Ok(current) => current,
                            Err(error) => {
                                vm.teardown(ctx.observer, &section.name);
                                return Err(error);
                            }
                        };
                        let enriched = crate::lua::enrich_sys_model(&current, binding);
                        if let Err(error) = vm.re_seal_sys(&enriched) {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(error);
                        }
                        sys = enriched;
                        completion_options = Some(binding.completion_options());
                    }
                    model_binding = resolved_model;
                }
                let local_schemas = vm.local_tool_schemas();
                // Seed aliases added since the first prose block (via
                // `tools.add` or `tools.add_local`) so the tool loop can count
                // their calls; `ensure` is idempotent on existing aliases.
                if let Some(counts) = counts.as_ref() {
                    let new_aliases = effective_bindings
                        .iter()
                        .map(ToolBinding::alias)
                        .chain(local_schemas.iter().map(|schema| schema.name.as_str()));
                    for alias in new_aliases {
                        if let Err(error) = counts.ensure(alias) {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(error);
                        }
                    }
                }
                let (schemas, dispatch) = match prepare_effective_scope(
                    ctx.analysis,
                    &effective_bindings,
                    &local_schemas,
                    &registry,
                    ctx.execution,
                    ctx.observer,
                    &section.name,
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                };

                let var = match vm.var() {
                    Ok(var) => var,
                    Err(error) => {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                };
                let prose =
                    match subst::substitute(text, ctx.args, reply.as_deref(), None, &var, &sys) {
                        Ok(prose) => prose,
                        Err(error) => {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(error);
                        }
                    };
                if prose.trim().is_empty() {
                    continue;
                }
                if model_binding.is_none() {
                    vm.teardown(ctx.observer, &section.name);
                    return Err(Error::ModelRequired {
                        section: section.name.clone(),
                    });
                }
                if client.is_none() {
                    match env_client_with_limits(ctx.limits) {
                        Ok(new_client) => *client = Some(new_client),
                        Err(error) => {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(error);
                        }
                    }
                }
                let Some(active_client) = client.as_ref() else {
                    // The block just above guarantees a client exists here; a
                    // `None` is an internal invariant violation, not a reason to
                    // silently skip this section's prose.
                    vm.teardown(ctx.observer, &section.name);
                    return Err(Error::Internal(
                        "model-facing prose reached inference with no gateway client",
                    ));
                };
                let Some(options) = completion_options.as_ref() else {
                    vm.teardown(ctx.observer, &section.name);
                    return Err(Error::ModelRequired {
                        section: section.name.clone(),
                    });
                };
                let global_aliases = Some(&ctx.analysis.alias_to_id);
                let mode = if *loop_capable {
                    ProseMode::Loop {
                        max_tool_iterations: ctx.max_tool_iterations,
                    }
                } else {
                    ProseMode::SingleShot
                };
                // Local tools are Lua functions on this section VM; route
                // their calls back into it rather than the registry.
                let local_dispatch =
                    |alias: &str, args: serde_json::Value| vm.call_local_tool(alias, &args);
                let outcome = match run_prose_inference(
                    active_client,
                    &schemas,
                    &dispatch,
                    &registry,
                    &mut conversation,
                    prose,
                    mode,
                    SectionProgress {
                        execution: ctx.execution,
                        observer: ctx.observer,
                        section: &section.name,
                        turns: ctx.turns.as_ref(),
                        debug: ctx.debug,
                        completion_options: options,
                    },
                    counts.as_ref(),
                    global_aliases,
                    Some(&local_dispatch),
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                };
                sys = crate::lua::enrich_sys_reply_finish_reason(
                    &sys,
                    outcome.finish_reason.as_deref(),
                );
                if let Err(error) = vm.re_seal_sys(&sys) {
                    vm.teardown(ctx.observer, &section.name);
                    return Err(error);
                }
                if let Some(text) = outcome.text {
                    if let Err(error) = vm.bind_reply(&text, ctx.observer, &section.name) {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                    reply = Some(text);
                }
            }
        }
    }

    vm.teardown(ctx.observer, &section.name);
    ctx.observer
        .observe(ctx.execution, &section.name, detail::SECTION_FINISHED);

    if let Some(heading) = jump_heading {
        return Ok(SectionFlow::Jumped { heading, reply });
    }
    if let Some(value) = early_return {
        return Ok(SectionFlow::Returned(value));
    }
    Ok(SectionFlow::FellThrough { reply })
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

/// The index of `target` in `slice`, matched on the parser-unique
/// `(level, name)` pair; `None` when the slice does not contain it.
fn section_position(slice: &[Section], target: &Section) -> Option<usize> {
    slice
        .iter()
        .position(|s| s.level == target.level && s.name == target.name)
}

/// The sections a running section may address by heading: the caller's own
/// sibling slice minus the caller itself, plus the caller's direct children.
///
/// The caller is found in `siblings` by its parser-unique `(level, name)`
/// pair and excluded by index; a caller that is not in the slice excludes
/// nothing. The parent, aunts/uncles, nieces/nephews, and grandchildren are
/// never in the set, so a resolution error that lists the set cannot leak the
/// rest of the document's structure.
fn visible_sections(siblings: &[Section], caller: &Section) -> Vec<Section> {
    let caller_index = section_position(siblings, caller);
    siblings
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != caller_index)
        .map(|(_, section)| section.clone())
        .chain(caller.children.iter().cloned())
        .collect()
}

/// Resolves `heading` against a caller's visible set and returns the matched
/// section's pre-parsed list items.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no visible
/// section, or matches more than one (see [`fanout::resolve_sibling`]), or
/// when the resolved section has no pre-parsed items - the error that catches
/// naming a prose section by mistake.
pub(crate) fn list_items_from_visible(heading: &str, visible: &[Section]) -> Result<Vec<String>> {
    let section = fanout::resolve_sibling(heading, visible)?;
    Ok(require_pre_parsed_items(section)?.to_vec())
}

/// Returns the section's pre-parsed list items, or the error that catches
/// naming a prose section by mistake.
///
/// # Errors
/// Returns [`Error::Lua`] when the section has no pre-parsed items.
fn require_pre_parsed_items(section: &Section) -> Result<&[String]> {
    if section.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` has no pre-parsed items",
            section.name
        )));
    }
    Ok(&section.items)
}

/// Where a jump transfers control, resolved against the jumper's visible set.
#[derive(Debug)]
pub(crate) enum JumpTarget {
    /// A flat index move within the jumper's own slice.
    Sibling(usize),
    /// A descent into the jumper's child slice, starting at this index.
    Child(usize),
}

/// Resolves `heading` against the jumper's visible set (its sibling slice
/// minus itself, plus its direct children) and classifies the target: a
/// direct child of the jumper starts a child-level walk; anything else is a
/// sibling within the jumper's own slice.
///
/// Resolution is an exact `(level, name)` match (see
/// [`fanout::resolve_sibling`]): two visible sections sharing an address
/// error loudly as ambiguous instead of silently resolving to the first.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no visible
/// section, or matches more than one.
pub(crate) fn resolve_jump_target(
    heading: &str,
    siblings: &[Section],
    jumper: &Section,
) -> Result<JumpTarget> {
    let visible = visible_sections(siblings, jumper);
    let target = fanout::resolve_sibling(heading, &visible)?;
    if let Some(index) = section_position(&jumper.children, target) {
        return Ok(JumpTarget::Child(index));
    }
    section_position(siblings, target)
        .map(JumpTarget::Sibling)
        .ok_or_else(|| Error::Lua(format!("section `{heading}` index missing")))
}

#[expect(
    clippy::too_many_arguments,
    reason = "fanout callback threads all borrowed run context through to the arm executor"
)]
fn make_fanout_callback(
    worker_heading: &str,
    items: &[serde_json::Value],
    visible: &[crate::parser::Section],
    args: &str,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    client: Option<&GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    shared: &crate::lua::LuaProgram,
    bindings: &ToolBindings,
    models: &ModelBindings,
    analysis: &ToolAnalysis,
    shared_tools: &SharedTools,
    max_tool_iterations: usize,
    limits: RunLimits,
    last_reply: Option<&str>,
    when: &str,
    parent_id: usize,
    section_count: usize,
) -> std::result::Result<Vec<crate::lua::LuaFanoutResult>, Error> {
    let worker = fanout::resolve_sibling(worker_heading, visible)?;
    if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` is a list section, not a worker template",
            worker.name
        )));
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
        analysis,
        shared_tools,
        max_tool_iterations,
        fanout_concurrency: limits.fanout(),
        max_fanout_items: limits.fanout_items(),
        lua_memory_bytes: limits.lua_memory().get(),
        lua_log_events: limits.lua_logs().get(),
        last_reply,
        when,
        parent_id,
        section_count,
    };

    // The collection was converted to JSON member-by-member at the Lua
    // boundary (the same bridge `var` uses); the cap inside counts members.
    bridge_blocking(fanout::run_fanout_arms(worker, items, &ctx))
}
