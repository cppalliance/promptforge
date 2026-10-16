//! Node metadata reported by backends: the file kind, its stat record,
//! and the directory entry that pairs a name with one.

use std::time::SystemTime;

/// The kind of node at a path, one of the seven POSIX file kinds.
///
/// Every kind has its own variant, so a backend that serves a special
/// node, such as a virtual `/dev/null`, can report it as a `CharDevice`.
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
/// A backend fills an optional field only with a value it tracks, and
/// leaves it `None` otherwise, so that a reported time stays
/// deterministic and a sort by time, such as `ls -t`, stays meaningful.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

impl Stat {
    /// Returns metadata for a node of `file_type` holding `size` bytes,
    /// with no mode bits or times.
    #[must_use]
    pub fn new(file_type: FileType, size: u64) -> Stat {
        Stat {
            file_type,
            size,
            mode: None,
            modified: None,
            created: None,
        }
    }

    /// Sets the POSIX mode bits.
    #[must_use]
    pub fn with_mode(mut self, mode: Option<u32>) -> Stat {
        self.mode = mode;
        self
    }

    /// Sets the last modification time.
    #[must_use]
    pub fn with_modified(mut self, modified: Option<SystemTime>) -> Stat {
        self.modified = modified;
        self
    }

    /// Sets the creation time.
    #[must_use]
    pub fn with_created(mut self, created: Option<SystemTime>) -> Stat {
        self.created = created;
        self
    }
}

/// One entry in a directory listing, pairing a name with its metadata.
///
/// The `description` field holds an optional annotation shown beside the
/// entry. The built-in backends, `MemoryBackend` and `RealBackend`, leave
/// it `None`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    /// The entry's name within its directory.
    pub name: String,
    /// The entry's metadata.
    pub stat: Stat,
    /// Optional annotation shown beside the entry.
    pub description: Option<String>,
}

impl Entry {
    /// Returns the entry `name` with metadata `stat` and no description.
    #[must_use]
    pub fn new(name: impl Into<String>, stat: Stat) -> Entry {
        Entry {
            name: name.into(),
            stat,
            description: None,
        }
    }

    /// Sets the annotation shown beside the entry.
    #[must_use]
    pub fn with_description(mut self, description: Option<String>) -> Entry {
        self.description = description;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{Entry, FileType, Stat};

    #[test]
    fn the_constructors_leave_every_optional_field_unset_until_a_setter_fills_it() {
        let bare = Stat::new(FileType::File, 3);
        assert_eq!(
            (
                bare.file_type,
                bare.size,
                bare.mode,
                bare.modified,
                bare.created
            ),
            (FileType::File, 3, None, None, None)
        );
        let when = SystemTime::UNIX_EPOCH;
        let full = bare
            .clone()
            .with_mode(Some(0o644))
            .with_modified(Some(when))
            .with_created(Some(when));
        assert_eq!(
            (full.mode, full.modified, full.created),
            (Some(0o644), Some(when), Some(when))
        );
        assert_ne!(full, bare, "the setters changed the record");
        let entry = Entry::new("a.txt", bare.clone());
        assert_eq!(entry.description, None);
        let described = entry.clone().with_description(Some("notes".to_owned()));
        assert_eq!(described.description.as_deref(), Some("notes"));
        assert_eq!(entry, Entry::new("a.txt", bare));
    }
}
