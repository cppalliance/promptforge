//! Value types exchanged with backends: entries, metadata, and grep.

use std::time::SystemTime;

use crate::path::VfsPathBuf;

/// The seven POSIX kinds, named rather than lumped: a virtual `/dev/null`
/// (char device) is a plausible backend, and an `Other` kind would hide it.
/// The engine adapter maps the first four directly and the three specials
/// to `File` with a trace.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// A named pipe.
    Fifo,
    /// A socket.
    Socket,
    /// A character device.
    CharDevice,
    /// A block device.
    BlockDevice,
}

/// Metadata for one path.
///
/// Options preserve honesty: a backend that does not track a field says
/// `None` rather than fabricating (an invented mtime is nondeterministic;
/// a constant one makes `ls -t` sort garbage).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Stat {
    /// What kind of node this is.
    pub file_type: FileType,
    /// Size in bytes.
    pub size: u64,
    /// POSIX mode bits, when the backend tracks them.
    pub mode: Option<u32>,
    /// Last modification time, when the backend tracks it.
    pub modified: Option<SystemTime>,
    /// Creation time, when the backend tracks it.
    pub created: Option<SystemTime>,
}

/// One directory entry.
///
/// `description` is the annotation column; it is `None` outside
/// `/_promptforge` and the engine adapter drops it. `Entry` is designed
/// to grow: annotations live here.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Entry {
    /// The entry's name within its directory.
    pub name: String,
    /// The entry's metadata.
    pub stat: Stat,
    /// Optional annotation shown beside the entry.
    pub description: Option<String>,
}

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
