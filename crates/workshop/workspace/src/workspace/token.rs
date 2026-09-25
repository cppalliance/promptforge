//! The conflict token a file read hands the client and a file write must
//! echo back: modified time plus length when the filesystem reports a
//! usable mtime, a content hash otherwise.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::MAX_FILE_BYTES;

/// The mtime half of the conflict token: full-precision modified time in
/// nanoseconds since the Unix epoch plus the byte length. `None` when the
/// filesystem reports no usable modified time, which callers cover with
/// [`hash_token`] - collapsing the error to a constant would make every
/// token on such a filesystem equal and no write would ever conflict.
pub(super) fn mtime_token(metadata: &fs::Metadata) -> Option<String> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!("{}-{}", duration.as_nanos(), metadata.len()))
}

/// The content-hash fallback token for filesystems without modified times.
/// `DefaultHasher` is stable within one process run, which is all a token
/// needs: a restart invalidates outstanding tokens toward conflict, never
/// toward a silent overwrite.
pub(super) fn hash_token(contents: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    format!("h-{:016x}", hasher.finish())
}

/// A file's opaque conflict token from its metadata and already-read
/// contents: the mtime form when available, otherwise the hash form.
pub(super) fn file_token(metadata: &fs::Metadata, contents: &[u8]) -> String {
    mtime_token(metadata).unwrap_or_else(|| hash_token(contents))
}

/// The current on-disk token of an existing write target, reading the file
/// only when the hash fallback demands it. `None` means no token could be
/// derived - an unreadable or oversized file - and the caller must refuse
/// the write rather than overwrite unverified contents.
pub(super) fn current_token(path: &Path, metadata: &fs::Metadata) -> Option<String> {
    if let Some(token) = mtime_token(metadata) {
        return Some(token);
    }
    if metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read(path).ok().map(|bytes| hash_token(&bytes))
}
