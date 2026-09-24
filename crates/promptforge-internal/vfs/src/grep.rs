//! The grep exchange with backends: one request against the namespace and
//! the hits it returns.

use crate::path::VfsPathBuf;

/// One grep request against the namespace.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GrepQuery {
    /// The text or pattern to search for.
    pub pattern: String,
    /// The directory the search is rooted at.
    pub root: VfsPathBuf,
    /// Whether `pattern` is a regular expression.
    pub is_regex: bool,
    /// Whether matching ignores case.
    pub case_insensitive: bool,
    /// An optional glob restricting which files are searched.
    pub glob_filter: Option<String>,
    /// An optional cap on returned matches.
    pub max_results: Option<usize>,
}

/// One grep hit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    /// The path of the file containing the hit.
    pub path: String,
    /// The 1-based line number of the hit.
    pub line_number: usize,
    /// The full text of the matching line.
    pub line: String,
}

/// The outcome of one grep request.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrepResults {
    /// The hits, in backend order.
    pub matches: Vec<GrepMatch>,
    /// Whether `max_results` cut the result set short.
    pub truncated: bool,
}
