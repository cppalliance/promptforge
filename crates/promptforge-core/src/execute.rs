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
//! advertised under their local aliases and dispatched through the
//! implementation each binding carries.
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
//! # Runtime
//!
//! One driver task runs a whole prompt: section Lua yields request messages
//! to the chain-stack scheduler, which awaits I/O without blocking a worker
//! thread and resumes the chain with the answer, so a run needs no
//! particular Tokio runtime flavor - a current-thread runtime runs any
//! prompt, host calls included. Concurrency (a fanout's arms) comes from
//! interleaving chains at I/O points on the driver's thread, not from
//! worker threads.
//!
//! # Module layout
//!
//! The orchestration boundary ([`run`]) lives here; the rest is split into
//! focused private children: `error` (the public [`RunError`]), `config`
//! (`RunConfig`/`RunLimits`), `context` (the ambient `RunContext` run
//! state), `gateway` (client acquisition and [`ResolutionContext`]),
//! `scope` (tool-scope validation and schema/dispatch preparation),
//! `tools` (the nested-inference round), `tool_loop` (the model tool
//! loop), `section_vm` (the section VM setup half shared by the walk and
//! the fanout arm), `section_context` (the per-section `SectionContext`
//! frame the scheduler's chains construct, run, and tear down),
//! `block_walk` (the per-block prose paths), `engine` (the walk-target
//! resolution helpers), `protocol` (the coroutine request/answer types
//! for the yield/resume boundary), `scheduler` (the chain-stack scheduler
//! driving the coroutine protocol: the live H1 pass, the walk, execute
//! chains, and fanout), and `support` (shared helpers).

mod block_walk;
mod config;
mod context;
mod engine;
mod error;
mod gateway;
pub(crate) mod protocol;
pub(crate) mod scheduler;
mod scope;
mod section_context;
pub(crate) mod section_vm;
mod support;
mod tool_loop;
mod tools;

// Public API surface.
pub use config::{RunConfig, RunLimits};
pub use error::{RunError, RunErrorKind};
pub use gateway::ResolutionContext;

// Crate-internal items reused through the historical `crate::execute::` path.
// Re-exported so the split stays surface-neutral for the public API while
// keeping one import path for internal collaborators.
pub(crate) use context::RunContext;

use scheduler::Scheduler;

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
pub(crate) use scope::{DispatchTarget, prepare_effective_scope, prepare_scoped_tools};
#[cfg(test)]
pub(crate) use serde_json::json;
#[cfg(test)]
pub(crate) use std::collections::BTreeMap;
#[cfg(test)]
pub(crate) use support::{advance_turn, now_rfc3339_checked};
#[cfg(test)]
pub(crate) use tool_loop::{LocalDispatch, ProseMode, run_prose_inference};

use crate::Error;
use crate::cancel;
use crate::observe::detail;
use crate::parser::{ParseErrorKind, Prompt};
use crate::store::StoreRef;
use support::SUPPORTED_MAJOR;

// Re-exported for the executor test glob.
#[cfg(test)]
pub(crate) use crate::model::ModelSet;

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
///   backend, or decode layer.
/// - [`RunErrorKind::Tool`] - a dispatched tool failed, was out of scope, or the
///   tool loop did not converge.
/// - [`RunErrorKind::Lua`] - a section's Lua phase failed to run or return a
///   usable value.
/// - [`RunErrorKind::Quota`] - a Lua host resource quota (log events, log bytes,
///   or instructions) was exhausted.
/// - [`RunErrorKind::Substitution`] - a `{{ }}` prose substitution failed.
/// - [`RunErrorKind::Store`] - a run-scoped store operation failed.
/// - [`RunErrorKind::Cancelled`] - the host cancelled the run.
/// - [`RunErrorKind::Internal`] - an internal invariant failed.
///
/// # Examples
/// A no-network prompt whose walk makes a nested host call: `execute` is a
/// structural request the scheduler drives on the run's one thread, so the
/// current-thread runtime below runs the whole prompt, host calls included:
/// ```
/// use promptforge_core::execute::{run, RunConfig, ResolutionContext};
/// use promptforge_core::model::ModelCatalog;
/// use promptforge_core::observe::NullObserver;
/// use promptforge_core::parser::Prompt;
/// use promptforge_core::store::StoreRef;
/// use promptforge_core::tools::ToolCatalog;
/// use promptforge_tool_picker::{Catalog, Config, ToolPicker};
///
/// let source = concat!(
///     "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n",
///     "# Title\n\n",
///     "## Calls\n\n",
///     "```lua\nreturn execute('## Answers')\n```\n\n",
///     "## Answers\n\n",
///     "```lua\nreturn 'hello'\n```\n",
/// );
/// let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
/// let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())?;
/// let models = ModelCatalog::empty();
/// let tools = ToolCatalog::new(&[])?;
///
/// let runtime = tokio::runtime::Builder::new_current_thread().build()?;
/// let output = runtime.block_on(run(
///     &prompt,
///     "",
///     ResolutionContext::new(&picker, &models, &tools),
///     &StoreRef::memory(),
///     RunConfig::new("doc-example"),
/// ))?;
/// assert_eq!(output, "hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Runtime
/// A run needs no particular Tokio runtime flavor. Every chain step - all
/// Lua, the walk, execute chains, and fanout joins - executes inside the
/// one driver task, and suspending Lua host calls (`models.infer`,
/// `execute`, `fanout`) are coroutine yields the scheduler answers, so no
/// host call parks a worker thread. Concurrency (a fanout's arms) comes
/// from interleaving chains at I/O points, not from threads; on a
/// multi-thread runtime only the leaf I/O waits, which never touch Lua or
/// scheduler state, may run on other workers.
pub async fn run(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
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

    // Section startup replays the shared library unconditionally; a prompt
    // without one replays an empty compiled chunk instead, so the startup
    // sequence carries no `Option` branch.
    let shared = match prompt.replay.as_ref() {
        Some(program) => program.clone(),
        None => {
            crate::lua::LuaProgram::empty().map_err(|error| RunError::from(Error::from(error)))?
        }
    };
    let ctx = RunContext::new(prompt, args, store, shared, &config);

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
        Scheduler::new(&ctx, client)
            .with_live_h1(resolution)
            .drive()
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
