//! The per-run Harness: one object per run over everything the Host
//! supplies, which drives that one run as a single future.
//!
//! A Host builds a [`Harness`] from its recorder, inference broker, timer,
//! capability registry, and services, takes the run's [`RunControl`], and
//! awaits [`Harness::run`] on whatever executor it likes. The run resolves
//! the launch model through the broker, prepares the request's source,
//! drives the run through the effect loop, and reads the declared output
//! file of a completed run. Every performer future is polled inside that
//! one future, and every VFS operation runs inline in it, so the Harness
//! starts no task and names no runtime.

use std::pin::pin;
use std::sync::Arc;

use futures_util::future::{Either, select};
use harness_capabilities::{CapabilityRegistry, HostServices};
use promptforge::cancel::CancelHandle;
use promptforge::vfs::VfsRef;

use crate::effect_loop::{DriveError, drive};
use crate::environment::{CurrentModelError, HostSnapshot, current_model};
use crate::files::{OutputError, report_output};
use crate::performers::{InferenceBroker, Timer};
use crate::prepare::{Services, prepare};
use crate::recorder::{RecorderError, RunId, RunOutcome, RunRecorder};

#[path = "harness-control.rs"]
mod control;

pub use control::RunControl;
pub(crate) use control::StopSignal;

/// Drives one run for a Host: holds the Host's recorder, broker, timer,
/// capabilities, and services, and consumes itself in [`Harness::run`].
pub struct Harness {
    recorder: Arc<dyn RunRecorder>,
    broker: Arc<dyn InferenceBroker>,
    timer: Arc<dyn Timer>,
    capabilities: CapabilityRegistry,
    services: HostServices,
    control: RunControl,
}

impl std::fmt::Debug for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Harness")
            .field("capabilities", &self.capabilities)
            .field("services", &self.services)
            .field("control", &self.control)
            .finish_non_exhaustive()
    }
}

/// What to run: the prompt's source and arguments, its declared input
/// text, the filesystem it works in, and the Host state it reads.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// The run's name: every event's `execution` and the run metadata's
    /// `name`.
    pub name: String,
    /// The prompt's Markdown source: the Harness's one prompt input.
    pub source: String,
    /// The run's argument text.
    pub args: String,
    /// Text staged at the prompt's declared `input:` file before the run.
    pub input_text: Option<String>,
    /// The run's whole filesystem: its declared store and any real mounts.
    pub vfs: VfsRef,
    /// The Host's selection and granted roots, which the run binds its
    /// model from and the `ui()` global serves.
    pub host: HostSnapshot,
}

/// How one run ended, and what it left at its declared output file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// The run's id at the recorder; `None` when a cancel ended the run
    /// before the recorder began it.
    pub run_id: Option<RunId>,
    /// How the run ended, as the recorder was told.
    pub outcome: RunOutcome,
    /// The text a completed run left at its declared `output:` file.
    pub output: Result<String, OutputError>,
}

/// Why a run ended without an outcome of its own.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// The Host's current model could not be resolved through its broker;
    /// the resolution failure is the source. The run never began.
    #[error("the run's model could not be resolved")]
    Model(#[source] CurrentModelError),
    /// The recorder refused a write, so the run is not driven further.
    /// `run` is the run the recorder issued, `None` when it refused to
    /// begin one.
    #[error("the run could not be recorded")]
    Recorder {
        /// The run the recorder issued before it refused.
        run: Option<RunId>,
        /// The recorder's refusal.
        #[source]
        source: RecorderError,
    },
    /// The run is pending with nothing in flight, which means the Harness
    /// lost an effect.
    #[error("the effect loop has nothing to await for a pending run")]
    Stalled,
}

impl Harness {
    /// A Harness for one run that records through `recorder`, performs
    /// model rounds and resolves the run's model through `broker`, sleeps
    /// through `timer`, activates the prompt's capabilities from
    /// `capabilities`, and hands `services` to them. Nothing runs until
    /// [`Harness::run`].
    #[must_use]
    pub fn new(
        recorder: Arc<dyn RunRecorder>,
        broker: Arc<dyn InferenceBroker>,
        timer: Arc<dyn Timer>,
        capabilities: CapabilityRegistry,
        services: HostServices,
    ) -> Harness {
        Harness {
            recorder,
            broker,
            timer,
            capabilities,
            services,
            control: RunControl::new(CancelHandle::new()),
        }
    }

    /// The control that steers this Harness's run. Take it before
    /// [`Harness::run`], which consumes the Harness.
    #[must_use]
    pub fn control(&self) -> RunControl {
        self.control.clone()
    }

    /// Runs `request` to its end: resolves the launch model, prepares the
    /// source, drives the run, and reads the declared output file of a
    /// completed run. The future is `Send` and needs no runtime of its
    /// own.
    ///
    /// A source that does not parse, a declared input that cannot be put
    /// in place, and an environment that cannot satisfy the prompt each
    /// end the run as failed, reported with its outcome. A cancel before
    /// `run` or while the broker lists its models reports `Cancelled` with
    /// no run.
    ///
    /// # Errors
    /// Returns [`HarnessError::Model`] when the broker cannot list its
    /// models or lacks the selected one, [`HarnessError::Recorder`] when
    /// the recorder refuses a write, and [`HarnessError::Stalled`] when the
    /// run pends with nothing in flight.
    pub async fn run(self, request: RunRequest) -> Result<RunReport, HarnessError> {
        Box::pin(self.run_to_end(request)).await
    }

    async fn run_to_end(self, request: RunRequest) -> Result<RunReport, HarnessError> {
        let Harness {
            recorder,
            broker,
            timer,
            capabilities,
            services,
            control,
        } = self;
        let RunRequest {
            name,
            source,
            args,
            input_text,
            vfs,
            host,
        } = request;

        let model = {
            let resolving = pin!(current_model(&host, &*broker));
            // The cancel arm is polled first, so a cancel raised before the
            // run, or before a listing that is ready at once, wins.
            match select(control.cancel_handle().cancelled(), resolving).await {
                Either::Left(((), _)) => {
                    return Ok(RunReport {
                        run_id: None,
                        outcome: RunOutcome::Cancelled,
                        output: Err(OutputError::NotCompleted),
                    });
                }
                Either::Right((resolved, _)) => resolved.map_err(HarnessError::Model)?,
            }
        };

        let services = Services {
            registry: Some(Arc::new(capabilities)),
            services,
            vfs: vfs.clone(),
            input_text,
            cancel: control.cancel_handle().clone(),
            recorder: Arc::clone(&recorder),
            broker,
            timer,
            name,
            model,
            ui: Some(host.ui()),
        };
        let prepared = match prepare(&source, &args, services).await {
            Ok(prepared) => prepared,
            Err(error) => {
                return match error.ended() {
                    Ok((run_id, outcome)) => Ok(RunReport {
                        run_id: Some(run_id),
                        outcome,
                        output: Err(OutputError::NotCompleted),
                    }),
                    Err((run, source)) => Err(HarnessError::Recorder { run, source }),
                };
            }
        };

        let run_id = prepared.run_id;
        let outcome = drive(
            prepared.run,
            prepared.performers,
            recorder,
            run_id,
            control.cancel_handle().clone(),
            Arc::clone(control.stop_signal()),
        )
        .await
        .map_err(|error| match error {
            DriveError::Recorder(source) => HarnessError::Recorder {
                run: Some(run_id),
                source,
            },
            DriveError::Stalled => HarnessError::Stalled,
        })?;
        let output = report_output(&vfs, prepared.output_path.as_deref(), &outcome);
        Ok(RunReport {
            run_id: Some(run_id),
            outcome,
            output,
        })
    }
}
