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
//!
//! The VM setup half of the lifecycle - host injection, host APIs, control
//! globals, the shared replay, captured bindings - lives in the sibling
//! `section_vm` module, which the fanout arm drives with the same sequence.
//! Construction and the Lua limits install stay with the driver: a limits
//! failure must propagate bare, before any teardown observation exists.
//! The walk half - the ordered block loop with its conversation state,
//! per-block scope rebuild, and reply roll-forward - lives in the sibling
//! `block_walk` module; [`run_one_section`] composes setup, infer hook,
//! walk, teardown, and observation, owning the teardown boundary so every
//! path tears the VM down exactly once.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::fanout;
use crate::lua::{LuaSectionHandle, SectionVm, ToolBindings, resolve_section_target};
use crate::model::ModelBindings;
use crate::observe::{Observer, detail};
use crate::parser::{Block, Prompt, Section};
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result};
use mlua::Value as LuaValue;

use super::block_walk::{BlockWalkContext, SectionFlow, walk_section_blocks};
use super::config::RunLimits;
use super::scope::ToolAnalysis;
use super::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use super::support::{
    GENERIC_COMPLETION, MAX_EXECUTE_DEPTH, bridge_blocking, now_rfc3339_checked, sys_json,
};
use super::tools::attach_engine_infer_hook;

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
pub(super) struct WalkContext<'a> {
    pub(super) args: &'a str,
    pub(super) store: &'a StoreRef,
    pub(super) execution: &'a str,
    pub(super) observer: &'a dyn Observer,
    pub(super) observer_arc: &'a Arc<dyn Observer>,
    pub(super) debug: Option<&'a dyn DebugCapture>,
    pub(super) debug_arc: Option<&'a Arc<dyn DebugCapture>>,
    /// The shared library replayed as every section's first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library.
    pub(super) shared: &'a crate::lua::LuaProgram,
    pub(super) bindings: &'a ToolBindings,
    pub(super) models: &'a ModelBindings,
    pub(super) analysis: &'a ToolAnalysis,
    pub(super) shared_tools: &'a SharedTools,
    pub(super) max_tool_iterations: usize,
    pub(super) limits: RunLimits,
    pub(super) when: &'a str,
    pub(super) top_sections: &'a [Section],
    pub(super) task_handles: &'a [LuaSectionHandle],
    pub(super) turns: &'a Arc<AtomicU32>,
    /// `var` seeded from an earlier VM (top-level H1 hand-off); `None` for a
    /// contained chain.
    pub(super) initial_var: Option<&'a serde_json::Value>,
}

impl<'a> From<&WalkContext<'a>> for BlockWalkContext<'a> {
    /// The walk driver's block-walk inputs: the shared field list lives here
    /// alone, so a future field add cannot drift between construction sites.
    /// `item` is absent on the walk; only the fanout arm sets it.
    fn from(ctx: &WalkContext<'a>) -> Self {
        BlockWalkContext {
            args: ctx.args,
            execution: ctx.execution,
            observer: ctx.observer,
            debug: ctx.debug,
            bindings: ctx.bindings,
            models: ctx.models,
            analysis: ctx.analysis,
            shared_tools: ctx.shared_tools,
            max_tool_iterations: ctx.max_tool_iterations,
            limits: ctx.limits,
            turns: ctx.turns.as_ref(),
            item: None,
        }
    }
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
        WalkEnd::Exhausted(reply) => Ok(reply.unwrap_or_else(|| GENERIC_COMPLETION.to_string())),
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
/// This is the single engine every chain drives: VM construction and limits,
/// the setup half through [`setup_section_vm`] (shared with the fanout arm),
/// the infer hook, the block walk through [`walk_section_blocks`], then the
/// teardown boundary and the section-finished observation.
/// `siblings` is the caller's
/// own walk slice, from which the section's visible set (its siblings minus
/// itself, plus its direct children) is built for the control globals.
/// `section_id` is the `sys.id` value and the nested-call parent id (the
/// count of sections entered in this chain). `incoming_reply` is the
/// model reply visible to this section's first prose; the engine rolls it
/// forward as later prose in the same section produces text. The returned
/// [`SectionFlow`] tells the chain how the section ended.
#[expect(
    clippy::too_many_lines,
    reason = "the driver builds the three control-global callbacks inline; each owns clones of the run context so the persistent Lua closures hold no borrows"
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
    let now = now_rfc3339_checked()?;
    let sys = sys_json(
        ctx.when,
        &now,
        section_id,
        &section.name,
        ctx.execution,
        ctx.top_sections.len(),
    );

    ctx.observer
        .observe(ctx.execution, &section.name, detail::SECTION_STARTED);

    let mut vm = SectionVm::new_for_section(
        ctx.bindings,
        ctx.models,
        ctx.execution,
        ctx.observer,
        &section.name,
    )?;
    // A limits failure propagates bare: no teardown runs here, so no
    // LUA_TEARDOWN_* observation fires on this path.
    vm.apply_lua_limits(ctx.limits.lua_memory().get(), ctx.limits.lua_logs().get())?;

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
        // The run's handles already describe this same top-level slice;
        // cloning them beats rebuilding them on every execute() call.
        let exec_task_handles = ctx.task_handles.to_vec();
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
            // The contained chain runs the target's own sibling slice from
            // the target's index: a child target sits beside the caller's
            // children, a sibling target beside the caller's siblings.
            let (chain_slice, start) =
                match resolve_jump_target(&heading, &exec_siblings, &exec_caller)? {
                    JumpTarget::Child(index) => (exec_caller.children.as_slice(), index),
                    JumpTarget::Sibling(index) => (exec_siblings.as_slice(), index),
                };
            let call_args = input.as_deref().unwrap_or(&exec_args);
            let task_handles = exec_task_handles.clone();
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
    // The section's visible set is built once per section and shared: the
    // fanout callback takes a clone, the list callback takes the original.
    let visible = visible_sections(siblings, section);
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
        let visible = visible.clone();
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
    let list_callback = move |heading: String| -> std::result::Result<Vec<String>, Error> {
        list_items_from_visible(&heading, &visible)
    };

    // The setup half of the section lifecycle - host injection, host APIs,
    // the control globals, the shared replay, and the captured alias
    // bindings - is shared with the fanout arm; only the seed, the `sys`
    // extras, and the callbacks are the walk's own.
    let setup = SectionVmSetup {
        args: ctx.args,
        sys: &sys,
        store: ctx.store,
        last_reply: incoming_reply,
        seed: VmSeed::InitialVar(ctx.initial_var),
        observer_arc: ctx.observer_arc,
        section_name: &section.name,
        task_handles: ctx.task_handles,
        shared: ctx.shared,
    };
    if let Err(error) = setup_section_vm(
        &mut vm,
        &setup,
        execute_callback,
        fanout_callback,
        list_callback,
    ) {
        vm.teardown(ctx.observer, &section.name);
        return Err(error);
    }

    // The infer hook carries a lazy client source (F5): a nested `model:infer`
    // surfaces a concrete construction error on first use instead of the setup
    // swallowing it. The direct prose path below still builds `client` and
    // propagates its own error.
    attach_engine_infer_hook(
        &vm,
        client.clone(),
        ctx.limits,
        ctx.shared_tools,
        Arc::clone(ctx.observer_arc),
        ctx.debug_arc.cloned(),
        ctx.execution,
        &section.name,
        ctx.max_tool_iterations,
        ctx.turns,
        ctx.analysis,
    );

    // The walk half - the ordered block loop - reports how the section
    // ended, reading only the narrowed BlockWalkContext inputs. The teardown
    // boundary stays here: every path out of the walk tears the VM down
    // exactly once, and SECTION_FINISHED fires only when the walk completed
    // (a jump or return included), never on an error.
    let walk_ctx = BlockWalkContext::from(ctx);
    let result =
        walk_section_blocks(&mut vm, &walk_ctx, section, sys, incoming_reply, client).await;
    vm.teardown(ctx.observer, &section.name);
    let flow = result?;
    ctx.observer
        .observe(ctx.execution, &section.name, detail::SECTION_FINISHED);
    Ok(flow)
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
pub(super) fn list_items_from_visible(heading: &str, visible: &[Section]) -> Result<Vec<String>> {
    let section = fanout::resolve_sibling(heading, visible)?;
    if section.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` has no pre-parsed items",
            section.name
        )));
    }
    Ok(section.items.clone())
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
pub(super) fn resolve_jump_target(
    heading: &str,
    siblings: &[Section],
    jumper: &Section,
) -> Result<JumpTarget> {
    let visible = visible_sections(siblings, jumper);
    let target = fanout::resolve_sibling(heading, &visible)?;
    if let Some(index) = section_position(&jumper.children, target) {
        return Ok(JumpTarget::Child(index));
    }
    // `target` was resolved out of the visible set built from exactly these
    // two slices, so a miss here is an internal invariant violation, not a
    // user-facing Lua error.
    section_position(siblings, target)
        .map(JumpTarget::Sibling)
        .ok_or(Error::Internal(
            "resolved jump target is absent from the jumper's sibling slice",
        ))
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
        limits,
        last_reply,
        when,
        parent_id,
        section_count,
    };

    // The collection was converted to JSON member-by-member at the Lua
    // boundary (the same bridge `var` uses); the cap inside counts members.
    bridge_blocking(fanout::run_fanout_arms(worker, items, &ctx))
}
