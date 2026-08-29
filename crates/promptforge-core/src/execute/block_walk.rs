//! The engine's prose paths: one per driver mode.
//!
//! [`run_section_prose`] runs one walked section's prose block (the
//! per-block scope rebuild, the one-time counts and model install,
//! substitution, the tool loop, and the reply/`sys` roll-forward);
//! [`run_live_h1_prose`] runs one live H1 prose block (substitution, the
//! empty-prose skip, the default model and the always-scope read from the
//! bindings-so-far, fresh per-block counts, and the reply written as a
//! plain global). The scheduler's driver calls both through the frame
//! ([`SectionContext`](super::section_context::SectionContext)); each takes
//! the borrowed [`RunContext`] plus the frame's own state borrows.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU32;

use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::lua::{SectionVm, ToolBinding, ToolCallCounts, current_tool_bindings};
use crate::model::CompletionOptions;
use crate::observe::Observer;
use crate::subst;
use crate::tools::ToolId;
use crate::{Error, Result};

use super::context::RunContext;
use super::gateway::env_client_with_limits;
use super::scope::{prepare_effective_scope, prepare_scoped_tools};
use super::tool_loop::{ProseMode, run_prose_inference};

/// Runs one live H1 prose block: substitution against the rolling reply,
/// the empty-prose skip, the default model and the always-scope read from
/// the bindings-so-far (through the run's views, which H1's binds write),
/// fresh per-block tool-call counts, and the reply written back as a plain
/// global - no `sys` enrichment, no local dispatch, no global aliases.
///
/// This is the live H1 prose path; the scheduler's driver runs it through
/// the frame's `run_live_h1_prose_block`.
///
/// # Errors
/// Returns the [`Error`] of whichever step failed: prose substitution, the
/// scope build, lazy client construction, the tool loop, or the reply
/// write. Returns [`Error::ModelRequired`] when non-empty prose has no
/// resolved default model, and [`Error::Internal`] when prose reaches
/// inference with no gateway client.
#[expect(
    clippy::too_many_arguments,
    reason = "one prose block keeps the VM, the shared run context, the frame's borrowed state, the effective reporting handles, and the client slot explicit and linear"
)]
pub(crate) async fn run_live_h1_prose(
    vm: &SectionVm,
    ctx: &RunContext,
    name: &str,
    text: &str,
    prose_mode: ProseMode,
    sys: &serde_json::Value,
    reply: &mut Option<String>,
    conversation: &mut Vec<Message>,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    turns: &AtomicU32,
    client: &mut Option<GatewayClient>,
) -> Result<()> {
    let var = vm.var()?;
    let prose = subst::substitute(
        text,
        ctx.args(),
        reply.as_deref(),
        // The live H1 pass is never a fanout arm, so there is no item.
        None,
        &var,
        sys,
        &|name| vm.global_json(name).map_err(Error::from),
    )?;
    if prose.trim().is_empty() {
        return Ok(());
    }
    // The default model reads the bindings-so-far through the run's model
    // view: H1's binds write the same shared set the view reads.
    let model = match ctx.models().default()? {
        Some(alias) => ctx.models().binding(&alias)?,
        None => None,
    };
    let Some(model) = model else {
        return Err(Error::ModelRequired {
            section: name.to_owned(),
        });
    };
    // The always-scope reads the bindings-so-far through the run's tool
    // view: H1's binds write the same shared set the view reads.
    let mut scope: Vec<ToolBinding> = Vec::new();
    for alias in ctx.tools().always()? {
        if let Some(binding) = ctx.tools().binding(&alias)? {
            scope.push(binding);
        }
    }
    // Live H1 registers no local tools; the list is always empty here.
    let local_schemas = vm.local_tool_schemas()?;
    let (schemas, dispatch) = prepare_scoped_tools(&scope, &local_schemas)?;
    if client.is_none() {
        *client = Some(env_client_with_limits(ctx.limits())?);
    }
    let Some(active_client) = client.as_ref() else {
        // The block just above guarantees a client exists here; a `None` is
        // an internal invariant violation, not a reason to silently skip
        // this prose.
        return Err(Error::Internal(
            "model-facing prose reached inference with no gateway client",
        ));
    };
    // Live H1 prose counts tool calls into a fresh per-block set; there is
    // no cross-block counts gate here.
    let block_counts = ToolCallCounts::new(scope.iter().map(|binding| binding.alias().to_owned()));
    let model_options = model.completion_options();
    let outcome = run_prose_inference(
        active_client,
        &schemas,
        &dispatch,
        conversation,
        prose,
        prose_mode,
        ctx.execution(),
        observer,
        name,
        turns,
        debug,
        &model_options,
        ctx.nonce(),
        Some(&block_counts),
        // Live H1 has no prompt-wide alias analysis and no local tools to
        // dispatch.
        None,
        None,
    )
    .await?;
    if let Some(text) = outcome.text {
        vm.set_global_string("reply", &text)?;
        *reply = Some(text);
    }
    Ok(())
}

/// Runs one section-mode prose block: the per-block scope rebuild, the
/// one-time counts and model install, substitution, the tool loop, and the
/// reply/`sys` roll-forward.
///
/// This is the section prose path; the scheduler's driver runs it through
/// the frame's `run_prose_block`. An
/// empty substituted prose skips inference after the scope rebuild. `counts` doubles as the one-time gate: it
/// is `Some` exactly after the first prose block installed the counts and
/// resolved the model.
///
/// # Errors
/// Returns the [`Error`] of whichever step failed: the tool-scope rebuild,
/// prose substitution, lazy client construction, the tool loop, or a
/// `sys`/reply re-seal. Returns [`Error::ModelRequired`] when model-facing
/// prose has no resolved model binding, and [`Error::Internal`] when prose
/// reaches inference with no gateway client.
#[expect(
    clippy::too_many_arguments,
    reason = "one prose block keeps the VM, the shared run context, the frame's borrowed state, the effective reporting handles, and the client slot explicit and linear"
)]
pub(crate) async fn run_section_prose(
    vm: &SectionVm,
    ctx: &RunContext,
    name: &str,
    text: &str,
    prose_mode: ProseMode,
    sys: &mut serde_json::Value,
    reply: &mut Option<String>,
    conversation: &mut Vec<Message>,
    counts: &mut Option<ToolCallCounts>,
    completion_options: &mut Option<CompletionOptions>,
    item: Option<&serde_json::Value>,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
    turns: &AtomicU32,
    client: &mut Option<GatewayClient>,
) -> Result<()> {
    let tool_set = ctx.tool_set_snapshot()?;
    let effective_bindings = current_tool_bindings(&tool_set, &vm.tool_runtime)?;
    if counts.is_none() {
        *counts = Some(vm.install_tool_call_counts(&effective_bindings)?);
        let resolved_model = crate::lua::resolve_model_binding(ctx.models(), &vm.model_runtime)?;
        if let Some(binding) = resolved_model.as_ref() {
            let current = vm.current_sys(sys)?;
            let enriched = crate::lua::enrich_sys_model(&current, binding);
            vm.re_seal_sys(&enriched)?;
            *sys = enriched;
            *completion_options = Some(binding.completion_options());
        }
    }
    let local_schemas = vm.local_tool_schemas()?;
    // Seed aliases added since the first prose block (via `tools.add` or
    // `tools.add_local`) so the tool loop can count their calls; `ensure`
    // is idempotent on existing aliases.
    if let Some(counts) = counts.as_ref() {
        let new_aliases = effective_bindings
            .iter()
            .map(ToolBinding::alias)
            .chain(local_schemas.iter().map(|schema| schema.name.as_str()));
        for alias in new_aliases {
            counts.ensure(alias)?;
        }
    }
    let (schemas, dispatch) = prepare_effective_scope(
        &effective_bindings,
        &local_schemas,
        ctx.execution(),
        observer,
        name,
    )?;

    let var = vm.var()?;
    let prose = subst::substitute(
        text,
        ctx.args(),
        reply.as_deref(),
        item,
        &var,
        sys,
        &|name| vm.global_json(name).map_err(Error::from),
    )?;
    if prose.trim().is_empty() {
        return Ok(());
    }
    let Some(options) = completion_options.as_ref() else {
        return Err(Error::ModelRequired {
            section: name.to_owned(),
        });
    };
    if client.is_none() {
        *client = Some(env_client_with_limits(ctx.limits())?);
    }
    let Some(active_client) = client.as_ref() else {
        // The block just above guarantees a client exists here; a `None` is
        // an internal invariant violation, not a reason to silently skip
        // this section's prose.
        return Err(Error::Internal(
            "model-facing prose reached inference with no gateway client",
        ));
    };
    // The alias map is derived on demand from the bindings list; the
    // analysis type that precomputed it at the H1 boundary is gone.
    let global_aliases: BTreeMap<String, ToolId> = tool_set
        .bindings()
        .iter()
        .map(|binding| (binding.alias().to_owned(), binding.id().clone()))
        .collect();
    let global_aliases = Some(&global_aliases);
    // Local tools are Lua functions on this section VM; route their calls
    // back into it rather than to a bound tool.
    let local_dispatch = |alias: &str, args: serde_json::Value| {
        vm.call_local_tool(alias, &args).map_err(Error::from)
    };
    let outcome = run_prose_inference(
        active_client,
        &schemas,
        &dispatch,
        conversation,
        prose,
        prose_mode,
        ctx.execution(),
        observer,
        name,
        turns,
        debug,
        options,
        ctx.nonce(),
        counts.as_ref(),
        global_aliases,
        Some(&local_dispatch),
    )
    .await?;
    *sys = crate::lua::enrich_sys_reply_finish_reason(sys, outcome.finish_reason.as_deref());
    vm.re_seal_sys(sys)?;
    if let Some(text) = outcome.text {
        vm.bind_reply(&text, observer, name)?;
        *reply = Some(text);
    }
    Ok(())
}
