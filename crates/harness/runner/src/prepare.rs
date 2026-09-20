//! Run preparation: everything between a prompt file on disk and a `Run`
//! the effect loop can drive.
//!
//! The harness is the engine's host, so the host inputs the engine
//! refuses to draw itself are drawn here: the run's seed from the OS
//! CSPRNG and its `started_at` from the wall clock, both written to the
//! run's row in the log before anything else, so the record can hand them
//! back verbatim to a future replay. Then the ceremony the engine's
//! `Environment` expects of a host: parse; build the run's VFS so the
//! capabilities' services and the run share one store; activate the
//! prompt's declared capabilities against the caller's registry, which
//! assembles the catalog and the implementation table; install the catalog
//! and prepare the context; merge activation's report into prepare's and
//! refuse an unsatisfiable prompt with the engine's model-readable notice;
//! and build the `Run` beside its performers.
//!
//! A refusal (or a prompt that fails to parse) is a run that ended before
//! it began: its row is closed as failed with the refusal as the message,
//! so the log answers "why did this session fail" for a run the loop
//! never saw.

use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_capabilities::{CapabilityRegistry, RunServices, activate};
use harness_log::{LogError, Record, RecordKind, RunId, RunMeta, RunOutcome};
use promptforge_api_runtime::execute::{Environment, RunContext, RunError};
use promptforge_api_runtime::{ParseError, Prompt, Run};
use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::event::Event;
use promptforge_api_types::models::ModelDescriptor;
use promptforge_api_types::timestamp::Timestamp;
use sha2::{Digest as _, Sha256};
use shared_vfs::VfsRef;

use crate::effect_loop::{SharedLog, failed_outcome};
use crate::performers::{
    ActivatedTools, ChatPerformer, InputPerformer, LogTaskEvents, Performers, TokioTimer, VfsStore,
};

/// What the caller owns and preparation borrows: the registry of
/// installed capabilities, the host roots, the run's cancel flag, the
/// log, the two performers that reach beyond the runner, and the
/// session's identity for the run's row.
pub struct Services {
    /// The installed capabilities the prompt's declarations resolve
    /// against; `None` is a host with no capabilities, where every
    /// required declaration is reported missing.
    pub registry: Option<Arc<CapabilityRegistry>>,
    /// The host roots the run's VFS mounts at `/`; never the store mount,
    /// which preparation adds fresh per run.
    pub vfs: VfsRef,
    /// The run's cancel flag: handed to the context, to every capability
    /// activated for the run, and polled by the engine.
    pub cancel: CancelHandle,
    /// The run log the row is opened in and the loop will write to.
    pub log: SharedLog,
    /// Performs the run's `Chat` effects.
    pub chat: Arc<dyn ChatPerformer>,
    /// Performs the run's `UserInput` effects.
    pub input: Arc<dyn InputPerformer>,
    /// The session launching the run: the row's `session_id` and the
    /// run's execution identifier.
    pub session_id: String,
    /// The agent the session runs: the row's `agent`.
    pub agent: String,
    /// The host's current model, when one is selected; prepare binds
    /// every declared role to it and checks each role's requirements.
    pub model: Option<ModelDescriptor>,
    /// The host-state snapshot the `ui()` global serves, when the run has
    /// one.
    pub ui: Option<serde_json::Value>,
}

impl fmt::Debug for Services {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Services")
            .field("registry", &self.registry)
            .field("session_id", &self.session_id)
            .field("agent", &self.agent)
            .field("model", &self.model)
            .field("ui", &self.ui)
            .finish_non_exhaustive()
    }
}

/// A run ready for the effect loop, with the host inputs it was given.
#[derive(Debug)]
pub struct Prepared {
    /// The run, built over the prepared context.
    pub run: Run,
    /// The run's open row in the log, for [`drive_run`](crate::effect_loop::drive_run).
    pub run_id: RunId,
    /// The seed the run was given, as written to its row.
    pub seed: u64,
    /// The start the run was given, as written to its row.
    pub started_at: Timestamp,
    /// The performers for the run: the caller's chat and input performers
    /// beside the runner's own over the activated tools, the VFS, tokio's
    /// timer, and the log.
    pub performers: Performers,
    /// What parsing reported, already recorded in the log ahead of the
    /// run's own events; the caller hands them to its sink so the session
    /// sees them in order.
    pub parse_events: Vec<Event>,
}

/// Why a run could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// The prompt file could not be read; no row is written, since there
    /// is no prompt to record.
    #[error("the prompt at {path} could not be read: {source}")]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The read failure.
        #[source]
        source: io::Error,
    },
    /// The prompt failed to parse. Its row is closed as failed.
    #[error("the prompt at {path} failed to parse: {source}")]
    Parse {
        /// The path that was parsed.
        path: PathBuf,
        /// The run's row, closed with this failure.
        run_id: RunId,
        /// The parse failure.
        #[source]
        source: ParseError,
    },
    /// The environment cannot satisfy the prompt: a required capability
    /// is missing, two declared capabilities conflict, or the current
    /// model falls short of a role's requirements. The message is the
    /// engine's model-readable notice, one line per gap. The run's row is
    /// closed as failed with that notice.
    #[error("{error}")]
    Refused {
        /// The run's row, closed with this refusal.
        run_id: RunId,
        /// The refusal, of kind `RequirementsUnmet`.
        #[source]
        error: RunError,
    },
    /// The run log refused a write; the run cannot be recorded, so it is
    /// not prepared.
    #[error(transparent)]
    Log(#[from] LogError),
}

/// Prepares the prompt at `prompt_path` for one run with `args`: draws the
/// run's seed and start and opens its row in the log, parses the prompt,
/// activates its declared capabilities against the caller's registry,
/// prepares the context, refuses an unsatisfiable prompt, and builds the
/// `Run` and its performers.
///
/// # Errors
/// Returns [`PrepareError::Read`] when the file cannot be read (no row is
/// written), [`PrepareError::Parse`] when it does not parse and
/// [`PrepareError::Refused`] when the environment cannot satisfy it (in
/// both cases the row is closed as failed), and [`PrepareError::Log`]
/// when the log refuses a write.
pub async fn prepare_run(
    prompt_path: &Path,
    args: &str,
    services: Services,
) -> Result<Prepared, PrepareError> {
    let source = tokio::fs::read_to_string(prompt_path)
        .await
        .map_err(|source| PrepareError::Read {
            path: prompt_path.to_path_buf(),
            source,
        })?;
    prepare_source(&source, prompt_path, args, services).await
}

/// Prepares prompt text already in hand, exactly as [`prepare_run`] does
/// after its read: for a prompt that has no file of its own (an embedded
/// built-in) or one the caller read itself. `prompt_path` is the path the
/// source is attributed to in [`PrepareError::Parse`].
///
/// # Errors
/// Returns [`PrepareError::Parse`] when the source does not parse and
/// [`PrepareError::Refused`] when the environment cannot satisfy it (in
/// both cases the row is closed as failed), and [`PrepareError::Log`]
/// when the log refuses a write. Never [`PrepareError::Read`].
pub async fn prepare_source(
    source: &str,
    prompt_path: &Path,
    args: &str,
    services: Services,
) -> Result<Prepared, PrepareError> {
    let Services {
        registry,
        vfs,
        cancel,
        log,
        chat,
        input,
        session_id,
        agent,
        model,
        ui,
    } = services;

    // The host inputs the engine never draws itself, recorded before the
    // run exists so the record has them however the run ends.
    let seed: u64 = rand::random();
    let started_at = now_timestamp();
    let run_id = log
        .lock()
        .await
        .begin_run(RunMeta {
            session_id: session_id.clone(),
            agent,
            prompt_hash: prompt_hash(source),
            seed,
            flags: 0,
            started_at: started_at.unix_millis(),
        })
        .await?;

    // Parse-time events are the run's first records, whether or not the
    // parse succeeds.
    let (prompt, parse_events) = Prompt::parse(source, &session_id);
    {
        let mut log = log.lock().await;
        for event in &parse_events {
            log.append(run_id, event_record(event)?).await?;
        }
    }
    let prompt = match prompt {
        Ok(prompt) => prompt,
        Err(source) => {
            let outcome = RunOutcome::Failed {
                kind: "Parse".to_owned(),
                message: source.to_string(),
            };
            close_failed(&log, run_id, outcome).await?;
            return Err(PrepareError::Parse {
                path: prompt_path.to_path_buf(),
                run_id,
                source,
            });
        }
    };

    // The parse events were stamped under task `0` from zero; the run's
    // root task continues the sequence past them, so `(task_id, task_seq)`
    // is unique across every record of the run.
    let provenance_start = u32::try_from(parse_events.len()).unwrap_or(u32::MAX);
    let mut ctx = RunContext::new(session_id, seed, started_at)
        .cancel(cancel)
        .provenance_start(provenance_start);
    if let Some(ui) = ui {
        ctx = ctx.ui(ui);
    }
    if let Some(model) = model {
        ctx = ctx.model(model);
    }
    // The activate-prepare-refuse ceremony: the run's VFS is built first so
    // the capabilities' services and the run share one store; the
    // activated catalog is what prepare fills slots against, and the
    // implementations stay here for the tool performer.
    let env = Environment::new().base_vfs(vfs);
    let ctx = ctx.vfs(env.run_vfs());
    let run_services = RunServices::new(ctx.vfs_handle().clone(), ctx.cancel_handle());
    let activation = activate(registry.as_deref(), &prompt, &run_services);
    let env = env.tools(activation.catalog);
    let (ctx, mut requirements) = env.prepare(&prompt, ctx);
    requirements.merge(activation.requirements);
    if let Some(error) = requirements.refusal() {
        close_failed(&log, run_id, failed_outcome(&error)).await?;
        return Err(PrepareError::Refused { run_id, error });
    }

    let run = Run::new(Arc::new(prompt), args, ctx);
    let performers = Performers {
        chat,
        tool: Arc::new(ActivatedTools::new(activation.tools)),
        input,
        store: Arc::new(VfsStore),
        timer: Arc::new(TokioTimer),
        task_events: Arc::new(LogTaskEvents::new(Arc::clone(&log), run_id)),
    };
    Ok(Prepared {
        run,
        run_id,
        seed,
        started_at,
        performers,
        parse_events,
    })
}

/// Closes `run_id`'s row with `outcome`, a run that ended before the loop
/// saw it.
async fn close_failed(log: &SharedLog, run_id: RunId, outcome: RunOutcome) -> Result<(), LogError> {
    log.lock().await.end_run(run_id, outcome).await
}

/// One event's record under its own provenance.
fn event_record(event: &Event) -> Result<Record, LogError> {
    let provenance = event.provenance();
    Ok(Record {
        task_id: provenance.task.to_string(),
        task_seq: provenance.seq,
        kind: RecordKind::Event,
        effect_id: None,
        payload: serde_json::to_value(event)?,
    })
}

/// The prompt text's content hash for the run's row, `sha256:` and the
/// lowercase hex digest.
fn prompt_hash(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let mut hash = String::with_capacity(7 + digest.len() * 2);
    hash.push_str("sha256:");
    for byte in digest {
        // Writing to a String is infallible.
        let _ = write!(hash, "{byte:02x}");
    }
    hash
}

/// The system clock now as the engine's `Timestamp`: the host's stamp for
/// a run's `started_at`, since the engine reads no clock of its own. A
/// clock before the epoch or beyond `i64` milliseconds (neither reachable
/// on a real host) saturates to the epoch rather than refusing the launch.
fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis)
}
