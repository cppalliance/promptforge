//! Catalog resolution faults and the accumulated [`CatalogError`].

use std::fmt;
use std::path::PathBuf;

/// One thing wrong with a resolved catalog, named as precisely as the pass can
/// name it.
///
/// A fault carries a machine-readable [`FaultKind`] set where it was raised, the
/// prompt it is about and the file it came from wherever either is known, and a
/// human-readable detail: a prompt that would not parse has a name and a path, a
/// stale override has only the name its block was keyed on, and an empty catalog
/// has neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fault {
    kind: FaultKind,
    prompt: Option<String>,
    path: Option<PathBuf>,
    detail: String,
}

impl Fault {
    /// Builds a fault, tagged with the class the raising site knows it to be
    /// rather than one inferred later from which fields happen to be set.
    pub(crate) fn new(
        kind: FaultKind,
        prompt: Option<String>,
        path: Option<PathBuf>,
        detail: impl Into<String>,
    ) -> Fault {
        Fault {
            kind,
            prompt,
            path,
            detail: detail.into(),
        }
    }
}

/// A stable, dependency-free classification of one [`FaultRef`].
///
/// The class is the actual reason resolution rejected the file or the catalog,
/// tagged where the fault was raised, so a caller can group, suppress, or
/// remediate faults without parsing the English detail. It is not inferred from
/// which locus fields are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FaultKind {
    /// An `[catalog].include` or `exclude` glob pattern would not expand.
    Pattern,
    /// A resolved file could not be read, or its canonical location escaped the
    /// prompts directory.
    Unreadable,
    /// A resolved file declared a `promptforge:` prompt but would not parse.
    Unparsable,
    /// A parsed prompt's frontmatter name is invalid, reserved, or does not
    /// match the `[prompts.NAME]` block that reached it.
    InvalidName,
    /// Two healthy prompts declare one name.
    Duplicate,
    /// A `[prompts.NAME]` block named neither a file nor any resolved prompt.
    StaleOverride,
    /// The pass resolved no prompts at all.
    Empty,
}

/// A borrowed view of one fault in a [`CatalogError`].
///
/// The fault's representation stays private; a caller reads it through these
/// accessors, so the crate is free to change how a fault is stored.
#[derive(Debug, Clone, Copy)]
pub struct FaultRef<'a> {
    inner: &'a Fault,
}

impl<'a> FaultRef<'a> {
    /// The prompt the fault is about, when the pass named one.
    #[must_use]
    pub fn prompt(&self) -> Option<&'a str> {
        self.inner.prompt.as_deref()
    }

    /// The file the fault is about, when one is known.
    #[must_use]
    pub fn path(&self) -> Option<&'a std::path::Path> {
        self.inner.path.as_deref()
    }

    /// What is wrong, in one line.
    #[must_use]
    pub fn detail(&self) -> &'a str {
        &self.inner.detail
    }

    /// This fault's stable classification, as tagged where it was raised.
    #[must_use]
    pub fn kind(&self) -> FaultKind {
        self.inner.kind
    }
}

impl fmt::Display for FaultRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.inner, f)
    }
}

/// An [`ExactSizeIterator`] over the faults in a [`CatalogError`].
///
/// A named iterator type rather than `impl Trait`, so the return type is part
/// of the API and does not silently change shape.
#[derive(Debug, Clone)]
pub struct Faults<'a> {
    iter: std::slice::Iter<'a, Fault>,
}

impl<'a> Iterator for Faults<'a> {
    type Item = FaultRef<'a>;

    fn next(&mut self) -> Option<FaultRef<'a>> {
        self.iter.next().map(|inner| FaultRef { inner })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl DoubleEndedIterator for Faults<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(|inner| FaultRef { inner })
    }
}

impl ExactSizeIterator for Faults<'_> {}

impl std::iter::FusedIterator for Faults<'_> {}

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
///
/// # Examples
/// A caller classifies with [`CatalogError::kind`] and walks each fault through
/// the borrowed [`FaultRef`] accessors, never touching a private field:
/// ```
/// use promptforge_mcp_server::{CatalogError, CatalogErrorKind};
///
/// fn report(err: &CatalogError) {
///     let severity = match err.kind() {
///         CatalogErrorKind::Broken => "a prompt or file failed to resolve",
///         CatalogErrorKind::Configuration => "the catalog as a whole is unusable",
///         _ => "unknown",
///     };
///     eprintln!("{severity}: {} fault(s)", err.faults().len());
///     for fault in err.faults() {
///         match (fault.prompt(), fault.path()) {
///             (Some(name), _) => eprintln!("  prompt {name}: {}", fault.detail()),
///             (None, Some(path)) => eprintln!("  file {}: {}", path.display(), fault.detail()),
///             (None, None) => eprintln!("  {}", fault.detail()),
///         }
///     }
/// }
/// ```
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
    ///
    /// The returned [`Faults`] iterator is [`ExactSizeIterator`], so a caller
    /// that only wants the count can call `.len()` without draining it.
    #[must_use]
    pub fn faults(&self) -> Faults<'_> {
        Faults {
            iter: self.faults.iter(),
        }
    }

    /// This error's stable classification.
    ///
    /// [`CatalogErrorKind::Broken`] when any fault names a prompt or a file,
    /// [`CatalogErrorKind::Configuration`] when every fault is catalog-level
    /// (an empty result, or an include or exclude pattern that would not
    /// compile).
    #[must_use]
    pub fn kind(&self) -> CatalogErrorKind {
        if self
            .faults
            .iter()
            .any(|fault| fault.prompt.is_some() || fault.path.is_some())
        {
            CatalogErrorKind::Broken
        } else {
            CatalogErrorKind::Configuration
        }
    }
}

/// A stable, dependency-free classification of a [`CatalogError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CatalogErrorKind {
    /// At least one prompt or file could not be resolved.
    Broken,
    /// The catalog as a whole is unusable: nothing resolved, or a pattern in
    /// `[catalog]` would not compile.
    Configuration,
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
