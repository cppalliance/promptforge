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

/// Drives one run of a prompt for a Host.
///
/// It holds what the Host supplies for the run: a recorder, an inference
/// broker, a timer, a capability registry, and services. [`Harness::run`]
/// consumes it.
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

/// The inputs to one run: the prompt's source and arguments, its declared
/// input text, the filesystem it works in, and the Host state it reads.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// The run's name. It becomes the `execution` field of every event and
    /// the `name` field of the run metadata.
    pub name: String,
    /// The prompt's Markdown source. The Harness reads the prompt only
    /// from this text.
    pub source: String,
    /// The run's argument text.
    pub args: String,
    /// Text written to the prompt's declared `input:` file before the run
    /// starts.
    pub input_text: Option<String>,
    /// The run's whole filesystem: the store that holds the prompt's
    /// declared files, plus any real directories mounted into it.
    pub vfs: VfsRef,
    /// The Host state the run reads: its selected model and the workspace
    /// roots it has granted. The run resolves its model from the
    /// selection, and the prompt's `ui()` global serves this state.
    pub host: HostSnapshot,
}

/// How one run ended, and what it left at its declared output file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// The id the recorder issued for the run. `None` when a cancel ended
    /// the run before the recorder began it.
    pub run_id: Option<RunId>,
    /// How the run ended. When `run_id` is `Some`, the recorder holds this
    /// same outcome.
    pub outcome: RunOutcome,
    /// The text a completed run left at its declared `output:` file, or
    /// the reason the report omits that text.
    pub output: Result<String, OutputError>,
}

/// An error that stopped a run before `Harness::run` could report an
/// outcome.
///
/// `Harness::run` returns it as its `Err` value.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// The Harness failed to resolve the Host's current model through the
    /// inference broker. The resolution failure is the error's source. The
    /// run failed to begin.
    #[error("the run's model could not be resolved")]
    Model(#[source] CurrentModelError),
    /// The recorder refused a write, so the Harness stops driving the run.
    #[error("the run could not be recorded")]
    Recorder {
        /// The id of the run the recorder issued before it refused. `None`
        /// when the recorder refused to begin the run.
        run: Option<RunId>,
        /// The error the recorder returned.
        #[source]
        source: RecorderError,
    },
    /// The run was still pending, but its set of effects in flight was
    /// empty. This means the Harness lost track of an effect.
    #[error("the effect loop has nothing to await for a pending run")]
    Stalled,
}

impl Harness {
    /// Creates a Harness for one run.
    ///
    /// The Harness records the run through `recorder`. It resolves the
    /// run's model and gets the model's replies through `broker`. It sleeps
    /// through `timer`. It activates the capabilities the prompt declares
    /// from `capabilities` and hands them `services`. All of this work
    /// happens in [`Harness::run`].
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

    /// Returns the control that steers this Harness's run from outside its
    /// future.
    ///
    /// Take it before calling [`Harness::run`], which consumes the Harness.
    #[must_use]
    pub fn control(&self) -> RunControl {
        self.control.clone()
    }

    /// Runs `request` to its end and reports how it ended.
    ///
    /// It resolves the run's model through the inference broker, prepares
    /// the prompt from its source, drives the run, and reads the declared
    /// output file if the run completed. The returned future is `Send`. Its
    /// only runtime needs are those of the Host's performers.
    ///
    /// Three problems end the run as failed and still return a report with
    /// that outcome: a source that fails to parse, a declared input file
    /// the Harness fails to put in place, and an environment that falls
    /// short of the prompt's requirements. A cancel raised before calling
    /// `run`, or while the broker lists its models, returns a report with
    /// the outcome `Cancelled` and `run_id` set to `None`.
    ///
    /// # Errors
    /// Returns [`HarnessError::Model`] when the broker fails to list its
    /// models or its list omits the selected one. Returns
    /// [`HarnessError::Recorder`] when the recorder refuses a write.
    /// Returns [`HarnessError::Stalled`] when the run is pending and its
    /// set of effects in flight is empty.
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
