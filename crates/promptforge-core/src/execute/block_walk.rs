//! The engine's walk half: one ordered block loop.
//!
//! [`run_one_section_impl`] runs the block lifecycle every driver of the
//! shared engine shares - Lua chunks in place, prose through the tool loop -
//! and returns a [`SectionFlow`] telling the driver how the walk ended.
//! The driver owns everything around the loop: VM construction and limits,
//! the setup half (`section_vm`), the infer hook, and the teardown boundary,
//! so a walk error propagates bare and the driver tears the VM down exactly
//! once on every path. Each driver (the live H1 pass, the section walk's
//! `run_one_section`, the fanout arm) hands the loop the same borrowed
//! [`RunFrame`] plus its per-frame state borrows and a [`BlockRunMode`]:
//! H1-vs-section is a caller-set mode,
//! not a second loop. Live mode reads only the frame's run-scoped fields;
//! the walk-only fields it never touches carry empty defaults.

use std::sync::atomic::AtomicU32;

use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::lua::{LuaBlockResult, SectionVm, ToolBinding, ToolCallCounts, current_tool_bindings};
use crate::model::CompletionOptions;
use crate::observe::Observer;
use crate::parser::Block;
use crate::resolve::RuntimeResolution;
use crate::subst;
use crate::{Error, Result};

use super::engine::RunFrame;
use super::gateway::env_client_with_limits;
use super::scope::{prepare_effective_scope, prepare_scoped_tools};
use super::tool_loop::{ProseMode, run_prose_inference};

/// How one section's block walk ended.
pub(crate) enum SectionFlow {
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

/// Which driver the shared block loop is serving.
///
/// H1-vs-section is a caller-set mode, not a second loop: the ordered walk,
/// the lazy client, and the reply roll-forward are shared, and each mode
/// keeps its own block handling exactly where the two drivers differ.
#[derive(Clone, Copy)]
pub(crate) enum BlockRunMode<'a> {
    /// The live H1 pass. Lua blocks run through
    /// [`SectionVm::run_live_h1_block`] with capability resolvers scoped to
    /// each block; prose binds the default model from the live producer's
    /// bindings-so-far, prepares the `always` scope without analysis, counts
    /// tool calls into a fresh per-block [`ToolCallCounts`], and writes the
    /// reply as a plain global - no `sys` enrichment, no local dispatch, no
    /// global aliases.
    LiveH1(&'a RuntimeResolution<'a, 'a>),
    /// An H2 section: the top-level walk, an `execute` chain, or a fanout
    /// arm.
    Section,
}

/// Walks one ordered block sequence on its prepared VM and reports how the
/// walk ended.
///
/// This is the engine's walk half, the per-block loop every driver of the
/// shared engine runs. `name`/`blocks` are the driver's heading and block
/// sequence (a section's, or the prompt's title and H1 blocks for the live
/// H1 pass); `mode` selects the per-block behavior (see [`BlockRunMode`]).
/// The per-frame state arrives as borrows out of the driver's frame: `sys`,
/// `reply`, `conversation`, `counts`, and `completion_options` are the
/// frame's slots (the driver's `SectionContext` fields), and
/// `observer`/`debug`/`turns` are the frame's effective reporting handles.
///
/// In section mode a Lua chunk executes in place and may end the walk early
/// (a scalar return or a `jump`); a prose block rebuilds its effective tool
/// scope (so `tools.add`/`tools.add_local` between blocks reach the next
/// model turn), substitutes against the rolling reply, and runs the tool
/// loop. The conversation, the tool-call counts, and the one-time model
/// resolution (with its `sys` model enrichment) are installed at the first
/// prose block. The reply starts at the driver's seed
/// and rolls forward as prose produces text; after each Lua chunk the walk
/// reads the VM's `reply` global back, so an author's `reply = nil` (or a
/// custom string) steers what a jump target or the next section sees.
/// `sys` arrives as the driver's
/// JSON and is enriched in place with the model binding and each outcome's
/// finish reason. Prose substitution resolves `{{ item }}` against
/// `item`, which is `Some` only when the driver is a fanout arm, and an
/// unknown first path segment against the VM's bare globals via
/// [`SectionVm::global_json`].
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
    clippy::too_many_arguments,
    reason = "the loop keeps the VM, the shared frame, the block sequence, the mode, the frame's borrowed state, the effective reporting handles, and the client slot explicit and linear"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one linear block loop: per-block scope rebuild, prose substitution, the tool loop, and the reply roll-forward stay together so every driver shares exactly one implementation"
)]
pub(crate) async fn run_one_section_impl(
    vm: &mut SectionVm,
    ctx: &RunFrame<'_>,
    name: &str,
    blocks: &[Block],
    mode: BlockRunMode<'_>,
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
) -> Result<SectionFlow> {
    // Walk-only state: the registry is read only inside the block walk.
    let registry = ctx.shared_tools.registry();
    // Section mode: `counts` doubles as the one-time gate: it is `Some`
    // exactly after the first prose block installed the counts and resolved
    // the model. Schemas and dispatch rebuild on EVERY prose block so
    // `tools.add`/`tools.add_local` between blocks reach the next model turn.
    // `completion_options` is set at the first prose block exactly when a
    // model binding resolved, so its `None` check below is the one
    // model-required gate. The reply rolls forward as prose produces text,
    // and after each section-mode Lua chunk it is read back from the VM's
    // `reply` global, so both the `{{reply}}` substitution and the Lua
    // `reply` global stay consistent within a walk and an author's
    // `reply = nil` takes effect.
    for block in blocks {
        match block {
            Block::Lua(program) => match mode {
                // Live H1 Lua runs with call-time capability resolution
                // scoped to the block; `run_live_h1_block` already turns a
                // recorded jump into an error, so only a scalar return ends
                // the pass.
                BlockRunMode::LiveH1(runtime) => {
                    if let Some(value) = vm.run_live_h1_block(program, runtime, observer, name)? {
                        return Ok(SectionFlow::Returned(value));
                    }
                }
                BlockRunMode::Section => {
                    match vm.run_chunk(program, observer, name)? {
                        LuaBlockResult::Returned(Some(value)) => {
                            return Ok(SectionFlow::Returned(value));
                        }
                        // The `reply` global is the author-writable shadow of
                        // the walk-local reply: seeded at host injection and
                        // rebound after prose, so reading it back after each
                        // chunk honors an author's `reply = nil` (or a custom
                        // string) at fall-through and across a jump.
                        LuaBlockResult::Returned(None) => {
                            *reply = vm.reply()?;
                        }
                        LuaBlockResult::Jump(heading) => {
                            return Ok(SectionFlow::Jumped {
                                heading,
                                reply: vm.reply()?,
                            });
                        }
                    }
                }
            },
            Block::Prose { text, loop_capable } => {
                let prose_mode = if *loop_capable {
                    ProseMode::Loop {
                        max_tool_iterations: ctx.max_tool_iterations,
                    }
                } else {
                    ProseMode::SingleShot
                };
                match mode {
                    BlockRunMode::LiveH1(runtime) => {
                        let var = vm.var()?;
                        let prose = subst::substitute(
                            text,
                            ctx.args,
                            reply.as_deref(),
                            item,
                            &var,
                            sys,
                            &|name| vm.global_json(name),
                        )?;
                        if prose.trim().is_empty() {
                            continue;
                        }
                        let (tool_bindings, model_bindings) = runtime.bindings()?;
                        let Some(model) = model_bindings
                            .default()
                            .and_then(|alias| model_bindings.binding(alias))
                        else {
                            return Err(Error::ModelRequired {
                                section: name.to_owned(),
                            });
                        };
                        let mut scope: Vec<ToolBinding> = Vec::new();
                        for alias in tool_bindings.always() {
                            if let Some(binding) = tool_bindings
                                .bindings()
                                .iter()
                                .find(|binding| binding.alias() == alias)
                            {
                                scope.push(binding.clone());
                            }
                        }
                        // Live H1 registers no local tools; the list is always
                        // empty here.
                        let local_schemas = vm.local_tool_schemas()?;
                        let (schemas, dispatch) =
                            prepare_scoped_tools(&scope, &local_schemas, &registry)?;
                        if client.is_none() {
                            *client = Some(env_client_with_limits(ctx.limits)?);
                        }
                        let Some(active_client) = client.as_ref() else {
                            // The block just above guarantees a client exists
                            // here; a `None` is an internal invariant violation,
                            // not a reason to silently skip this prose.
                            return Err(Error::Internal(
                                "model-facing prose reached inference with no gateway client",
                            ));
                        };
                        // Live H1 prose counts tool calls into a fresh per-block
                        // set; there is no cross-block counts gate here.
                        let block_counts = ToolCallCounts::new(
                            scope.iter().map(|binding| binding.alias().to_owned()),
                        );
                        let model_options = model.completion_options();
                        let outcome = run_prose_inference(
                            active_client,
                            &schemas,
                            &dispatch,
                            &registry,
                            conversation,
                            prose,
                            prose_mode,
                            ctx.execution,
                            observer,
                            name,
                            turns,
                            debug,
                            &model_options,
                            ctx.nonce,
                            Some(&block_counts),
                            // Live H1 has no prompt-wide alias analysis and no
                            // local tools to dispatch.
                            None,
                            None,
                        )
                        .await?;
                        if let Some(text) = outcome.text {
                            vm.set_global_string("reply", &text)?;
                            *reply = Some(text);
                        }
                    }
                    BlockRunMode::Section => {
                        let effective_bindings =
                            current_tool_bindings(ctx.bindings, &vm.tool_runtime)?;
                        if counts.is_none() {
                            *counts = Some(vm.install_tool_call_counts(&effective_bindings)?);
                            let resolved_model =
                                crate::lua::resolve_model_binding(ctx.models, &vm.model_runtime)?;
                            if let Some(binding) = resolved_model.as_ref() {
                                let current = vm.current_sys(sys)?;
                                let enriched = crate::lua::enrich_sys_model(&current, binding);
                                vm.re_seal_sys(&enriched)?;
                                *sys = enriched;
                                *completion_options = Some(binding.completion_options());
                            }
                        }
                        let local_schemas = vm.local_tool_schemas()?;
                        // Seed aliases added since the first prose block (via
                        // `tools.add` or `tools.add_local`) so the tool loop can
                        // count their calls; `ensure` is idempotent on existing
                        // aliases.
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
                            observer,
                            name,
                        )?;

                        let var = vm.var()?;
                        let prose = subst::substitute(
                            text,
                            ctx.args,
                            reply.as_deref(),
                            item,
                            &var,
                            sys,
                            &|name| vm.global_json(name),
                        )?;
                        if prose.trim().is_empty() {
                            continue;
                        }
                        let Some(options) = completion_options.as_ref() else {
                            return Err(Error::ModelRequired {
                                section: name.to_owned(),
                            });
                        };
                        if client.is_none() {
                            *client = Some(env_client_with_limits(ctx.limits)?);
                        }
                        let Some(active_client) = client.as_ref() else {
                            // The block just above guarantees a client exists
                            // here; a `None` is an internal invariant violation,
                            // not a reason to silently skip this section's prose.
                            return Err(Error::Internal(
                                "model-facing prose reached inference with no gateway client",
                            ));
                        };
                        let global_aliases = Some(&ctx.analysis.alias_to_id);
                        // Local tools are Lua functions on this section VM; route
                        // their calls back into it rather than the registry.
                        let local_dispatch =
                            |alias: &str, args: serde_json::Value| vm.call_local_tool(alias, &args);
                        let outcome = run_prose_inference(
                            active_client,
                            &schemas,
                            &dispatch,
                            &registry,
                            conversation,
                            prose,
                            prose_mode,
                            ctx.execution,
                            observer,
                            name,
                            turns,
                            debug,
                            options,
                            ctx.nonce,
                            counts.as_ref(),
                            global_aliases,
                            Some(&local_dispatch),
                        )
                        .await?;
                        *sys = crate::lua::enrich_sys_reply_finish_reason(
                            sys,
                            outcome.finish_reason.as_deref(),
                        );
                        vm.re_seal_sys(sys)?;
                        if let Some(text) = outcome.text {
                            vm.bind_reply(&text, observer, name)?;
                            *reply = Some(text);
                        }
                    }
                }
            }
        }
    }

    Ok(SectionFlow::FellThrough {
        reply: reply.clone(),
    })
}
