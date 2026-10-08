//! Run preparation: everything between a prompt's source text and a `Run`
//! the effect loop can drive.
//!
//! Here the Harness draws the inputs the Engine refuses to draw itself:
//! the run's seed from the OS CSPRNG and its `started_at` from the wall
//! clock, both handed to the recorder when the run begins, before anything
//! else, so the record holds them verbatim. Then
//! the ceremony the Engine's `Environment` expects of the Harness:
//! parse; put the prompt's declared `input:` file in place in the store
//! (`files::stage_input`); hand the run's whole filesystem, real
//! directories and the declared store, to the context as given; wait for
//! every built Plugin to be ready, or for the run's cancel; take the
//! run's snapshot of the Host's Plugins with the run's own services,
//! which yields the catalog, the preludes, and the tool performer;
//! install the catalog and the preludes and prepare the context; merge
//! the snapshot's report into prepare's and refuse an unsatisfiable
//! prompt with the Engine's model-readable notice; and build the `Run`
//! beside its performers.
//!
//! A refusal (or a prompt that fails to parse, or an input file that
//! cannot be put in place) is a run that ended before
//! it began: the recorder ends it as failed with the refusal as the
//! message, so the record answers "why did this run fail" for a run the
//! loop never saw. A refusal after the run's cancel fired, as when a
//! cancel cuts the wait short while a needed Plugin is still starting,
//! ends the run as cancelled instead. `Harness::run_to_end` reports that
//! ended run, and its caller reads the events recorded so far from the
//! recorder.
//!
//! The input staging is store work and runs inline, like every other VFS
//! operation of the run.

use std::fmt::{self, Write as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use promptforge::cancel::CancelHandle;
use promptforge::event::Event;
use promptforge::model::ModelDescriptor;
use promptforge::prompt::FileDecl;
use promptforge::timestamp::Timestamp;
use promptforge::vfs::VfsRef;
use promptforge::{ParseError, Prompt, Run, RunContext, RunError};
use promptforge_plugin::HostServices;
use sha2::{Digest as _, Sha256};

use crate::display_chain::display_chain;
use crate::effect_loop::failed_outcome;
use crate::files::{InputFileError, stage_input};
use crate::host::HostContext;
use crate::performers::{InferenceBroker, Performers, Timer};
use crate::recorder::{Record, RecordKind, RecorderError, RunId, RunMeta, RunOutcome, RunRecorder};

/// What the caller owns and preparation borrows: the Host's installed
/// Plugins and the run's own services, the real directories, the run's
/// cancel flag, the recorder, the inference broker and the timer that
/// reach beyond the runner, and the run's name.
pub struct Services {
    /// The Host's installed Plugins, which the run takes its snapshot of.
    pub host: Arc<HostContext>,
    /// The run's own services, such as its input broker: every tool call
    /// of the run is lent them.
    pub services: HostServices,
    /// The run's whole filesystem: the real directories and the declared store,
    /// passed straight to the context's VFS.
    pub vfs: VfsRef,
    /// The text staged at the prompt's declared `input:` path before the
    /// run, when the launch supplied one.
    pub input_text: Option<String>,
    /// The run's cancel flag: handed to the context and polled by the
    /// Engine.
    pub cancel: CancelHandle,
    /// The recorder the run begins at and the loop will write to.
    pub recorder: Arc<dyn RunRecorder>,
    /// Performs the run's `Chat` effects.
    pub broker: Arc<dyn InferenceBroker>,
    /// Performs the run's `Timer` effects.
    pub timer: Arc<dyn Timer>,
    /// The run's name: the run metadata's `name` and every event's
    /// `execution`.
    pub name: String,
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
            .field("host", &self.host)
            .field("services", &self.services)
            .field("name", &self.name)
            .field("model", &self.model)
            .field("ui", &self.ui)
            .finish_non_exhaustive()
    }
}

/// A run ready for the effect loop, whose seed, start, and parse events are
/// already at the recorder.
#[derive(Debug)]
pub struct Prepared {
    /// The run, built over the prepared context.
    pub run: Run,
    /// The run's id at the recorder, begun and still open, for
    /// `Harness::run_to_end` to drive; the run's events, its parse events
    /// first, are read from the recorder under this id.
    pub run_id: RunId,
    /// The performers for the run: the caller's inference broker and timer
    /// beside the runner's own tool performer, the run's snapshot of the
    /// Host's Plugins.
    pub performers: Performers,
    /// The prompt's declared `output:` path, which the caller reads with
    /// [`read_output`](crate::files::read_output) once the run completes.
    pub output_path: Option<String>,
}

/// Why a run could not be prepared.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PrepareError {
    /// The prompt does not parse. The run is ended as failed; the parse
    /// failure is the source.
    #[error("the prompt does not parse")]
    Parse {
        /// The run, ended with this failure.
        run_id: RunId,
        /// The parse failure.
        #[source]
        source: ParseError,
    },
    /// The environment cannot satisfy the prompt: a Plugin the prompt
    /// declares or slots is missing, unavailable, or lacks a service it
    /// needs, its Plugin does not offer a slotted tool, or the current
    /// model falls short of a role's requirements. The Engine's
    /// model-readable notice, one line per gap, is the source; the run is
    /// ended as failed with that notice, or as cancelled when the run's
    /// cancel had fired.
    #[error("the environment cannot satisfy the prompt")]
    Refused {
        /// The run, ended with this refusal.
        run_id: RunId,
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
        /// Why the input could not be put in place.
        #[source]
        source: InputFileError,
    },
    /// The recorder refused a write; the run cannot be recorded, so it is
    /// not prepared. `run` is the run the recorder issued, `None` when it
    /// refused to begin one.
    #[error("the run could not be recorded")]
    Recorder {
        /// The run the recorder issued before it refused.
        run: Option<RunId>,
        /// The recorder's refusal.
        #[source]
        source: RecorderError,
    },
}

impl PrepareError {
    /// The run this failure ended at the recorder, beside the failed
    /// outcome it was ended with, or, for a recorder refusal, which ends
    /// nothing, the run the recorder issued and its refusal. A refusal
    /// that [`prepare_noting_cancel`] says it ended `Cancelled` was not
    /// ended as failed, and its caller reports it without asking here.
    pub(crate) fn ended(self) -> Result<(RunId, RunOutcome), (Option<RunId>, RecorderError)> {
        match self {
            PrepareError::Parse { run_id, source } => Ok((run_id, failed("Parse", &source))),
            PrepareError::Refused { run_id, error } => Ok((run_id, failed_outcome(&error))),
            PrepareError::Input { run_id, source } => Ok((run_id, failed("Input", &source))),
            PrepareError::Recorder { run, source } => Err((run, source)),
        }
    }
}

/// Prepares the prompt `source` for one run with `args`: draws the run's
/// seed and start and begins the run at its recorder, parses the prompt,
/// puts its declared input file in place, waits for every built Plugin
/// to be ready or for the run's cancel, takes the run's snapshot of the
/// Host's Plugins, prepares the context, refuses an unsatisfiable prompt,
/// and builds the `Run` and its performers.
///
/// A Plugin whose `ready` fails is unavailable to this run, with its
/// error as the reason. A cancel ends the wait, and each Plugin is
/// snapshotted as it stands.
///
/// # Errors
/// Returns [`PrepareError::Parse`] when the source does not parse,
/// [`PrepareError::Input`] when its declared input file cannot be put in
/// place, and [`PrepareError::Refused`] when the environment cannot
/// satisfy it (in these three cases the run is ended as failed, except
/// that a refusal after the run's cancel fired ends it as cancelled, and
/// `Harness::run_to_end` reports it while the events recorded so far stay
/// at the recorder), and [`PrepareError::Recorder`] when the recorder
/// refuses a write.
pub async fn prepare(
    source: &str,
    args: &str,
    services: Services,
) -> Result<Prepared, PrepareError> {
    prepare_noting_cancel(source, args, services)
        .await
        .map_err(|(error, _)| error)
}

/// [`prepare`], whose failure comes beside whether it ended the run
/// `Cancelled`, which only a refusal after the run's cancel fired does.
/// That one reading of the cancel decides the record, and the caller
/// reports from it rather than reading the cancel again, so a cancel
/// landing while the refusal is recorded cannot split the report from
/// the record.
#[expect(
    clippy::result_large_err,
    reason = "the error is returned once per launch and carries the failure's source beside the one cancel reading"
)]
pub(crate) async fn prepare_noting_cancel(
    source: &str,
    args: &str,
    services: Services,
) -> Result<Prepared, (PrepareError, bool)> {
    let Services {
        host,
        services,
        vfs,
        input_text,
        cancel,
        recorder,
        broker,
        timer,
        name,
        model,
        ui,
    } = services;

    // The inputs the Engine leaves to the Harness, recorded before the
    // run exists so the record has them however the run ends.
    let seed: u64 = rand::random();
    let started_at = now_timestamp();
    let run_id = recorder
        .begin_run(RunMeta {
            name: name.clone(),
            prompt_hash: prompt_hash(source),
            seed,
            flags: 0,
            started_at: started_at.unix_millis(),
        })
        .await
        .map_err(|source| (PrepareError::Recorder { run: None, source }, false))?;
    let recorded = |source| {
        (
            PrepareError::Recorder {
                run: Some(run_id),
                source,
            },
            false,
        )
    };

    // Parse-time events are the run's first records, whether or not the
    // parse succeeds.
    let (prompt, parse_events) = Prompt::parse(source, &name);
    for event in &parse_events {
        let record = event_record(event).map_err(recorded)?;
        recorder.append(run_id, record).await.map_err(recorded)?;
    }
    let prompt = match prompt {
        Ok(prompt) => prompt,
        Err(source) => {
            recorder
                .end_run(run_id, failed("Parse", &source))
                .await
                .map_err(recorded)?;
            return Err((PrepareError::Parse { run_id, source }, false));
        }
    };

    // The declared input is in place before anything else sees the
    // store: the run's first section may read it.
    let declared = prompt.frontmatter().input().map(FileDecl::path);
    if let Err(source) = stage_input(&vfs, declared, input_text) {
        recorder
            .end_run(run_id, failed("Input", &source))
            .await
            .map_err(recorded)?;
        return Err((PrepareError::Input { run_id, source }, false));
    }
    let output_path = prompt
        .frontmatter()
        .output()
        .map(|decl| decl.path().to_owned());

    // The parse events were stamped under task `0` from zero; the run's
    // root task continues the sequence past them, so `(task_id, task_seq)`
    // is unique across every record of the run.
    let provenance_start = u32::try_from(parse_events.len()).unwrap_or(u32::MAX);
    let mut ctx = RunContext::new(name, seed, started_at)
        .cancel(cancel.clone())
        .provenance_start(provenance_start);
    if let Some(ui) = ui {
        ctx = ctx.ui(ui);
    }
    if let Some(model) = model {
        ctx = ctx.model(model);
    }
    // The wait-snapshot-prepare-refuse ceremony: the run's whole
    // filesystem, the real directories and the declared store, goes to
    // the context as given; the snapshot is taken once every Plugin is
    // ready or the run is cancelled, its catalog is what prepare fills
    // slots against, its preludes go to every section VM, and the
    // snapshot itself stays here as the tool performer, lending each call
    // the run's services. A refusal after a cancel ends the run cancelled,
    // as `Harness::run_to_end` reports it.
    let ctx = ctx.vfs(vfs);
    let failures = host.wait_ready(&cancel).await;
    let (snapshot, env, unmet) = host.begin_run(services, failures, &prompt);
    let (ctx, mut requirements) = env.prepare(&prompt, ctx);
    requirements.merge(unmet);
    if let Some(error) = requirements.refusal() {
        let cancelled = cancel.is_cancelled();
        let outcome = if cancelled {
            RunOutcome::Cancelled
        } else {
            failed_outcome(&error)
        };
        recorder.end_run(run_id, outcome).await.map_err(recorded)?;
        return Err((PrepareError::Refused { run_id, error }, cancelled));
    }

    let run = Run::new(Arc::new(prompt), args, ctx);
    let performers = Performers {
        broker,
        tool: Arc::new(snapshot),
        timer,
    };
    Ok(Prepared {
        run,
        run_id,
        performers,
        output_path,
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
