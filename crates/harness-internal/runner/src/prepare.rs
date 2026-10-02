//! Run preparation: everything between a prompt file on disk and a `Run`
//! the effect loop can drive.
//!
//! Here the Harness draws the inputs the Engine refuses to draw itself:
//! the run's seed from the OS CSPRNG and its `started_at` from the wall
//! clock, both handed to the recorder when the run begins, before anything
//! else, so the record can hand them back verbatim to a future replay. Then
//! the ceremony the Engine's `Environment` expects of the Harness:
//! parse; put the prompt's declared `input:` file in place in the store
//! (`files::stage_input`); hand the run's whole filesystem, real
//! directories and the declared store, to the capabilities' services
//! beside the Host's, and to the context as given; activate the prompt's
//! declared capabilities against the caller's registry, which assembles the
//! catalog, the preludes, and the implementation table; install the
//! catalog and the preludes and prepare the context; merge activation's
//! report into prepare's and refuse an
//! unsatisfiable prompt with the Engine's model-readable notice; and
//! build the `Run` beside its performers.
//!
//! A refusal (or a prompt that fails to parse, or an input file that
//! cannot be put in place) is a run that ended before
//! it began: the recorder ends it as failed with the refusal as the
//! message, so the record answers "why did this session fail" for a run
//! the loop never saw. The error carries the events recorded so far, so
//! the caller can show them too.

use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_capabilities::{CapabilityRegistry, HostServices, InputBroker, RunServices, activate};
use promptforge::cancel::CancelHandle;
use promptforge::event::Event;
use promptforge::model::ModelDescriptor;
use promptforge::timestamp::Timestamp;
use promptforge::vfs::{VfsError, VfsRef};
use promptforge::{Environment, RunContext, RunError};
use promptforge::{ParseError, Prompt, Run};
use sha2::{Digest as _, Sha256};

use crate::display_chain::display_chain;
use crate::effect_loop::failed_outcome;
use crate::files::{InputFileError, stage_input};
use crate::performers::{ActivatedTools, ChatPerformer, Performers, TokioTimer};
use crate::recorder::{Record, RecordKind, RecorderError, RunId, RunMeta, RunOutcome, RunRecorder};
use crate::spawn::spawn_blocking_launch;

/// What the caller owns and preparation borrows: the registry of
/// installed capabilities and the Host's services, the real directories,
/// the run's cancel flag, the recorder, the chat performer and the
/// optional input broker that reach beyond the runner, and the session's
/// identity for the run's metadata.
pub struct Services {
    /// The installed capabilities the prompt's declarations resolve
    /// against; `None` is a Harness with no capabilities, where every
    /// required declaration is reported missing.
    pub registry: Option<Arc<CapabilityRegistry>>,
    /// The Host's services: the run's capabilities read them, and the
    /// input broker, when present, replaces any provider under its id.
    pub services: HostServices,
    /// The run's whole filesystem: the real directories and the declared store,
    /// passed straight to the context's VFS and handed to the capabilities
    /// as the run's services.
    pub vfs: VfsRef,
    /// The text staged at the prompt's declared `input:` path before the
    /// run, when the launch supplied one.
    pub input_text: Option<String>,
    /// The run's cancel flag: handed to the context, to every capability
    /// activated for the run, and polled by the Engine.
    pub cancel: CancelHandle,
    /// The recorder the run begins at and the loop will write to.
    pub recorder: Arc<dyn RunRecorder>,
    /// Performs the run's `Chat` effects.
    pub chat: Arc<dyn ChatPerformer>,
    /// The operator's input broker, when the Host has someone to ask:
    /// handed to every capability activated for the run. `None` is a Host
    /// with nobody to ask.
    pub input: Option<Arc<dyn InputBroker>>,
    /// The session launching the run: the run metadata's `session_id` and
    /// the run's execution identifier.
    pub session_id: String,
    /// The agent the session runs: the run metadata's `agent`.
    pub agent: String,
    /// The Host's current model, when one is selected; prepare binds
    /// every declared role to it and checks each role's requirements.
    pub model: Option<ModelDescriptor>,
    /// The Host snapshot the `ui()` global serves, when the run has
    /// one.
    pub ui: Option<serde_json::Value>,
}

impl fmt::Debug for Services {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Services")
            .field("registry", &self.registry)
            .field("services", &self.services)
            .field("session_id", &self.session_id)
            .field("agent", &self.agent)
            .field("model", &self.model)
            .field("ui", &self.ui)
            .finish_non_exhaustive()
    }
}

/// A run ready for the effect loop, with the inputs the Harness drew for it.
#[derive(Debug)]
pub struct Prepared {
    /// The run, built over the prepared context.
    pub run: Run,
    /// The run's id at the recorder, begun and still open, for
    /// [`drive_run`](crate::effect_loop::drive_run).
    pub run_id: RunId,
    /// The seed the run was given, as handed to the recorder.
    pub seed: u64,
    /// The start the run was given, as handed to the recorder.
    pub started_at: Timestamp,
    /// The performers for the run: the caller's chat performer beside the
    /// runner's own over the activated tools, the VFS, and tokio's timer.
    pub performers: Performers,
    /// What parsing reported, already recorded ahead of the run's own
    /// events; the caller hands them to its sink so the session sees them
    /// in order.
    pub parse_events: Vec<Event>,
    /// The prompt's declared `output:` path, which the caller reads with
    /// [`read_output`](crate::files::read_output) once the run completes.
    pub output_path: Option<String>,
}

/// Why a run could not be prepared.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PrepareError {
    /// The prompt file could not be read; the run is never begun, since
    /// there is no prompt to record. The read failure is the source.
    #[error("the prompt at {path} could not be read")]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The read failure.
        #[source]
        source: io::Error,
    },
    /// The prompt does not parse. The run is ended as failed; the parse
    /// failure is the source.
    #[error("the prompt at {path} does not parse")]
    Parse {
        /// The path that was parsed.
        path: PathBuf,
        /// The run, ended with this failure.
        run_id: RunId,
        /// What parsing reported, in the order it was recorded.
        events: Vec<Event>,
        /// The parse failure.
        #[source]
        source: ParseError,
    },
    /// The environment cannot satisfy the prompt: a required capability
    /// is missing, two declared capabilities conflict, or the current
    /// model falls short of a role's requirements. The Engine's
    /// model-readable notice, one line per gap, is the source; the run is
    /// ended as failed with that notice.
    #[error("the environment cannot satisfy the prompt")]
    Refused {
        /// The run, ended with this refusal.
        run_id: RunId,
        /// What parsing reported, in the order it was recorded.
        events: Vec<Event>,
        /// The refusal, of kind `RequirementsUnmet`.
        #[source]
        error: RunError,
    },
    /// The prompt's declared input file could not be put in place: the
    /// launch supplied text the prompt declares no file for, the prompt
    /// declares a file that is neither supplied nor in the store, or the
    /// store refused. The run is ended as failed with kind `Input`.
    #[error("the prompt's declared input file cannot be put in place")]
    Input {
        /// The run, ended with this refusal.
        run_id: RunId,
        /// What parsing reported, in the order it was recorded.
        events: Vec<Event>,
        /// Why the input could not be put in place.
        #[source]
        source: InputFileError,
    },
    /// The recorder refused a write; the run cannot be recorded, so it is
    /// not prepared.
    #[error(transparent)]
    Recorder(#[from] RecorderError),
}

/// Prepares the prompt at `prompt_path` for one run with `args`: draws the
/// run's seed and start and begins the run at its recorder, parses the
/// prompt, puts its declared input file in place, activates its declared
/// capabilities against the caller's registry, prepares the context,
/// refuses an unsatisfiable prompt, and builds the `Run` and its
/// performers.
///
/// # Errors
/// Returns [`PrepareError::Read`] when the file cannot be read (the run is
/// never begun), [`PrepareError::Parse`] when it does not parse,
/// [`PrepareError::Input`] when its declared input file cannot be put in
/// place, and [`PrepareError::Refused`] when the environment cannot
/// satisfy it (in these three cases the run is ended as failed and the
/// error carries the events recorded so far), and
/// [`PrepareError::Recorder`] when the recorder refuses a write.
#[expect(
    clippy::result_large_err,
    reason = "the error is returned once per launch and carries the events recorded before the failure"
)]
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

/// Prepares prompt text already in hand, just as [`prepare_run`] does
/// after its read: for a prompt that has no file of its own (an embedded
/// built-in) or one the caller read itself. `prompt_path` is the path the
/// source is attributed to in [`PrepareError::Parse`].
///
/// # Errors
/// Returns [`PrepareError::Parse`] when the source does not parse,
/// [`PrepareError::Input`] when its declared input file cannot be put in
/// place, and [`PrepareError::Refused`] when the environment cannot
/// satisfy it (in these three cases the run is ended as failed and the
/// error carries the events recorded so far), and
/// [`PrepareError::Recorder`] when the recorder refuses a write. Never
/// [`PrepareError::Read`].
#[expect(
    clippy::result_large_err,
    reason = "the error is returned once per launch and carries the events recorded before the failure"
)]
pub async fn prepare_source(
    source: &str,
    prompt_path: &Path,
    args: &str,
    services: Services,
) -> Result<Prepared, PrepareError> {
    let Services {
        registry,
        services: host,
        vfs,
        input_text,
        cancel,
        recorder,
        chat,
        input,
        session_id,
        agent,
        model,
        ui,
    } = services;

    // The inputs the Engine leaves to the Harness, recorded before the
    // run exists so the record has them however the run ends.
    let seed: u64 = rand::random();
    let started_at = now_timestamp();
    let run_id = recorder
        .begin_run(RunMeta {
            session_id: session_id.clone(),
            agent: agent.clone(),
            prompt_hash: prompt_hash(source),
            seed,
            flags: 0,
            started_at: started_at.unix_millis(),
        })
        .await?;

    // Parse-time events are the run's first records, whether or not the
    // parse succeeds.
    let (prompt, parse_events) = Prompt::parse(source, &session_id);
    for event in &parse_events {
        recorder.append(run_id, event_record(event)?).await?;
    }
    let prompt = match prompt {
        Ok(prompt) => prompt,
        Err(source) => {
            recorder.end_run(run_id, failed("Parse", &source)).await?;
            return Err(PrepareError::Parse {
                path: prompt_path.to_path_buf(),
                run_id,
                events: parse_events,
                source,
            });
        }
    };

    // The declared input is in place before anything else sees the
    // store: the capabilities activate over the same filesystem, and the
    // run's first section may read it.
    if let Err(source) = stage_declared_input(&prompt, &vfs, input_text, &agent).await {
        recorder.end_run(run_id, failed("Input", &source)).await?;
        return Err(PrepareError::Input {
            run_id,
            events: parse_events,
            source,
        });
    }
    let output_path = prompt
        .frontmatter()
        .output()
        .map(|decl| decl.path().to_owned());

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
    // The activate-prepare-refuse ceremony: the run's whole filesystem,
    // the real directories and the declared store, is handed to the
    // capabilities' services beside the Host's and to the context as
    // given, so the capabilities and the run share one filesystem; the
    // activated catalog is what prepare fills slots against, its preludes
    // go to every section VM, and the implementations stay here for the
    // tool performer.
    let env = Environment::new();
    let ctx = ctx.vfs(vfs);
    let mut run_services =
        RunServices::with_host(ctx.vfs_handle().clone(), ctx.cancel_handle(), host);
    if let Some(broker) = input {
        run_services.insert_input_broker(broker);
    }
    let activation = activate(registry.as_deref(), &prompt, &run_services);
    let env = env.tools(activation.catalog).preludes(activation.preludes);
    let (ctx, mut requirements) = env.prepare(&prompt, ctx);
    requirements.merge(activation.requirements);
    if let Some(error) = requirements.refusal() {
        recorder.end_run(run_id, failed_outcome(&error)).await?;
        return Err(PrepareError::Refused {
            run_id,
            events: parse_events,
            error,
        });
    }

    let run = Run::new(Arc::new(prompt), args, ctx);
    let performers = Performers {
        chat,
        tool: Arc::new(ActivatedTools::new(activation.tools)),
        timer: Arc::new(TokioTimer),
    };
    Ok(Prepared {
        run,
        run_id,
        seed,
        started_at,
        performers,
        parse_events,
        output_path,
    })
}

/// Puts `prompt`'s declared input file in place in `vfs`'s store on the
/// blocking pool, tagged with `agent`.
async fn stage_declared_input(
    prompt: &Prompt,
    vfs: &VfsRef,
    input_text: Option<String>,
    agent: &str,
) -> Result<(), InputFileError> {
    let declared = prompt
        .frontmatter()
        .input()
        .map(|decl| decl.path().to_owned());
    if declared.is_none() && input_text.is_none() {
        return Ok(());
    }
    let staging = vfs.clone();
    let path = declared.clone();
    spawn_blocking_launch(agent, move || {
        stage_input(&staging, path.as_deref(), input_text)
    })
    .await
    .unwrap_or_else(|join| {
        Err(InputFileError::Vfs {
            path: declared.unwrap_or_default(),
            source: VfsError::Backend {
                message: format!("the staging task failed: {join}"),
            },
        })
    })
}

/// The failed outcome of a run that ended in preparation: `kind` names the
/// stage and the message is the cause chain.
fn failed(kind: &str, source: &dyn std::error::Error) -> RunOutcome {
    RunOutcome::Failed {
        kind: kind.to_owned(),
        message: display_chain(source),
    }
}

/// One event's record under its own provenance.
fn event_record(event: &Event) -> Result<Record, RecorderError> {
    let provenance = event.provenance();
    Ok(Record {
        task_id: provenance.task.to_string(),
        task_seq: provenance.seq,
        kind: RecordKind::Event,
        effect_id: None,
        payload: serde_json::to_value(event).map_err(RecorderError::new)?,
    })
}

/// The prompt text's content hash for the run's metadata, `sha256:` and
/// the lowercase hex digest.
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

/// The system clock now as the Engine's `Timestamp`: the Harness's stamp for
/// a run's `started_at`, since the Engine reads no clock of its own. A
/// clock before the epoch or beyond `i64` milliseconds (neither reachable
/// on a real machine) saturates to the epoch rather than refusing the launch.
fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis)
}
