//! The Harness's half of a prompt's declared store files: the launch's input
//! text staged at the frontmatter's `input:` path before a run, and the
//! `output:` path read after it.
//!
//! Both go through the handle's store view ([`VfsRef::acquire_store`]),
//! so the store's strict path rules apply to the declared paths, which
//! the parser takes as written, and the handle's policy and op sink see
//! each operation like any other. Both are synchronous, like the VFS, and
//! the Harness runs them inline in the run's future.

use promptforge::vfs::{Origin, VfsError, VfsOp, VfsOutcome, VfsRef, perform_vfs_op};

use crate::recorder::RunOutcome;

/// Why a run's report has no output text. A missing output never fails
/// the run: the prompt's run succeeded, and only its declared output file
/// is absent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OutputError {
    /// The run did not complete: it failed, was cancelled, or ended
    /// before it began, so its output file is not read.
    #[error("the run did not complete")]
    NotCompleted,
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

/// Why a run's declared input file could not be put in place.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InputFileError {
    /// The launch supplied input text, but the prompt declares no
    /// `input:` file to stage it at.
    #[error("the launch supplied input text, but the prompt declares no `input:` file")]
    Undeclared,
    /// The prompt declares an input file that the launch did not supply
    /// and the store does not already hold.
    #[error(
        "the prompt declares the input file `{path}`, but the launch supplied no input text \
         and the store has no such file"
    )]
    Missing {
        /// The declared input path.
        path: String,
    },
    /// The store refused the staging write or the existence check.
    #[error("the input file `{path}` could not be staged")]
    Vfs {
        /// The declared input path.
        path: String,
        /// The store's failure.
        #[source]
        source: VfsError,
    },
}

/// Puts the prompt's declared input in place in `vfs`'s store: writes
/// `text` at `declared`, overwriting any file there, or, when the launch
/// supplied no text, checks that the store already holds the file.
///
/// # Errors
/// Returns [`InputFileError::Undeclared`] for text the prompt declares no
/// file for, [`InputFileError::Missing`] for a declared file that is
/// neither supplied nor present, and [`InputFileError::Vfs`] when the
/// store refuses the write or the check.
pub fn stage_input(
    vfs: &VfsRef,
    declared: Option<&str>,
    text: Option<String>,
) -> Result<(), InputFileError> {
    let path = match (declared, &text) {
        (None, None) => return Ok(()),
        (None, Some(_)) => return Err(InputFileError::Undeclared),
        (Some(path), _) => path,
    };
    let store = |source| InputFileError::Vfs {
        path: path.to_owned(),
        source,
    };
    let view = vfs
        .acquire_store(Origin::new(format!("input: {path}")))
        .map_err(store)?;
    let path = path.to_owned();
    match text {
        Some(contents) => perform_vfs_op(&view, VfsOp::Write { path, contents })
            .map(drop)
            .map_err(store),
        None => match perform_vfs_op(&view, VfsOp::Exists { path: path.clone() }) {
            Ok(VfsOutcome::Bool(true)) => Ok(()),
            Ok(_) => Err(InputFileError::Missing { path }),
            Err(source) => Err(store(source)),
        },
    }
}

/// Reads the prompt's declared output file at `path` from `vfs`'s store.
///
/// # Errors
/// Returns the store's failure, [`VfsError::NotFound`] when the run never
/// wrote the file.
pub fn read_output(vfs: &VfsRef, path: &str) -> Result<String, VfsError> {
    let view = vfs.acquire_store(Origin::new(format!("output: {path}")))?;
    let read = VfsOp::Read {
        path: path.to_owned(),
        start: None,
        end: None,
    };
    match perform_vfs_op(&view, read)? {
        VfsOutcome::Text(text) => Ok(text),
        other => Err(VfsError::Backend {
            message: format!("a whole-file store read answered {other:?} instead of text"),
        }),
    }
}

/// The output a run's report carries: what a completed run left at its
/// declared `output:` path in `vfs`'s store, and
/// [`OutputError::NotCompleted`] for every other outcome.
pub(crate) fn report_output(
    vfs: &VfsRef,
    path: Option<&str>,
    outcome: &RunOutcome,
) -> Result<String, OutputError> {
    if !matches!(outcome, RunOutcome::Completed { .. }) {
        return Err(OutputError::NotCompleted);
    }
    let path = path.ok_or(OutputError::Undeclared)?;
    read_output(vfs, path).map_err(|error| match error {
        VfsError::NotFound { .. } => OutputError::Missing {
            path: path.to_owned(),
        },
        source => OutputError::Vfs {
            path: path.to_owned(),
            source,
        },
    })
}
