//! Error types for the MCP server.

use std::fmt;
use std::path::{Path, PathBuf};

/// A `prompts.toml` load failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {path}")]
    Read {
        /// The path that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML, or a value had the wrong shape.
    #[non_exhaustive]
    #[error("parse config: {0}")]
    Parse(String),

    /// A `${VAR}` referenced an environment variable that was not set.
    #[non_exhaustive]
    #[error("unresolved environment variable {0}")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),
}

/// One thing wrong with a resolved catalog, named as precisely as the pass can
/// name it.
///
/// A fault carries the prompt it is about and the file it came from wherever
/// either is known: a prompt that would not parse has both, a stale override
/// has only the name its block was keyed on, and an empty catalog has neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    prompt: Option<String>,
    path: Option<PathBuf>,
    detail: String,
}

impl Fault {
    /// Builds a fault from whatever the pass could name.
    pub(crate) fn new(
        prompt: Option<String>,
        path: Option<PathBuf>,
        detail: impl Into<String>,
    ) -> Fault {
        Fault {
            prompt,
            path,
            detail: detail.into(),
        }
    }

    /// The prompt the fault is about, when the pass got far enough to name one.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    /// The file the fault is about, when one is known.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What is wrong, as a lowercase noun phrase.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.prompt, &self.path) {
            (Some(prompt), Some(path)) => {
                write!(f, "{prompt} ({}): {}", path.display(), self.detail)
            }
            (Some(prompt), None) => write!(f, "{prompt}: {}", self.detail),
            (None, Some(path)) => write!(f, "{}: {}", path.display(), self.detail),
            (None, None) => f.write_str(&self.detail),
        }
    }
}

/// Everything wrong with one resolution pass, accumulated.
///
/// The pass runs to completion rather than stopping at the first problem, so an
/// operator fixing a configuration sees every fault in one go instead of one per
/// restart. `Display` writes the count and then one indented line per fault.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogError {
    faults: Vec<Fault>,
}

impl CatalogError {
    /// Collects faults into an error. Never called with an empty list, since a
    /// pass with no faults returns the catalog.
    pub(crate) fn new(faults: Vec<Fault>) -> CatalogError {
        CatalogError { faults }
    }

    /// Every fault, in the order the pass found them.
    #[must_use]
    pub fn faults(&self) -> &[Fault] {
        &self.faults
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = if self.faults.len() == 1 { "" } else { "s" };
        write!(f, "catalog has {} fault{plural}", self.faults.len())?;
        for fault in &self.faults {
            write!(f, "\n  {fault}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CatalogError {}
