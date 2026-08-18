//! The section-lifecycle engine.
//!
//! One private engine, [`run_one_section`], executes a single section's block
//! lifecycle: VM construction, host injection, the ordered Lua/prose block
//! walk, scope close, teardown, and per-section observation. Two thin adapters
//! drive it:
//!
//! - [`run_sections`] is the top-level walk. It loops over the prompt's
//!   sections with [`JumpPolicy::Follow`], carries the model reply across
//!   section boundaries (preserving it across a jump), and computes the run's
//!   final result (the last reply, else `"done"`).
//! - [`run_execute_section`] is the `execute()` subroutine. It drives the
//!   engine once for one isolated section with [`JumpPolicy::Reject`] and a
//!   frozen `execute_depth`, then returns that section's reply.
//!
//! The engine parameterizes exactly the divergences the two paths require: the
//! `sys.id` index, the host-injection seed (`initial_var`), the jump policy,
//! the `execute_depth`, and the cross-section reply carried in. Everything else
//! (the prose tool loop, scope validation, cancellation, teardown, and every
//! observation) is shared, so a fix lands in one place for both paths.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::json;

use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::fanout;
use crate::lua::{
    LuaBlockResult, LuaSectionHandle, SectionVm, ToolBindings, ToolCallCounts,
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

/// Whether a `jump(target)` may escape the section being walked.
#[derive(Clone, Copy)]
enum JumpPolicy {
    /// Top-level walk: a jump ends the section; the caller resolves the target.
    Follow,
    /// `execute()` subroutine: a jump is rejected as an error.
    Reject,
}

/// How one section's block walk ended.
enum SectionFlow {
    /// Ran off the section end, carrying the reply produced within it (if any).
    FellThrough { reply: Option<String> },
    /// A scalar early return ended the section (and, top-level, the run).
    Returned(String),
    /// A `jump(target)` requested control transfer (top-level only).
    Jumped {
        heading: String,
        reply: Option<String>,
    },
}

/// The borrowed run context every section in one walk shares.
///
/// Bundled so the single-section engine and the two adapters keep one linear
/// set of borrows rather than threading two dozen parameters each.
struct WalkContext<'a> {
    args: &'a str,
    store: &'a StoreRef,
    execution: &'a str,
    observer: &'a dyn Observer,
    observer_arc: &'a Arc<dyn Observer>,
    debug: Option<&'a dyn DebugCapture>,
    debug_arc: Option<&'a Arc<dyn DebugCapture>>,
    shared: Option<&'a crate::lua::LuaProgram>,
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
    jump_policy: JumpPolicy,
    /// `var` seeded from an earlier VM (top-level H1 hand-off); `None` for a
    /// subroutine.
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
    let ctx = WalkContext {
        args,
        store,
        execution,
        observer,
        observer_arc,
        debug,
        debug_arc,
        shared: prompt.replay.as_ref(),
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
        jump_policy: JumpPolicy::Follow,
        initial_var,
    };

    let mut reply: Option<String> = None;
    let mut index = 0usize;
    while index < prompt.sections.len() {
        let section = &prompt.sections[index];
        // `id` counts sections entered, so the first is 1, and serves as the
        // section's parent id for nested execute/fanout.
        match run_one_section(&ctx, section, index + 1, 0, reply.as_deref(), &mut client).await? {
            SectionFlow::Jumped { heading, reply: r } => {
                let target = resolve_h2_index(&heading, &prompt.sections)?;
                reply = r;
                index = target;
            }
            SectionFlow::Returned(value) => return Ok(value),
            SectionFlow::FellThrough { reply: r } => {
                reply = r;
                index += 1;
            }
        }
    }

    // Ran off the end.
    Ok(reply.unwrap_or_else(|| "done".to_string()))
}

/// Execute one section's block lifecycle over the shared [`WalkContext`].
///
/// This is the single engine both the top-level walk and the `execute()`
/// subroutine drive. `section_id` is the `sys.id` value and the nested-call
/// parent id (1-based for the top-level walk, `0` for a subroutine).
/// `incoming_reply` is the model reply visible to this section's first prose;
/// the engine rolls it forward as later prose in the same section produces
/// text. The returned [`SectionFlow`] tells the adapter how the section ended.
#[expect(
    clippy::too_many_lines,
    reason = "one linear section lifecycle: VM setup, ordered block walk, scope close, and teardown, kept together so both walk policies share exactly one implementation"
)]
async fn run_one_section(
    ctx: &WalkContext<'_>,
    section: &Section,
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
        ctx.shared,
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
        let exec_shared = ctx.shared.cloned();
        let exec_bindings = ctx.bindings.clone();
        let exec_models = ctx.models.clone();
        let exec_client = client.clone();
        let exec_tools = ctx.shared_tools.clone();
        let exec_sections = ctx.top_sections.to_vec();
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
            let worker = resolve_h2_section(&heading, &exec_sections)?;
            let call_args = input.as_deref().unwrap_or(&exec_args);
            // Return the structured error directly (LUA-012): the typed error and
            // its source cross the Lua boundary via `mlua::Error::external` rather
            // than being flattened to a string here.
            bridge_blocking(run_execute_section(
                worker,
                call_args,
                &exec_store,
                &exec_execution,
                exec_observer.as_ref(),
                &exec_observer,
                exec_debug.as_deref(),
                exec_debug.as_ref(),
                exec_shared.as_ref(),
                &exec_bindings,
                &exec_models,
                &exec_analysis,
                &exec_tools,
                exec_client.as_ref(),
                max_tool_iterations,
                limits,
                None,
                &exec_when,
                &exec_turns,
                next_depth,
                &exec_sections,
            ))
        }
    };
    let fanout_callback = {
        let fanout_store = ctx.store.clone();
        let fanout_args = ctx.args.to_string();
        let fanout_execution = ctx.execution.to_string();
        let fanout_when = ctx.when.to_string();
        let fanout_last_reply = incoming_reply.map(str::to_owned);
        let fanout_shared = ctx.shared.cloned();
        let fanout_bindings = ctx.bindings.clone();
        let fanout_models = ctx.models.clone();
        let fanout_client = client.clone();
        let fanout_tools = ctx.shared_tools.clone();
        let fanout_analysis = ctx.analysis.clone();
        let fanout_observer = Arc::clone(ctx.observer_arc);
        let fanout_debug = ctx.debug_arc.cloned();
        let children = section.children.clone();
        let section_count = ctx.top_sections.len();
        let limits = ctx.limits;
        let max_tool_iterations = ctx.max_tool_iterations;
        move |worker_heading: String, list_heading: String| {
            make_fanout_callback(
                &worker_heading,
                &list_heading,
                &children,
                &fanout_args,
                &fanout_store,
                &fanout_execution,
                fanout_observer.as_ref(),
                fanout_client.as_ref(),
                fanout_debug.as_deref(),
                fanout_shared.as_ref(),
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
    if let Err(error) =
        vm.install_control_globals(ctx.task_handles, execute_callback, fanout_callback)
    {
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
    // `tools.add`/`tools.local` between blocks reach the next model turn.
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
                    Ok(LuaBlockResult::Jump(heading)) => match ctx.jump_policy {
                        JumpPolicy::Follow => {
                            jump_heading = Some(heading);
                            break;
                        }
                        JumpPolicy::Reject => {
                            vm.teardown(ctx.observer, &section.name);
                            return Err(Error::Lua(format!(
                                "jump({heading}) is not allowed inside execute()"
                            )));
                        }
                    },
                    Err(error) => {
                        vm.teardown(ctx.observer, &section.name);
                        return Err(error);
                    }
                }
            }
            Block::Prose { text, loop_capable } => {
                let effective_bindings =
                    match current_tool_bindings(ctx.bindings, &vm.tool_runtime) {
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
                // `tools.add` or `tools.local`) so the tool loop can count
                // their calls; `ensure` is idempotent on existing aliases.
                if let Some(counts) = counts.as_ref() {
                    let new_aliases = effective_bindings
                        .iter()
                        .map(|binding| binding.alias())
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
///
/// A thin adapter over [`run_one_section`] with the subroutine policy: `sys.id`
/// is `0`, no `initial_var` seed, jumps are rejected, and the caller's
/// `execute_depth` is threaded through so the recursion cap is enforced.
#[expect(
    clippy::too_many_arguments,
    reason = "subroutine shares the full run context with the top-level walker before it is folded into one WalkContext"
)]
async fn run_execute_section(
    section: &Section,
    args: &str,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    observer_arc: &Arc<dyn Observer>,
    debug: Option<&dyn DebugCapture>,
    debug_arc: Option<&Arc<dyn DebugCapture>>,
    shared: Option<&crate::lua::LuaProgram>,
    bindings: &ToolBindings,
    models: &ModelBindings,
    analysis: &ToolAnalysis,
    shared_tools: &SharedTools,
    client: Option<&GatewayClient>,
    max_tool_iterations: usize,
    limits: RunLimits,
    last_reply: Option<&str>,
    when: &str,
    turns: &Arc<AtomicU32>,
    execute_depth: usize,
    top_sections: &[Section],
) -> Result<String> {
    let task_handles = section_handles(top_sections);
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
        when,
        top_sections,
        task_handles: &task_handles,
        turns,
        jump_policy: JumpPolicy::Reject,
        initial_var: None,
    };
    let mut client = client.cloned();
    match run_one_section(&ctx, section, 0, execute_depth, last_reply, &mut client).await? {
        SectionFlow::Returned(value) => Ok(value),
        SectionFlow::FellThrough { reply } => Ok(reply.unwrap_or_default()),
        // Unreachable under `JumpPolicy::Reject`: a jump returns an error from
        // the engine before it can surface here. Kept typed rather than
        // panicking so a future policy change fails loudly, not silently.
        SectionFlow::Jumped { .. } => Err(Error::Internal(
            "execute() subroutine produced a jump despite the reject policy",
        )),
    }
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
    analysis: &ToolAnalysis,
    shared_tools: &SharedTools,
    max_tool_iterations: usize,
    limits: RunLimits,
    last_reply: Option<&str>,
    when: &str,
    parent_id: usize,
    section_count: usize,
) -> std::result::Result<Vec<crate::lua::LuaFanoutResult>, Error> {
    let worker = fanout::resolve_sibling(worker_heading, children)?;
    let list = fanout::resolve_sibling(list_heading, children)?;
    if list.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` has no pre-parsed items",
            list.name
        )));
    }
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

    bridge_blocking(fanout::run_fanout_arms(worker, &list.items, &ctx))
}
