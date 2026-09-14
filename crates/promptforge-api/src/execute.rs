//! Section lifecycle execution and fall-through.
//!
//! The run walks top-level sections in file order, creating one isolated
//! section VM for each. The VM is fully equipped (host values, store, log,
//! control globals) before the shared Lua library replays as the section's
//! first chunk, then ordered section blocks use that same VM. Prose never
//! infers: each prose block stashes the pending Markdown buffer, and the
//! next Lua block reads it as its fresh read-only lazy `prose` template.
//! A scalar Lua return ends the chain it fires in.
//!
//! Running off the last section ends the run: the result is the last
//! scalar return, else a generic completion.
//!
//! The walk is level-independent and never descends on its own: a jump to a
//! child heading starts a child-level walk over the jumper's children under
//! the same rules, and the parent walk resumes after the jumper when that
//! level exhausts.
//!
//! One run-scoped store handle travels with the run's [`RunContext`] (the
//! stock handle by default), shared by
//! every section, so
//! bulk state persists across the context-clearing transitions even though a
//! section's Lua state never does.
//!
//! A run reports itself as it goes: the [`RunContext`] observer receives a
//! `(execution, section, event)` record when the run starts and ends, at each
//! section boundary, model turn, tool call, and harness-mediated store
//! operation. Reporting is a side channel and never
//! a decision, so passing [`NullObserver`](shared_promptforge_api::observe::NullObserver) changes nothing but
//! the silence.
//!
//! Rust installs the run's filled tool and model slots - bound at prepare
//! from the frontmatter - into each section VM. Prompt-wide aliases and
//! section additions form the effective model-visible scope,
//! which is checked for semantic near-duplicates before concrete tools are
//! advertised under their local aliases and dispatched through the
//! implementation each binding carries.
//!
//! Lua `call()` starts a contained chain at a visible section (fresh VM,
//! recursion capped at 8): the chain runs from the target
//! with every normal walk rule - fall-through, jumps, child
//! chains - and the outer walk never moves while it runs. When the chain
//! ends (its level exhausts or a return fires), its final text is the call's
//! return value; a return ends only the chain it fires in.
//! Lua `jump(target)` transfers control to a named section.
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
//! ([`RunContext`]/[`RunLimits`]), `environment` (the public
//! [`Environment`]), `requirements` (the preflight
//! [`Requirements`] report), `context` (the ambient `RunState` run
//! state), `gateway` (client acquisition and the live H1 resolution
//! inputs),
//! `tools` (the nested-inference round),
//! `section_vm` (the section VM setup half shared by the walk and
//! the fanout arm), `section_context` (the per-section `SectionContext`
//! frame the scheduler's chains construct, run, and tear down),
//! `engine` (the walk-target
//! resolution helpers), `protocol` (the coroutine request/answer types
//! for the yield/resume boundary), `scheduler` (the chain-stack scheduler
//! driving the coroutine protocol: the live H1 pass, the walk, call
//! chains, and fanout), `scope` (tool-scope
//! validation and schema/dispatch preparation), `tool_loop` (the
//! Rust-backed model-tool loop behind the section-visible `models.loop`),
//! and `support` (shared helpers).

mod bindings;
mod config;
mod context;
mod engine;
mod environment;
mod error;
mod fill;
mod gateway;
pub(crate) mod protocol;
mod requirements;
mod scheduler;
mod scope;
mod section_context;
pub(crate) mod section_vm;
mod support;
mod tool_loop;
mod tools;

// Public API surface.
pub use bindings::{ModelBindings, ToolBindings};
pub use config::{RunContext, RunLimits};
pub use environment::Environment;
pub use error::{RunError, RunErrorKind, SourceLocation};
pub use requirements::{CapabilityConflict, RequirementCheck, Requirements, UnmetRequirement};

use context::RunState;
use scheduler::Scheduler;

use crate::Error;
use crate::cancel;
use crate::observe::detail;
use crate::parser::{ParseErrorKind, Prompt};
use crate::store::VfsRef;

/// What the run produced. Domain outcomes (including "the prompt
/// declined") are values, not thrown errors: the variant is for code, the
/// payload is for humans and models.
#[derive(Debug)]
pub enum RunResult {
    /// The run completed with its final text. Mirrors `Result` vocabulary,
    /// so patterns need `RunResult::Ok` qualification wherever `Result` is
    /// also in scope.
    Ok(String),
    /// The host cancelled the run.
    Cancelled,
    /// The run failed; the typed error classifies the failure.
    Failure(RunError),
}

/// Executes a parsed prompt and returns its final text.
///
/// H1 is section 0: its Lua and prose blocks run once in source order with
/// the same surface every section gets (its only privilege, `argv`
/// writability, arrives with the args/argv step). If H1 does not return,
/// the H2 section walk runs and its final text is returned.
///
/// The free `run` receives an already-prepared [`RunContext`] and has
/// nothing to prepare from: a context that never passed through
/// [`Environment::prepare`] runs capability-free (empty tool and model
/// sets). Hosts normally go through [`Environment::run`], the
/// zero-burden path.
///
/// # Outcomes
/// - [`RunResult::Ok`] - the run completed with its final text.
/// - [`RunResult::Cancelled`] - the host cancelled the run.
/// - [`RunResult::Failure`] - the run failed; the [`RunError`]'s
///   [`kind`](RunError::kind) classifies the failure by condition:
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
/// - [`RunErrorKind::ContextExhausted`] - the selected compactor exhausted the
///   model's context window.
/// - [`RunErrorKind::Input`] - the host's input broker failed a `user_input`
///   request.
/// - [`RunErrorKind::Substitution`] - a `{{ }}` prose substitution failed.
/// - [`RunErrorKind::Store`] - a run-scoped store operation failed.
/// - [`RunErrorKind::Determinism`] - two live execution identities claimed
///   one store path; the run terminated on the spot, uncatchably from Lua.
/// - [`RunErrorKind::Cancelled`] - the host cancelled the run (mid-run
///   classification only; the interface reports [`RunResult::Cancelled`]).
/// - [`RunErrorKind::Internal`] - an internal invariant failed.
/// - [`RunErrorKind::RequirementsUnmet`] - an H1 assertion or model
///   requirement the environment cannot satisfy.
///
/// # Examples
/// A no-network prompt whose walk makes a nested host call: `call` is a
/// structural request the scheduler drives on the run's one thread, so the
/// current-thread runtime below runs the whole prompt, host calls included:
/// ```
/// use promptforge_api::execute::{RunContext, RunResult, run};
/// use promptforge_api::parser::Prompt;
/// use shared_promptforge_api::observe::NullObserver;
///
/// let source = concat!(
///     "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
///     "# Title\n\n",
///     "## Calls\n\n",
///     "```lua\nreturn call('## Answers')\n```\n\n",
///     "## Answers\n\n",
///     "```lua\nreturn 'hello'\n```\n",
/// );
/// let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
/// let runtime = tokio::runtime::Builder::new_current_thread().build()?;
/// let output = runtime.block_on(run(&prompt, "", RunContext::new("doc-example")));
/// let RunResult::Ok(text) = output else {
///     panic!("the doc example run succeeds: {output:?}");
/// };
/// assert_eq!(text, "hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Runtime
/// A run needs no particular Tokio runtime flavor. Every chain step - all
/// Lua, the walk, call chains, and fanout joins - executes inside the
/// one driver task, and suspending Lua host calls (`models.infer`,
/// `call`, `fanout`) are coroutine yields the scheduler answers, so no
/// host call parks a worker thread. Concurrency (a fanout's arms) comes
/// from interleaving chains at I/O points, not from threads; on a
/// multi-thread runtime only the leaf I/O waits, which never touch Lua or
/// scheduler state, may run on other workers.
pub async fn run(prompt: &Prompt, args: &str, ctx: RunContext) -> RunResult {
    match prompt.frontmatter().promptforge() {
        Some(0) => {}
        Some(other) => {
            return RunResult::Failure(RunError::from(Error::UnsupportedVersion(other)));
        }
        None => {
            return RunResult::Failure(RunError::from(
                Error::parse(
                    ParseErrorKind::Structure,
                    "not a promptforge prompt: no promptforge version",
                )
                .with_prompt_name(prompt.frontmatter().name()),
            ));
        }
    }

    // Section startup replays the shared library unconditionally; a prompt
    // without one replays an empty compiled chunk instead, so the startup
    // sequence carries no `Option` branch.
    let shared = match prompt.replay() {
        Some(program) => program.clone(),
        None => match crate::lua::LuaProgram::empty() {
            Ok(program) => program,
            Err(error) => return RunResult::Failure(RunError::from(Error::from(error))),
        },
    };
    // The stock handle carries the store mount; a hand-built router lacking
    // it gets a fresh memory store overlaid as a defensive fallback, so a
    // run never fails for want of the mount. A mounted-but-failing backend
    // is never shadowed by the throwaway overlay: its error fails the run.
    let mut ctx = ctx;
    match store_mount_present(&ctx.vfs) {
        Ok(true) => {}
        Ok(false) => {
            ctx.vfs = ctx.vfs.overlay(
                promptforge_vfs::STORE_MOUNT,
                shared_vfs::MemoryBackend::new(),
            );
        }
        Err(error) => return RunResult::Failure(RunError::from(Error::Store(error))),
    }
    let state = RunState::new(prompt, args, &ctx.vfs, shared, &ctx);

    let RunContext {
        name,
        observer,
        client,
        cancel,
        limits,
        ..
    } = ctx;
    let client =
        client.map(|client| client.with_request_limits(limits.timeout(), limits.response_bytes()));
    observer.observe(&name, prompt.title(), detail::RUN_STARTED);

    // Boxed: the driver future carries the whole scheduler step machinery,
    // and `run`'s own future must stay small for its callers (the
    // workspace's large-futures lint gates every one of them).
    let run_body = Box::pin(async { Scheduler::new(&state, client).drive().await });

    // Explicit cancellation: when the caller supplies a handle it is installed
    // for the run so cooperative cancel checks observe it; without one the run
    // simply is not cancellable from this path.
    let result = cancel::maybe_scope(cancel, run_body).await;

    observer.observe(
        &name,
        prompt.title(),
        if result.is_ok() {
            detail::RUN_SUCCEEDED
        } else {
            detail::RUN_FAILED
        },
    );
    match result {
        Ok(text) => RunResult::Ok(text),
        Err(Error::Interrupted) => RunResult::Cancelled,
        Err(error) => RunResult::Failure(RunError::from(error)),
    }
}

/// Whether the handle already serves the store mount. The probe stats the
/// mount root through a throwaway capability: a mounted backend answers
/// (the memory backend's root always exists), an unmounted path is
/// `NotFound`. Only `NotFound` means "mount absent": any other error is the
/// mounted backend's own failure and propagates, so a loud backend failure
/// is never converted into the run silently reading and writing a
/// throwaway overlay. The probe's identity and claim release with the
/// access.
fn store_mount_present(vfs: &VfsRef) -> std::result::Result<bool, shared_vfs::VfsError> {
    match vfs
        .acquire(shared_vfs::Origin::new("store mount probe"))?
        .stat(promptforge_vfs::STORE_MOUNT)
    {
        Ok(_) => Ok(true),
        Err(shared_vfs::VfsError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
