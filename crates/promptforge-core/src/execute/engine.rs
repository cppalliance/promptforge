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
//!   value, and a return ends only the chain it fires in. Every section
//!   entry and every fanout arm takes the next id from the run-global
//!   counter (H1 keeps id 0), so entering the same section twice yields two
//!   ids; the chain carries the caller's `execute_depth` so the recursion
//!   cap holds across nesting.
//!
//! The engine parameterizes exactly the divergences the paths require: the
//! walk's `var` seed, the `execute_depth`, and the cross-section reply
//! carried in. The run-global `sys.id` counter rides the [`RunContext`],
//! shared by every chain and fanout. Everything else
//! (the prose tool loop, scope validation, cancellation, teardown, and every
//! observation) is shared, so a fix lands in one place for every path.
//!
//! The VM setup half of the lifecycle - host injection, host APIs, control
//! globals, the shared replay, captured bindings - lives in the sibling
//! `section_vm` module, which the fanout arm drives with the same sequence.
//! The control-global callbacks themselves are built once in
//! [`make_control_globals`] for both drivers, so an arm's
//! `execute`/`fanout`/`list_from_section` and its jump behave exactly as a
//! walked section's, resolved over the worker's visible set.
//! The walk half - the ordered block loop with its conversation state,
//! per-block scope rebuild, and reply roll-forward - lives in the sibling
//! `block_walk` module. [`run_one_section`] drives the lifecycle as one
//! [`SectionContext`]: construction absorbs VM construction, the Lua limits
//! install (a limits failure propagates bare, before any teardown
//! observation exists), the setup half, and the infer hook; the block walk
//! runs as the frame's `run`; and the driver owns the teardown boundary, so
//! every path tears the VM down exactly once.

use crate::client::GatewayClient;
use crate::fanout;
use crate::lua::{LuaFanoutResult, ToolBindings, resolve_section_target};
use crate::model::ModelBindings;
use crate::observe::detail;
use crate::parser::Section;
use crate::{Error, Result};
use mlua::Value as LuaValue;

use super::block_walk::{BlockRunMode, SectionFlow};
use super::context::RunContext;
use super::scope::ToolAnalysis;
use super::section_context::SectionContext;
use super::support::{
    GENERIC_COMPLETION, MAX_EXECUTE_DEPTH, bridge_blocking, next_id, now_rfc3339_checked,
};

/// How one chain ended.
enum WalkEnd {
    /// The level ran off its last section, carrying the final reply produced
    /// within it (if any).
    Exhausted(Option<String>),
    /// A scalar early return ended the chain (the run, for the top-level
    /// chain; the call's return value, for an `execute()` chain or a fanout
    /// arm's jump-started child walk).
    Returned(String),
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
pub(super) async fn run_sections(
    ctx: &RunContext,
    bindings: &ToolBindings,
    models: &ModelBindings,
    analysis: &ToolAnalysis,
    initial_var: Option<&serde_json::Value>,
    client: Option<&GatewayClient>,
) -> Result<String> {
    let when = now_rfc3339_checked()?;
    // The walk's context: the run-scoped values carry over; the walk-scoped
    // fields take their live values now that H1 produced them.
    let ctx = ctx.with_walk_state(bindings, models, analysis, &when);

    // The walk owns its client slot: seeded from the run's client (if any),
    // created lazily on first prose, and shared by every section.
    let mut client = client.cloned();
    // The walk owns its `var`: seeded from H1's hand-off, rolled forward
    // across sections, and shared with jump-started child walks.
    let mut var = initial_var
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    match walk_siblings(
        &ctx,
        &ctx.prompt().sections,
        0,
        None,
        false,
        0,
        &mut client,
        &mut var,
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
/// heading recurses into the jumper's children, an `execute()` call runs a
/// contained chain over the target's slice, and a fanout arm's jump drives a
/// child walk from the arm's driver boundary - all under the same rules
/// (fall-through order, off-walk skips, the reply rolling forward across
/// sections). When a jump-started sub-walk's level exhausts, the parent
/// chain resumes after the jumper with the sub-walk's last reply. A scalar
/// return ends the chain it fires in: it propagates out of every sub-walk to
/// the chain's root, where the top-level chain turns it into the run's
/// result and an `execute()` chain into the call's return value.
/// `start_addressed` is true when the chain begins on an addressed target (a
/// jump or execute target), which runs even when marked off-walk.
/// `execute_depth` is the chain's nesting depth, threaded into every section
/// so nested `execute()` calls stay under the cap. Each section entry takes
/// the next id from the context's run-global counter. `var` is the
/// walk's clipboard: each section's VM seeds from it, the section's final
/// `var` replaces it, and a jump-started child walk shares the same value.
#[expect(
    clippy::too_many_arguments,
    reason = "the chain keeps its shared context, position, entry mode, depth, and walk-owned var explicit and linear"
)]
async fn walk_siblings(
    ctx: &RunContext,
    siblings: &[Section],
    start: usize,
    incoming_reply: Option<String>,
    start_addressed: bool,
    execute_depth: usize,
    client: &mut Option<GatewayClient>,
    var: &mut serde_json::Value,
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
        let (flow, final_var) = run_one_section(
            ctx,
            section,
            siblings,
            next_id(ctx.ids()),
            execute_depth,
            reply.as_deref(),
            client,
            var,
        )
        .await?;
        *var = final_var;
        match flow {
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
                        var,
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

/// Execute one section's block lifecycle over the shared [`RunContext`].
///
/// This is the single engine every chain drives: it constructs the
/// section's [`SectionContext`] (VM construction and limits, the control
/// globals, the setup half, the infer hook), runs the block walk on the
/// frame, then owns the teardown boundary and the section-finished
/// observation.
/// `siblings` is the caller's
/// own walk slice, from which the section's visible set (its siblings minus
/// itself, plus its direct children) is built for the control globals.
/// `section_id` is the section's `sys.id`: the next value from the
/// run-global counter. `incoming_reply` is the
/// model reply visible to this section's first prose; the engine rolls it
/// forward as later prose in the same section produces text. `var` is the
/// walk's current clipboard, seeded into the section's VM; the section's
/// final `var` is read back before teardown and returned alongside the
/// [`SectionFlow`] so the walk rolls it forward.
#[expect(
    clippy::too_many_arguments,
    reason = "the engine keeps the shared context, the section's chain position, its depth, and the walk's var explicit and linear"
)]
async fn run_one_section(
    ctx: &RunContext,
    section: &Section,
    siblings: &[Section],
    section_id: u64,
    execute_depth: usize,
    incoming_reply: Option<&str>,
    client: &mut Option<GatewayClient>,
    var: &serde_json::Value,
) -> Result<(SectionFlow, serde_json::Value)> {
    let mut section_frame = SectionContext::new(
        ctx,
        section,
        siblings,
        section_id,
        execute_depth,
        incoming_reply,
        client,
        var,
    )?;

    // The walk half - the ordered block loop - reports how the section
    // ended. The teardown boundary stays here: every path out of the walk
    // tears the VM down exactly once, and SECTION_FINISHED fires only when
    // the walk completed (a jump or return included), never on an error.
    let flow = match section_frame
        .run(
            ctx,
            &section.name,
            &section.blocks,
            BlockRunMode::Section,
            client,
        )
        .await
    {
        Ok(flow) => flow,
        Err(error) => {
            section_frame.teardown(&section.name);
            return Err(error);
        }
    };
    // Read the section's final var back before teardown so the walk rolls it
    // forward; the write guard keeps this conversion from failing in
    // practice.
    let final_var = section_frame.read_var();
    section_frame.teardown(&section.name);
    let final_var = final_var?;
    ctx.observer()
        .observe(ctx.execution(), &section.name, detail::SECTION_FINISHED);
    Ok((flow, final_var))
}

/// The index of `target` in `slice`, matched on the parser-unique
/// `(level, name)` pair; `None` when the slice does not contain it.
fn section_position(slice: &[Section], target: &Section) -> Option<usize> {
    slice
        .iter()
        .position(|s| s.level == target.level && s.name == target.name)
}

/// The caller's home slice minus the caller itself, the caller found by its
/// parser-unique `(level, name)` pair and excluded by index.
///
/// A caller that is not in the slice excludes nothing: that is the fanout
/// arm's case, whose home slice is the worker's resolution set with the
/// worker already removed (built in [`make_fanout_callback`]), so the arm's
/// visible set comes out as exactly the home slice plus the worker's
/// children (pinned by the arm control-global tests in
/// `execute/tests/exec_flow.rs`).
fn home_without(home: &[Section], caller: &Section) -> Vec<Section> {
    let caller_index = section_position(home, caller);
    home.iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != caller_index)
        .map(|(_, section)| section.clone())
        .collect()
}

/// The sections a running section may address by heading: the caller's own
/// home slice minus the caller itself, plus the caller's direct children.
///
/// The parent, aunts/uncles, nieces/nephews, and grandchildren are never in
/// the set, so a resolution error that lists the set cannot leak the rest of
/// the document's structure.
fn visible_sections(home: &[Section], caller: &Section) -> Vec<Section> {
    home_without(home, caller)
        .into_iter()
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
pub(super) enum JumpTarget {
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

/// Drives a contained chain from an execute/jump target: resolves the
/// target over the caller's visible set, walks the target's chain slice
/// from its index under every normal rule (each entry taking the next
/// run-global `sys.id`), and maps the chain's end to its text - a
/// return's value,
/// else the final reply. The chain seeds its `var` from `var` (a clone
/// of the caller's, taken at the call site) and discards it when the
/// chain ends, so the caller never sees the chain's writes. Both chain
/// drivers call this one helper: the `execute` callback bridges it
/// synchronously, the fanout arm's jump awaits it directly.
#[expect(
    clippy::too_many_arguments,
    reason = "the chain drive keeps the caller, home slice, target, args, reply, depth, client, and var clone explicit and linear beside the shared run context"
)]
pub(crate) async fn drive_contained_chain(
    ctx: &RunContext,
    caller: &Section,
    home: &[Section],
    heading: &str,
    args: &str,
    incoming_reply: Option<String>,
    execute_depth: usize,
    client: &mut Option<GatewayClient>,
    var: serde_json::Value,
) -> Result<String> {
    // The contained chain runs the target's own slice from the target's
    // index: a child target sits beside the caller's children, a sibling
    // target beside the caller's home slice.
    let (chain_slice, start) = match resolve_jump_target(heading, home, caller)? {
        JumpTarget::Child(index) => (caller.children.as_slice(), index),
        JumpTarget::Sibling(index) => (home, index),
    };
    // The chain sees the call's args: an `execute` call's explicit input
    // overrides the run's own.
    let ctx = ctx.with_args(args);
    let mut var = var;
    let end = walk_siblings(
        &ctx,
        chain_slice,
        start,
        incoming_reply,
        true,
        execute_depth,
        client,
        &mut var,
    )
    .await?;
    // A return ends the chain, and its value is the call's return; an
    // exhausted chain returns its final reply.
    Ok(match end {
        WalkEnd::Returned(value) => value,
        WalkEnd::Exhausted(reply) => reply.unwrap_or_default(),
    })
}

/// The one recursion-cap boundary check, shared by the `execute` and `fanout`
/// callbacks: `op` at `depth` past [`MAX_EXECUTE_DEPTH`] errors.
fn check_execute_depth(depth: usize, op: &str) -> Result<()> {
    if depth > MAX_EXECUTE_DEPTH {
        return Err(Error::Lua(format!(
            "{op} recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
        )));
    }
    Ok(())
}

/// Builds the three control-global callbacks every engine driver installs on
/// a section VM: `execute`, `fanout`, and `list_from_section`.
///
/// `caller` is the section the globals serve (a walked section, or a fanout
/// arm's worker); `home` is the slice its `execute` chain slices into - the
/// caller's own walk slice for the section walk, or the worker's home slice
/// (its resolution set minus the worker) for an arm. The resolution set for
/// all three globals is derived here as the home slice minus the caller, plus
/// the caller's direct children (see [`visible_sections`]). `client` is the
/// driver's client snapshot at install time: each nested chain starts from
/// it, creating one lazily when absent. `execute_depth` is the caller's
/// nesting depth: an `execute` chain runs one level deeper, and a `fanout`
/// passes it through so each arm runs one level deeper, keeping
/// [`MAX_EXECUTE_DEPTH`] the only recursion constraint across both
/// boundaries. `last_reply` seeds each arm's reply roll-forward. The
/// `execute` and
/// `fanout` closures receive the caller VM's `var` as a JSON snapshot taken
/// at the call site (see [`SectionVm::install_control_globals`]): an
/// `execute` chain seeds from that clone and discards it, and each fanout
/// arm seeds from its own copy. The closures may outlive the driver that
/// built them, so they capture owned clones of the run context (cheap -
/// every field is shared ownership) rather than borrows.
#[expect(
    clippy::type_complexity,
    reason = "the triple of anonymous control-global closures is the product; a named struct cannot hold them without type_alias_impl_trait, and boxing would allocate per VM install"
)]
#[expect(
    clippy::ref_option,
    reason = "the client snapshot is cloned into the returned 'static closures, so the parameter must borrow the Option itself"
)]
pub(crate) fn make_control_globals(
    ctx: &RunContext,
    client: &Option<GatewayClient>,
    caller: Section,
    home: Vec<Section>,
    execute_depth: usize,
    last_reply: Option<String>,
) -> (
    impl Fn(LuaValue, Option<String>, serde_json::Value) -> std::result::Result<String, Error>
    + Send
    + use<>,
    impl Fn(
        String,
        Vec<serde_json::Value>,
        serde_json::Value,
    ) -> std::result::Result<Vec<LuaFanoutResult>, Error>
    + Send
    + use<>,
    impl Fn(String) -> std::result::Result<Vec<String>, Error> + Send + use<>,
) {
    let visible = visible_sections(&home, &caller);
    let execute_callback = {
        let ctx = ctx.clone();
        let client = client.clone();
        move |target: LuaValue,
              input: Option<String>,
              var: serde_json::Value|
              -> std::result::Result<String, Error> {
            let heading = resolve_section_target(target).map_err(Error::lua)?;
            // Each execute chain runs one level deeper than its caller.
            let next_depth = execute_depth + 1;
            check_execute_depth(next_depth, "execute")?;
            let call_args = input.as_deref().unwrap_or_else(|| ctx.args());
            let mut client = client.clone();
            // Return the structured error directly (LUA-012): the typed error
            // and its source cross the Lua boundary via `mlua::Error::external`
            // rather than being flattened to a string here.
            bridge_blocking(drive_contained_chain(
                &ctx,
                &caller,
                &home,
                &heading,
                call_args,
                None,
                next_depth,
                &mut client,
                var,
            ))
        }
    };
    // The caller's visible set is built once and shared: the fanout callback
    // takes a clone, the list callback takes the original.
    let fanout_callback = {
        let ctx = ctx.clone();
        let client = client.clone();
        let visible = visible.clone();
        move |worker_heading: String, items: Vec<serde_json::Value>, var: serde_json::Value| {
            make_fanout_callback(
                &worker_heading,
                &items,
                &var,
                &visible,
                &ctx,
                client.as_ref(),
                last_reply.as_deref(),
                execute_depth,
            )
        }
    };
    let list_callback = move |heading: String| -> std::result::Result<Vec<String>, Error> {
        list_items_from_visible(&heading, &visible)
    };
    (execute_callback, fanout_callback, list_callback)
}

/// Runs one `fanout(worker, collection)` call: resolves the worker over the
/// caller's visible set, checks the recursion cap, and schedules the arms.
///
/// # Errors
/// Returns [`Error::Lua`] when the fanout caller sits at
/// [`MAX_EXECUTE_DEPTH`] (each arm runs one level deeper, so the cap trips at
/// the boundary), when the heading resolves to nothing or to more than one
/// section (see [`fanout::resolve_sibling`]), or when the resolved section is
/// a list section rather than a worker template.
#[expect(
    clippy::too_many_arguments,
    reason = "the fanout callback threads the resolution set, the run context, and the client snapshot through to the arm scheduler as one linear parameter list"
)]
fn make_fanout_callback(
    worker_heading: &str,
    items: &[serde_json::Value],
    caller_var: &serde_json::Value,
    visible: &[Section],
    ctx: &RunContext,
    client: Option<&GatewayClient>,
    last_reply: Option<&str>,
    execute_depth: usize,
) -> std::result::Result<Vec<LuaFanoutResult>, Error> {
    // Each arm runs one execute level deeper than the fanout caller, so
    // recursion accounting accumulates across the fanout boundary instead of
    // resetting, and MAX_EXECUTE_DEPTH stays the only recursion constraint.
    check_execute_depth(execute_depth + 1, "fanout")?;
    let worker = fanout::resolve_sibling(worker_heading, visible)?;
    if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items.is_empty() {
        return Err(Error::Lua(format!(
            "section `{}` is a list section, not a worker template",
            worker.name
        )));
    }

    // The worker's home slice - the set it was resolved from, minus the
    // worker - is threaded to the arms as constructed here; each arm's
    // control globals derive their resolution set from it (the home slice
    // plus the worker's children), so the arm never inverts this layout.
    let worker_home = home_without(visible, worker);

    // The collection was converted to JSON member-by-member at the Lua
    // boundary (the same bridge `var` uses); the cap inside counts members.
    bridge_blocking(fanout::run_fanout_arms(
        ctx,
        worker,
        items,
        client,
        last_reply,
        &worker_home,
        execute_depth,
        caller_var,
    ))
}
