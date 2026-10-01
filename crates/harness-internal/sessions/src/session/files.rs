//! A session's declared files: the filesystem every run of the session
//! works in, the launch's text for the prompt's `input:` file, and what
//! the completed run left at its `output:` file, which
//! [`Session::output_text`] returns.
//!
//! The output is read once, as the run completes and before the session
//! reports `Closed`, so a client that awaits `Closed` and then asks for
//! it never races the read, and a Host that tears its filesystem down
//! afterwards keeps the text.

use std::sync::{Mutex, PoisonError};

use harness_runner::files::read_output;
use harness_runner::spawn::spawn_blocking_launch;
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
    Store {
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

    /// Reads the completed run's declared output file at `path` from
    /// `vfs` on the blocking pool, tagged with `agent`, and keeps what it
    /// finds for [`Session::output_text`].
    pub(crate) async fn collect(&self, agent: &str, vfs: VfsRef, path: Option<String>) {
        let output = match path {
            None => Err(OutputError::Undeclared),
            Some(path) => {
                let read_path = path.clone();
                let read = spawn_blocking_launch(agent, move || read_output(&vfs, &read_path))
                    .await
                    .unwrap_or_else(|join| {
                        Err(VfsError::Backend {
                            message: format!("the output read failed: {join}"),
                        })
                    });
                match read {
                    Ok(text) => Ok(text),
                    Err(VfsError::NotFound { .. }) => Err(OutputError::Missing { path }),
                    Err(source) => Err(OutputError::Store { path, source }),
                }
            }
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
    /// [`OutputError::Store`] when the store refused the read.
    pub fn output_text(&self) -> Result<String, OutputError> {
        self.core
            .files
            .output
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
