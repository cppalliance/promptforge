//! Node metadata reported by backends: the file kind, its stat record,
//! and the directory entry that pairs a name with one.

use std::time::SystemTime;

/// The kind of node at a path, one of the seven POSIX file kinds.
///
/// Every kind has its own variant. There is no catch-all variant, so a
/// backend that serves a special node, such as a virtual `/dev/null`, can
/// report it as a `CharDevice`.
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
/// A backend that does not track an optional field leaves it `None`
/// instead of inventing a value. An invented modification time would be
/// nondeterministic, and a constant one would make a sort by time, such
/// as `ls -t`, meaningless.
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

/// One entry in a directory listing, pairing a name with its metadata.
///
/// The `description` field holds an optional annotation shown beside the
/// entry. The built-in backends, `MemoryBackend` and `RealBackend`, leave
/// it `None`.
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
