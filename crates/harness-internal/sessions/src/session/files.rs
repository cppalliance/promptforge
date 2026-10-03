//! A session's declared files: the filesystem every run of the session
//! works in, the launch's text for the prompt's `input:` file, and what
//! the completed run left at its `output:` file, which
//! [`Session::output_text`] returns.
//!
//! The output is the one each run's report carries, read as the run
//! completes and kept before the session reports `Closed`, so a client
//! that awaits `Closed` and then asks for it never races the read, and a
//! Host that tears its filesystem down afterwards keeps the text.

use std::sync::{Mutex, PoisonError};

use harness_runner::files::OutputError as ReportedOutput;
use promptforge::vfs::{VfsError, VfsRef};

use super::Session;

/// Why a session has no output text to return. A missing output never
/// fails the run: the prompt's run succeeded, and only its declared
/// output file is absent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OutputError {
    /// No run of the session has completed: it is still running, or it
    /// failed, was cancelled, or was closed first.
    #[error("no run of the session has completed")]
    Unfinished,
    /// The prompt declares no `output:` file.
    #[error("the prompt declares no `output:` file")]
    Undeclared,
    /// The run completed without writing its declared output file.
    #[error("the run completed without writing its declared output file `{path}`")]
    Missing {
        /// The declared output path.
        path: String,
    },
    /// The store refused the read of the declared output file.
    #[error("the output file `{path}` could not be read")]
    Vfs {
        /// The declared output path.
        path: String,
        /// The store's failure.
        #[source]
        source: VfsError,
    },
}

/// The per-session state behind the declared files.
pub(crate) struct SessionFiles {
    /// The launch's filesystem, shared by every run; `None` gives each
    /// run a fresh memory store.
    vfs: Option<VfsRef>,
    /// The launch's text for the prompt's declared input file.
    input_text: Option<String>,
    /// What the completed run left at the declared output file.
    output: Mutex<Result<String, OutputError>>,
}

impl SessionFiles {
    /// The files of a session launched over `vfs` with `input_text`.
    pub(crate) fn new(vfs: Option<VfsRef>, input_text: Option<String>) -> Self {
        Self {
            vfs,
            input_text,
            output: Mutex::new(Err(OutputError::Unfinished)),
        }
    }

    /// The filesystem one run works in: the launch's handle, or a fresh
    /// memory store at `/`.
    pub(crate) fn run_vfs(&self) -> VfsRef {
        self.vfs.clone().unwrap_or_default()
    }

    /// The launch's text for the prompt's declared input file.
    pub(crate) fn input_text(&self) -> Option<String> {
        self.input_text.clone()
    }

    /// Keeps what a run's report carries at its declared output file for
    /// [`Session::output_text`]. A run that did not complete leaves
    /// whatever an earlier run of the session left.
    pub(crate) fn collect(&self, reported: Result<String, ReportedOutput>) {
        let output = match reported {
            Ok(text) => Ok(text),
            Err(ReportedOutput::Undeclared) => Err(OutputError::Undeclared),
            Err(ReportedOutput::Missing { path }) => Err(OutputError::Missing { path }),
            Err(ReportedOutput::Vfs { path, source }) => Err(OutputError::Vfs { path, source }),
            // `NotCompleted`, or a reason the runner adds behind its
            // `#[non_exhaustive]` enum: no completed run's output to keep.
            Err(_) => return,
        };
        *self.output.lock().unwrap_or_else(PoisonError::into_inner) = output;
    }
}

impl Session {
    /// The text the session's completed run left at the prompt's declared
    /// `output:` file. It is read as the run completes, before the
    /// session reports `Closed`, so await `Closed` and then call this.
    ///
    /// # Errors
    /// Returns [`OutputError::Unfinished`] until a run completes (and for
    /// good when it failed, was cancelled, or was closed first),
    /// [`OutputError::Undeclared`] for a prompt with no `output:` file,
    /// [`OutputError::Missing`] when the run never wrote it, and
    /// [`OutputError::Vfs`] when the store refused the read.
    pub fn output_text(&self) -> Result<String, OutputError> {
        self.core
            .files
            .output
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
