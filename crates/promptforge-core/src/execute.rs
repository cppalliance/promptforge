//! Section lifecycle execution and fall-through.
//!
//! The run walks top-level sections in file order, creating one isolated
//! section VM for each. The VM is fully equipped (host values, store, log,
//! control globals) before the shared Lua library replays as the section's
//! first chunk, then ordered section blocks use that same VM. Lua before the first prose is
//! prologue-style; Lua after is epilog-style. Non-final prose is single-shot;
//! final prose runs the full tool loop. A scalar early Lua return ends the
//! section; a scalar late Lua return ends the run.
//!
//! Running off the last section ends the run: the result is the last model
//! reply, else a generic completion.
//!
//! The walk is level-independent and never descends on its own: a jump to a
//! child heading starts a child-level walk over the jumper's children under
//! the same rules, and the parent walk resumes after the jumper when that
//! level exhausts.
//!
//! One run-scoped [`StoreRef`] is created once by the caller and threaded through
//! every section (both its Lua prologue and, later, the model's file tools), so
//! bulk state persists across the context-clearing transitions even though a
//! section's conversation never does.
//!
//! A run reports itself as it goes: the [`RunConfig`] observer receives a
//! `(execution, section, event)` record when the run starts and ends, at each
//! section boundary, model turn, tool call, and harness-mediated store
//! operation. Reporting is a side channel and never
//! a decision, so passing [`crate::observe::NullObserver`] changes nothing but
//! the silence.
//!
//! Rust installs tool bindings captured from live H1 into each section VM.
//! Prompt-wide aliases and H2 additions form the effective model-visible scope,
//! which is checked for semantic near-duplicates before concrete tools are
//! advertised under their local aliases and dispatched by stable identity.
//!
//! Lua `execute()` starts a contained chain at a visible section (fresh VM,
//! fresh conversation, recursion capped at 8): the chain runs from the target
//! with every normal walk rule - fall-through, off-walk skips, jumps, child
//! chains - and the outer walk never moves while it runs. When the chain
//! ends (its level exhausts or a return fires), its final reply is the call's
//! return value; a return ends only the chain it fires in.
//! Lua `jump(target)` transfers control to a named section: the conversation
//! is cleared and the current reply carries across the jump.
//!
//! # Module layout
//!
//! The orchestration boundary ([`run`]) lives here; the rest is split into
//! focused private children: `error` (the public [`RunError`]), `config`
//! (`RunConfig`/`RunLimits`), `context` (the ambient [`RunContext`] run
//! state), `gateway` (client acquisition and [`ResolutionContext`]),
//! `scope` (tool-scope analysis and validation), `tools` (the
//! nested-inference hook), `tool_loop` (the model tool loop), `h1` (the
//! live H1 pass), `section_vm` (the section VM setup half shared by the
//! walk and the fanout arm), `section_context` (the per-section
//! [`SectionContext`](section_context::SectionContext) frame the walk's
//! driver constructs, runs, and tears down), `block_walk` (the
//! ordered block loop - the
//! engine's walk half, shared by the live H1 pass, the walk, and the
//! fanout arm), `engine` (the section walkers), and `support` (the
//! sync/async bridge and shared helpers).

mod block_walk;
mod config;
mod context;
mod engine;
mod error;
mod gateway;
mod h1;
mod scope;
mod section_context;
mod section_vm;
mod support;
mod tool_loop;
mod tools;

use std::sync::Arc;

// Public API surface.
pub use config::{RunConfig, RunLimits};
pub use error::{RunError, RunErrorKind};
pub use gateway::ResolutionContext;

// Crate-internal items reused through the historical `crate::execute::` path.
// `ToolAnalysis`, the engine's `drive_contained_chain`, and the
// `SectionContext` frame are consumed by `fanout`; `run_sections` and
// `execute_live_h1` serve only `run` below and stay module-private.
// Re-exported so the split stays surface-neutral for the public API while
// keeping one import path for internal collaborators.
pub(crate) use block_walk::{BlockRunMode, SectionFlow};
pub(crate) use context::RunContext;
pub(crate) use engine::drive_contained_chain;
pub(crate) use scope::ToolAnalysis;
pub(crate) use section_context::SectionContext;

use engine::run_sections;
use h1::execute_live_h1;

// Everything the executor's own tests reach through `use super::super::*`
// (and that `tests/mod.rs` does not itself import): executor-internal items,
// crate types, and the two external conveniences (`json`, `BTreeMap`).
// Test-only, so the non-test lib carries no unused re-export while the
// historic executor namespace stays intact for the test glob.
#[cfg(test)]
pub(crate) use crate::Result;
#[cfg(test)]
pub(crate) use crate::client::ToolSchema;
#[cfg(test)]
pub(crate) use crate::lua::{SectionVm, ToolCallCounts};
#[cfg(test)]
pub(crate) use crate::observe::Observer;
#[cfg(test)]
pub(crate) use gateway::GatewaySource;
#[cfg(test)]
pub(crate) use gateway::env_client_with_limits;
#[cfg(test)]
pub(crate) use scope::OwnedNearDuplicate;
#[cfg(test)]
pub(crate) use scope::{prepare_effective_scope, prepare_scoped_tools};
#[cfg(test)]
pub(crate) use serde_json::json;
#[cfg(test)]
pub(crate) use std::collections::BTreeMap;
#[cfg(test)]
pub(crate) use support::{advance_turn, bridge_blocking, now_rfc3339_checked};
#[cfg(test)]
pub(crate) use tool_loop::{LocalDispatch, ProseMode, run_prose_inference};

use crate::Error;
use crate::cancel;
use crate::observe::detail;
use crate::parser::{ParseErrorKind, Prompt};
use crate::store::StoreRef;
use crate::tools::{SharedTools, Tool};
use support::{GENERIC_COMPLETION, SUPPORTED_MAJOR};

// Re-exported for the executor test glob.
#[cfg(test)]
pub(crate) use crate::model::ModelBindings;

/// Executes a parsed prompt and returns its final text.
///
/// H1 Lua and prose blocks run once in source order with full host access;
/// capability calls resolve when executed. If H1 does not return, the H2 section
/// walk runs and its accumulated text is returned.
///
/// # Errors
/// Returns a [`RunError`] whose [`kind`](RunError::kind) classifies the failure
/// by condition:
/// - [`RunErrorKind::Parse`] - a prompt/frontmatter or compiled Lua region was
///   invalid.
/// - [`RunErrorKind::Version`] - the prompt declared an unsupported
///   `promptforge:` major.
/// - [`RunErrorKind::Binding`] - a `tools.bind`/`models.bind` capability could
///   not be bound, was absent, or clashed.
/// - [`RunErrorKind::Completion`] - a model completion failed at the transport,
///   backend, decode, or dialect layer.
/// - [`RunErrorKind::Tool`] - a dispatched tool failed, was out of scope, or the
///   tool loop did not converge.
/// - [`RunErrorKind::Lua`] - a section's Lua phase failed to run or return a
///   usable value.
/// - [`RunErrorKind::Quota`] - a Lua host resource quota (log events, log bytes,
///   or instructions) was exhausted.
/// - [`RunErrorKind::Substitution`] - a `{{ }}` prose substitution failed.
/// - [`RunErrorKind::Store`] - a run-scoped store operation failed.
/// - [`RunErrorKind::Cancelled`] - the host cancelled the run.
/// - [`RunErrorKind::Internal`] - an internal invariant failed (for example a
///   Lua host call reached on a current-thread runtime, which returns this
///   rather than panicking).
///
/// # Examples
/// A no-network, Lua-only prompt whose H1 block returns a value:
/// ```
/// use promptforge_core::execute::{run, RunConfig, ResolutionContext};
/// use promptforge_core::model::ModelCatalog;
/// use promptforge_core::observe::NullObserver;
/// use promptforge_core::parser::Prompt;
/// use promptforge_core::store::StoreRef;
/// use promptforge_tool_picker::{Catalog, Config, ToolPicker};
///
/// let source = concat!(
///     "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n",
///     "# Title\n\n",
///     "```lua\nreturn 'hello'\n```\n\n",
///     "## Only\n\ndone\n",
/// );
/// let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
/// let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())?;
/// let models = ModelCatalog::empty();
///
/// let runtime = tokio::runtime::Builder::new_current_thread().build()?;
/// let output = runtime.block_on(run(
///     &prompt,
///     "",
///     ResolutionContext::new(&picker, &models),
///     &[],
///     &StoreRef::memory(),
///     RunConfig::new("doc-example"),
/// ))?;
/// assert_eq!(output, "hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Runtime
/// Nested Lua host calls (`model:infer`, `execute`, `fanout`) bridge synchronous
/// Lua into async work via `tokio::task::block_in_place`, which requires the
/// multi-threaded Tokio runtime. Reaching such a call on a current-thread
/// runtime returns [`RunErrorKind::Internal`] rather than panicking (a prompt
/// with no nested host calls, like the example above, runs on either runtime).
pub async fn run(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
    tools: &[Arc<dyn Tool>],
    store: &StoreRef,
    config: RunConfig,
) -> std::result::Result<String, RunError> {
    match prompt.frontmatter.promptforge {
        Some(SUPPORTED_MAJOR) => {}
        Some(other) => return Err(RunError::from(Error::UnsupportedVersion(other))),
        None => {
            return Err(RunError::from(Error::parse(
                ParseErrorKind::Structure,
                "not a promptforge prompt: no promptforge version",
            )));
        }
    }

    let shared_tools =
        SharedTools::new(tools).map_err(|error| RunError::from(Error::from(error)))?;
    // Section startup replays the shared library unconditionally; a prompt
    // without one replays an empty compiled chunk instead, so the startup
    // sequence carries no `Option` branch.
    let shared = match prompt.replay.as_ref() {
        Some(program) => program.clone(),
        None => crate::lua::LuaProgram::empty().map_err(RunError::from)?,
    };
    let ctx = RunContext::new(prompt, args, store, shared_tools, shared, &config);
    let registry = ctx.shared_tools().registry();

    let RunConfig {
        execution,
        observer,
        client,
        cancel,
        limits,
        ..
    } = config;
    let client =
        client.map(|client| client.with_request_limits(limits.timeout(), limits.response_bytes()));
    observer.observe(&execution, &prompt.title, detail::RUN_STARTED);

    let run_body = async {
        let h1 = execute_live_h1(&ctx, resolution, &registry, client.as_ref()).await?;
        if let Some(value) = h1.returned {
            return Ok(value);
        }
        if prompt.sections.is_empty() {
            return Ok(h1.reply.unwrap_or_else(|| GENERIC_COMPLETION.to_string()));
        }
        let analysis = ToolAnalysis::new(&h1.bindings, resolution.picker)?;
        run_sections(
            &ctx,
            &h1.bindings,
            &h1.models,
            &analysis,
            Some(&h1.var),
            client.as_ref(),
        )
        .await
    };

    // Explicit cancellation: when the caller supplies a handle it is installed
    // for the run so cooperative cancel checks observe it; without one the run
    // simply is not cancellable from this path.
    let result = cancel::maybe_scope(cancel, run_body).await;

    observer.observe(
        &execution,
        &prompt.title,
        if result.is_ok() {
            detail::RUN_SUCCEEDED
        } else {
            detail::RUN_FAILED
        },
    );
    result.map_err(RunError::from)
}

#[cfg(test)]
mod tests;
