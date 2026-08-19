//! The engine's walk half: one section's ordered block loop.
//!
//! [`walk_section_blocks`] runs the block lifecycle every driver of the
//! shared engine shares - Lua chunks in place, prose through the tool loop -
//! and returns a [`SectionFlow`] telling the driver how the section ended.
//! The driver owns everything around the loop: VM construction and limits,
//! the setup half (`section_vm`), the infer hook, and the teardown boundary,
//! so a walk error propagates bare and the driver tears the VM down exactly
//! once on every path.

use crate::client::{GatewayClient, Message};
use crate::lua::{LuaBlockResult, SectionVm, ToolBinding, ToolCallCounts, current_tool_bindings};
use crate::model::CompletionOptions;
use crate::parser::{Block, Section};
use crate::subst;
use crate::{Error, Result};

use super::engine::WalkContext;
use super::gateway::env_client_with_limits;
use super::scope::prepare_effective_scope;
use super::tool_loop::{ProseMode, SectionProgress, run_prose_inference};

/// How one section's block walk ended.
pub(super) enum SectionFlow {
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

/// Walks one section's ordered blocks on its prepared VM and reports how the
/// section ended.
///
/// This is the engine's walk half, the per-block loop every driver of the
/// shared engine runs. A Lua chunk executes in place and may end the walk
/// early (a scalar return or a `jump`); a prose block rebuilds its effective
/// tool scope (so `tools.add`/`tools.add_local` between blocks reach the
/// next model turn), substitutes against the rolling reply, and runs the
/// tool loop. The conversation, the tool-call counts, and the one-time model
/// resolution (with its `sys` model enrichment) are walk-local state
/// installed at the first prose block. The reply starts at `incoming_reply`
/// and rolls forward as prose produces text; `sys` arrives as the driver's
/// JSON and is enriched in place with the model binding and each outcome's
/// finish reason.
///
/// The caller owns the teardown boundary: an error propagates without
/// tearing the VM down, so the driver's single teardown covers every path.
///
/// # Errors
/// Returns the [`Error`] of whichever step failed: a Lua chunk, the
/// tool-scope rebuild, prose substitution, lazy client construction, the
/// tool loop, or a `sys`/reply re-seal. Returns [`Error::ModelRequired`]
/// when model-facing prose has no resolved model binding, and
/// [`Error::Internal`] when prose reaches inference with no gateway client.
#[expect(
    clippy::too_many_lines,
    reason = "one linear block loop: per-block scope rebuild, prose substitution, the tool loop, and the reply roll-forward stay together so every driver shares exactly one implementation"
)]
pub(super) async fn walk_section_blocks(
    vm: &mut SectionVm,
    ctx: &WalkContext<'_>,
    section: &Section,
    mut sys: serde_json::Value,
    incoming_reply: Option<&str>,
    client: &mut Option<GatewayClient>,
) -> Result<SectionFlow> {
    // Walk-only state: the registry is read only inside the block walk.
    let registry = ctx.shared_tools.registry();
    let mut conversation: Vec<Message> = Vec::new();
    // `counts` doubles as the one-time gate: it is `Some` exactly after the
    // first prose block installed the counts and resolved the model. Schemas
    // and dispatch rebuild on EVERY prose block so `tools.add`/
    // `tools.add_local` between blocks reach the next model turn.
    let mut counts: Option<ToolCallCounts> = None;
    // Set at the first prose block exactly when a model binding resolved, so
    // its `None` check below is the one model-required gate.
    let mut completion_options: Option<CompletionOptions> = None;
    // The reply visible to this section's prose. It starts at the incoming
    // reply and rolls forward as prose produces text, so both the `{{reply}}`
    // substitution and the Lua `reply` global stay consistent within a section.
    let mut reply: Option<String> = incoming_reply.map(str::to_owned);

    for block in &section.blocks {
        match block {
            Block::Lua(program) => match vm.run_chunk(program, ctx.observer, &section.name)? {
                LuaBlockResult::Returned(Some(value)) => {
                    return Ok(SectionFlow::Returned(value));
                }
                LuaBlockResult::Returned(None) => {}
                LuaBlockResult::Jump(heading) => {
                    return Ok(SectionFlow::Jumped { heading, reply });
                }
            },
            Block::Prose { text, loop_capable } => {
                let effective_bindings = current_tool_bindings(ctx.bindings, &vm.tool_runtime)?;
                if counts.is_none() {
                    counts = Some(vm.install_tool_call_counts(&effective_bindings)?);
                    let resolved_model =
                        crate::lua::resolve_model_binding(ctx.models, &vm.model_runtime)?;
                    if let Some(binding) = resolved_model.as_ref() {
                        let current = vm.current_sys(&sys)?;
                        let enriched = crate::lua::enrich_sys_model(&current, binding);
                        vm.re_seal_sys(&enriched)?;
                        sys = enriched;
                        completion_options = Some(binding.completion_options());
                    }
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
                        counts.ensure(alias)?;
                    }
                }
                let (schemas, dispatch) = prepare_effective_scope(
                    ctx.analysis,
                    &effective_bindings,
                    &local_schemas,
                    &registry,
                    ctx.execution,
                    ctx.observer,
                    &section.name,
                )?;

                let var = vm.var()?;
                let prose = subst::substitute(text, ctx.args, reply.as_deref(), None, &var, &sys)?;
                if prose.trim().is_empty() {
                    continue;
                }
                let Some(options) = completion_options.as_ref() else {
                    return Err(Error::ModelRequired {
                        section: section.name.clone(),
                    });
                };
                if client.is_none() {
                    *client = Some(env_client_with_limits(ctx.limits)?);
                }
                let Some(active_client) = client.as_ref() else {
                    // The block just above guarantees a client exists here; a
                    // `None` is an internal invariant violation, not a reason to
                    // silently skip this section's prose.
                    return Err(Error::Internal(
                        "model-facing prose reached inference with no gateway client",
                    ));
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
                let outcome = run_prose_inference(
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
                .await?;
                sys = crate::lua::enrich_sys_reply_finish_reason(
                    &sys,
                    outcome.finish_reason.as_deref(),
                );
                vm.re_seal_sys(&sys)?;
                if let Some(text) = outcome.text {
                    vm.bind_reply(&text, ctx.observer, &section.name)?;
                    reply = Some(text);
                }
            }
        }
    }

    Ok(SectionFlow::FellThrough { reply })
}
