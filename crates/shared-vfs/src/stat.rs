//! Node metadata reported by backends: the file kind, its stat record,
//! and the directory entry that pairs a name with one.

use std::time::SystemTime;

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
