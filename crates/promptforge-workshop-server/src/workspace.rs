//! Confined workspace filesystem access: directory trees, file reads, and
//! file writes jailed to roots explicitly granted through drag and drop.
//!
//! A dropped folder becomes a granted root; a dropped file grants its parent
//! directory. Grants live in memory for the running process only - profile
//! persistence is a separate future consent decision. Every request path is
//! checked lexically (no `..`, and on Windows no NTFS alternate data
//! stream names) and then
//! canonicalized and prefix-matched against the canonical grants before any
//! filesystem operation, so traversal, symlink escapes, and UNC aliases
//! cannot reach outside a grant. This is the same jail shape as the
//! gateway's artifact-cache `confine.rs`, with canonicalization performing
//! the resolution that module's component walk performs by hand.

use std::collections::BTreeSet;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::UNIX_EPOCH;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// The largest file the workspace reads or accepts for a write: the editor
/// targets source text, not media, so one MiB is generous.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A workspace operation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum WorkspaceError {
    /// A granted path could not be canonicalized.
    #[error("grant path cannot be resolved")]
    ResolveGrant {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A requested path could not be canonicalized.
    #[error("requested path cannot be resolved")]
    ResolvePath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// Filesystem metadata for a path could not be read.
    #[error("path cannot be inspected")]
    InspectPath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A directory could not be listed.
    #[error("directory cannot be listed")]
    ListDirectory {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A file could not be read.
    #[error("file cannot be read")]
    ReadFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A file could not be written.
    #[error("file cannot be written")]
    WriteFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// The path is not inside any granted root.
    #[error("path is outside every granted root")]
    OutsideGrants,

    /// The path carries a `..` or an alternate data stream name.
    #[error("path contains a forbidden component")]
    ForbiddenComponent,

    /// The path does not exist.
    #[error("path does not exist")]
    NotFound,

    /// A revoke named a path that is not a granted root.
    #[error("path is not a granted root")]
    NotGranted,

    /// A tree listing was requested for something that is not a directory.
    #[error("path is not a directory")]
    NotADirectory,

    /// A read or write targeted something that is not a regular file.
    #[error("path is not a file")]
    NotAFile,

    /// The file contains NUL bytes and is not editable text.
    #[error("file is binary, not text")]
    BinaryFile,

    /// The file is not valid UTF-8.
    #[error("file is not utf-8 text")]
    NotUtf8,

    /// The file or body exceeds [`MAX_FILE_BYTES`].
    #[error("file exceeds the {limit}-byte size limit")]
    FileTooLarge {
        /// The size limit that was exceeded.
        limit: u64,
    },

    /// The on-disk conflict token does not match the writer's token.
    #[error("file changed on disk since it was read")]
    ModifiedConflict,
}

/// Whether a tree entry is a directory or a regular file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EntryKind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
}

/// One entry in a directory listing.
#[derive(Debug, Serialize)]
pub(crate) struct TreeEntry {
    /// The entry's file name (lossy for non-Unicode names).
    name: String,
    /// The entry's full path, ready to pass back to the API.
    path: PathBuf,
    /// Directory or file.
    kind: EntryKind,
    /// Byte length (0 for directories).
    size: u64,
    /// Modification time in milliseconds since the Unix epoch.
    modified_ms: u64,
}

/// One level of a workspace directory tree.
#[derive(Debug, Serialize)]
pub(crate) struct TreeListing {
    /// The listed directory; `None` when the listing is the granted roots.
    path: Option<PathBuf>,
    /// Directories before files, each group ordered by name.
    entries: Vec<TreeEntry>,
}

/// A file's text plus the metadata a writer needs to detect conflicts.
#[derive(Debug, Serialize)]
pub(crate) struct FileContents {
    /// The canonical file path.
    path: PathBuf,
    /// Byte length.
    size: u64,
    /// The opaque conflict token a writer must echo back as
    /// `expected_token`; see [`file_token`] for its derivation.
    token: String,
    /// The file's UTF-8 text.
    text: String,
}

/// The in-memory set of granted workspace roots.
///
/// Cloning shares the same grant set, so the router state and every handler
/// see grants registered through `POST /workspace/grant` immediately.
#[derive(Debug, Clone, Default)]
pub(crate) struct Workspace {
    grants: Arc<RwLock<BTreeSet<PathBuf>>>,
}

impl Workspace {
    /// Creates a workspace with no grants.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers `path` as a granted root: a directory grants itself, a
    /// file grants its parent directory.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path carries
    /// a `..` or stream name, [`WorkspaceError::ResolveGrant`] when it
    /// cannot be canonicalized, and [`WorkspaceError::NotFound`] when a
    /// file path has no parent directory.
    pub(crate) fn grant(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        let canonical = canonicalize_simplified(path)
            .map_err(|source| WorkspaceError::ResolveGrant { source })?;
        let root = if canonical.is_dir() {
            canonical
        } else {
            canonical
                .parent()
                .map(Path::to_owned)
                .ok_or(WorkspaceError::NotFound)?
        };
        self.grants
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root.clone());
        Ok(root)
    }

    /// Removes `path` from the granted roots by exact canonical match.
    /// A root deleted from disk stays revocable by the literal stored
    /// key. Nested grants are independent: revoking a parent leaves a
    /// separately granted child intact, and files under the child stay
    /// reachable while everything else under the parent loses access on
    /// its next operation.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path carries
    /// a `..` or stream name, [`WorkspaceError::ResolveGrant`] when
    /// canonicalization fails for a reason other than absence, and
    /// [`WorkspaceError::NotGranted`] when the resolved path is not a
    /// granted root.
    pub(crate) fn revoke(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        // A root deleted from disk no longer canonicalizes, but its grant
        // must stay removable: fall back to the literal path, which matches
        // the stored canonical key the roots listing handed the client.
        let canonical = match canonicalize_simplified(path) {
            Ok(canonical) => canonical,
            Err(source) if source.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(source) => return Err(WorkspaceError::ResolveGrant { source }),
        };
        let removed = self
            .grants
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&canonical);
        if removed {
            Ok(canonical)
        } else {
            Err(WorkspaceError::NotGranted)
        }
    }

    /// The granted roots in stable sorted order.
    pub(crate) fn granted_roots(&self) -> Vec<PathBuf> {
        self.grants
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Lists one level of `path`, or the granted roots when `path` is
    /// `None` or empty. Directories sort before files, each group ordered
    /// by name.
    ///
    /// # Errors
    /// Returns [`WorkspaceError`] when the path is forbidden, outside every
    /// grant, missing, not a directory, or cannot be listed.
    pub(crate) fn tree(&self, path: Option<&Path>) -> Result<TreeListing, WorkspaceError> {
        match path {
            None => Ok(self.grants_listing()),
            Some(path) if path.as_os_str().is_empty() => Ok(self.grants_listing()),
            Some(path) => self.directory_listing(path),
        }
    }

    /// Reads a confined UTF-8 text file with its size and conflict
    /// token. Binary and oversized files are rejected.
    ///
    /// # Errors
    /// Returns [`WorkspaceError`] when the path is forbidden, outside every
    /// grant, missing, not a regular file, binary, not UTF-8, oversized, or
    /// cannot be read.
    pub(crate) fn read_file(&self, path: &Path) -> Result<FileContents, WorkspaceError> {
        let canonical = self.confine_existing(path)?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        if !metadata.is_file() {
            return Err(WorkspaceError::NotAFile);
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(WorkspaceError::FileTooLarge {
                limit: MAX_FILE_BYTES,
            });
        }
        let bytes = fs::read(&canonical).map_err(|source| WorkspaceError::ReadFile { source })?;
        if bytes.contains(&0) {
            return Err(WorkspaceError::BinaryFile);
        }
        let token = file_token(&metadata, &bytes);
        let text = String::from_utf8(bytes).map_err(|_| WorkspaceError::NotUtf8)?;
        Ok(FileContents {
            path: canonical,
            size: metadata.len(),
            token,
            text,
        })
    }

    /// Writes `text` to a confined path, creating the file when it does not
    /// exist. When the file exists, `expected_token` must match its current
    /// conflict token or the write is refused as a conflict.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::FileTooLarge`] when the text exceeds the
    /// size limit, [`WorkspaceError::ModifiedConflict`] when the token is
    /// stale, absent, or underivable for the existing file, and otherwise
    /// [`WorkspaceError`] when the path is forbidden, outside every grant,
    /// not a regular file, or cannot be written.
    pub(crate) fn write_file(
        &self,
        path: &Path,
        text: &str,
        expected_token: Option<&str>,
    ) -> Result<FileContents, WorkspaceError> {
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(WorkspaceError::FileTooLarge {
                limit: MAX_FILE_BYTES,
            });
        }
        let canonical = self.confine_for_write(path)?;
        match fs::metadata(&canonical) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(WorkspaceError::NotAFile);
                }
                // Fail closed: only a derivable on-disk token that equals
                // the writer's token proves the file is unchanged.
                match (current_token(&canonical, &metadata), expected_token) {
                    (Some(current), Some(expected)) if current == expected => {}
                    _ => return Err(WorkspaceError::ModifiedConflict),
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(WorkspaceError::InspectPath { source }),
        }
        crate::atomic::write_atomic(&canonical, text.as_bytes())
            .map_err(|source| WorkspaceError::WriteFile { source })?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        Ok(FileContents {
            path: canonical,
            size: metadata.len(),
            token: file_token(&metadata, text.as_bytes()),
            text: text.to_owned(),
        })
    }

    /// The granted roots rendered as a synthetic directory listing.
    fn grants_listing(&self) -> TreeListing {
        let entries = self
            .granted_roots()
            .into_iter()
            .map(|root| {
                let metadata = fs::metadata(&root).ok();
                // The folder's own name reads better than the full path in
                // the tree; the path stays available as the row tooltip. A
                // drive root (C:\) has no file name and shows the path.
                let name = root.file_name().map_or_else(
                    || root.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                TreeEntry {
                    name,
                    path: root,
                    kind: EntryKind::Directory,
                    size: 0,
                    modified_ms: metadata.as_ref().map_or(0, modified_ms),
                }
            })
            .collect();
        TreeListing {
            path: None,
            entries,
        }
    }

    /// Lists one level of an existing confined directory.
    fn directory_listing(&self, path: &Path) -> Result<TreeListing, WorkspaceError> {
        let canonical = self.confine_existing(path)?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        if !metadata.is_dir() {
            return Err(WorkspaceError::NotADirectory);
        }
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&canonical).map_err(|source| WorkspaceError::ListDirectory { source })?
        {
            let entry = entry.map_err(|source| WorkspaceError::ListDirectory { source })?;
            let metadata = entry
                .metadata()
                .map_err(|source| WorkspaceError::InspectPath { source })?;
            let kind = if metadata.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            entries.push(TreeEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                kind,
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                modified_ms: modified_ms(&metadata),
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Directory)
                .cmp(&(b.kind != EntryKind::Directory))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(TreeListing {
            path: Some(canonical),
            entries,
        })
    }

    /// Canonicalizes an existing path and confines it to the grants.
    fn confine_existing(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        let canonical = canonicalize_simplified(path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                WorkspaceError::NotFound
            } else {
                WorkspaceError::ResolvePath { source }
            }
        })?;
        self.check_confined(canonical)
    }

    /// Confines a write target: an existing path canonicalizes directly; a
    /// new file confines its canonicalized parent and reattaches its name.
    fn confine_for_write(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        match canonicalize_simplified(path) {
            Ok(canonical) => self.check_confined(canonical),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                // A dangling symlink canonicalizes as NotFound, but fs::write
                // would follow it and create the target outside the grant.
                match fs::symlink_metadata(path) {
                    Ok(_) => return Err(WorkspaceError::OutsideGrants),
                    Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => return Err(WorkspaceError::InspectPath { source }),
                }
                let parent = path.parent().ok_or(WorkspaceError::NotFound)?;
                let canonical_parent = canonicalize_simplified(parent).map_err(|source| {
                    if source.kind() == io::ErrorKind::NotFound {
                        WorkspaceError::NotFound
                    } else {
                        WorkspaceError::ResolvePath { source }
                    }
                })?;
                let name = path.file_name().ok_or(WorkspaceError::ForbiddenComponent)?;
                self.check_confined(canonical_parent.join(name))
            }
            Err(source) => Err(WorkspaceError::ResolvePath { source }),
        }
    }

    /// Admits a canonical path that starts with a granted root.
    fn check_confined(&self, canonical: PathBuf) -> Result<PathBuf, WorkspaceError> {
        let grants = self.grants.read().unwrap_or_else(PoisonError::into_inner);
        if grants.iter().any(|root| canonical.starts_with(root)) {
            Ok(canonical)
        } else {
            Err(WorkspaceError::OutsideGrants)
        }
    }
}

/// Canonicalizes and strips Windows' `\\?\` verbatim prefix (a no-op on
/// other platforms). Every path the workspace stores, compares, or returns
/// goes through here, so grants and confinement checks stay in one form
/// and the UI never sees the prefix.
fn canonicalize_simplified(path: &Path) -> io::Result<PathBuf> {
    Ok(dunce::simplified(&path.canonicalize()?).to_path_buf())
}

/// Rejects the lexical tricks canonicalization would otherwise hide: `..`
/// traversal everywhere, and `:` alternate data stream names on Windows,
/// where a colon in a name addresses an NTFS stream. Elsewhere a colon is
/// an ordinary filename character and passes.
fn reject_forbidden(path: &Path) -> Result<(), WorkspaceError> {
    for component in path.components() {
        match component {
            Component::ParentDir => return Err(WorkspaceError::ForbiddenComponent),
            #[cfg(windows)]
            Component::Normal(name) if name.to_string_lossy().contains(':') => {
                return Err(WorkspaceError::ForbiddenComponent);
            }
            _ => {}
        }
    }
    Ok(())
}

/// A file's modification time as milliseconds since the Unix epoch.
fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The mtime half of the conflict token: full-precision modified time in
/// nanoseconds since the Unix epoch plus the byte length. `None` when the
/// filesystem reports no usable modified time, which callers cover with
/// [`hash_token`] - collapsing the error to a constant would make every
/// token on such a filesystem equal and no write would ever conflict.
fn mtime_token(metadata: &fs::Metadata) -> Option<String> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!("{}-{}", duration.as_nanos(), metadata.len()))
}

/// The content-hash fallback token for filesystems without modified times.
/// `DefaultHasher` is stable within one process run, which is all a token
/// needs: a restart invalidates outstanding tokens toward conflict, never
/// toward a silent overwrite.
fn hash_token(contents: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    format!("h-{:016x}", hasher.finish())
}

/// A file's opaque conflict token from its metadata and already-read
/// contents: the mtime form when available, otherwise the hash form.
fn file_token(metadata: &fs::Metadata, contents: &[u8]) -> String {
    mtime_token(metadata).unwrap_or_else(|| hash_token(contents))
}

/// The current on-disk token of an existing write target, reading the file
/// only when the hash fallback demands it. `None` means no token could be
/// derived - an unreadable or oversized file - and the caller must refuse
/// the write rather than overwrite unverified contents.
fn current_token(path: &Path, metadata: &fs::Metadata) -> Option<String> {
    if let Some(token) = mtime_token(metadata) {
        return Some(token);
    }
    if metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read(path).ok().map(|bytes| hash_token(&bytes))
}

/// The query string of `GET /workspace/tree`.
#[derive(Debug, Deserialize)]
pub(crate) struct TreeQuery {
    /// The directory to list; absent or empty lists the granted roots.
    path: Option<String>,
}

/// The query string of `GET /workspace/file`.
#[derive(Debug, Deserialize)]
pub(crate) struct FileQuery {
    /// The file to read.
    path: String,
}

/// The JSON body of `PUT /workspace/file`.
#[derive(Debug, Deserialize)]
pub(crate) struct WriteRequest {
    /// The file to write.
    path: String,
    /// The new UTF-8 contents.
    text: String,
    /// The conflict token the writer last read; required to match when
    /// the file already exists.
    expected_token: Option<String>,
}

/// The JSON body of `POST /workspace/grant`.
#[derive(Debug, Deserialize)]
pub(crate) struct GrantRequest {
    /// The dropped path: a folder grants itself, a file grants its parent.
    path: String,
}

/// The JSON body of a successful grant.
#[derive(Debug, Serialize)]
pub(crate) struct GrantResponse {
    /// The root that was registered.
    granted: PathBuf,
}

/// The JSON body of `POST /workspace/revoke`.
#[derive(Debug, Deserialize)]
pub(crate) struct RevokeRequest {
    /// The granted root to remove, as listed by the roots tree.
    path: String,
}

/// The JSON body of a successful revoke.
#[derive(Debug, Serialize)]
pub(crate) struct RevokeResponse {
    /// The root that was removed.
    revoked: PathBuf,
}

/// Percent-decodes a workspace path parameter before validation. The query
/// layer already decoded once, so any surviving `%XX` sequence is a second
/// encoding layer - decoding it here means an encoded traversal (`%2e%2e`)
/// reaches the lexical `..` check as a literal `..` however the client
/// encoded it. Invalid sequences pass through unchanged.
fn decode_path_param(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned()
}

/// Lists one level of a workspace directory, or the granted roots when the
/// query carries no path.
pub(crate) async fn tree(
    State(workspace): State<Workspace>,
    Query(query): Query<TreeQuery>,
) -> Response {
    let path = query.path.as_deref().map(decode_path_param);
    respond(workspace.tree(path.as_deref().map(Path::new)))
}

/// Reads a confined UTF-8 text file with its metadata.
pub(crate) async fn read_file(
    State(workspace): State<Workspace>,
    Query(query): Query<FileQuery>,
) -> Response {
    let path = decode_path_param(&query.path);
    respond(workspace.read_file(Path::new(&path)))
}

/// Writes a confined file after path, size, and conflict-token validation.
pub(crate) async fn write_file(
    State(workspace): State<Workspace>,
    Json(body): Json<WriteRequest>,
) -> Response {
    respond(workspace.write_file(
        Path::new(&body.path),
        &body.text,
        body.expected_token.as_deref(),
    ))
}

/// Registers a dropped path as a granted root for this process.
pub(crate) async fn grant(
    State(workspace): State<Workspace>,
    Json(body): Json<GrantRequest>,
) -> Response {
    respond(
        workspace
            .grant(Path::new(&body.path))
            .map(|granted| GrantResponse { granted }),
    )
}

/// Removes a granted root; paths under it fail their next operation.
pub(crate) async fn revoke(
    State(workspace): State<Workspace>,
    Json(body): Json<RevokeRequest>,
) -> Response {
    respond(
        workspace
            .revoke(Path::new(&body.path))
            .map(|revoked| RevokeResponse { revoked }),
    )
}

/// Renders a workspace result as JSON, routing failures through the
/// [`AppError`] wire envelope.
fn respond<T: Serialize>(result: Result<T, WorkspaceError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => AppError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace with one granted tempdir, returned alongside so the
    /// directory outlives the test.
    fn granted_dir() -> (Workspace, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let workspace = Workspace::new();
        workspace.grant(dir.path()).expect("grant the tempdir");
        (workspace, dir)
    }

    /// The canonical, verbatim-prefix-free form grants are stored in.
    fn simplified(path: &Path) -> PathBuf {
        canonicalize_simplified(path).expect("canonical")
    }

    #[test]
    fn a_folder_grant_grants_the_folder_itself() {
        let workspace = Workspace::new();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let granted = workspace.grant(dir.path()).expect("grant succeeds");
        assert_eq!(granted, simplified(dir.path()));
        assert_eq!(workspace.granted_roots(), vec![granted]);
    }

    #[test]
    fn a_file_grant_grants_the_parent_directory() {
        let workspace = Workspace::new();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = dir.path().join("dropped.txt");
        fs::write(&file, "x").expect("seed the dropped file");
        let granted = workspace.grant(&file).expect("grant succeeds");
        assert_eq!(granted, simplified(dir.path()));
        assert_eq!(workspace.granted_roots(), vec![granted]);
    }

    #[test]
    fn files_read_and_write_inside_a_grant() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("notes.txt");
        let written = workspace
            .write_file(&file, "hello", None)
            .expect("write inside the grant");
        assert_eq!(written.text, "hello");
        assert_eq!(written.size, 5);
        let read = workspace.read_file(&file).expect("read inside the grant");
        assert_eq!(read.text, "hello");
        assert_eq!(read.size, 5);
        assert_eq!(read.token, written.token);
    }

    #[test]
    fn writes_leave_no_temp_file_behind() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("notes.txt");
        let written = workspace
            .write_file(&file, "one", None)
            .expect("the create write succeeds");
        workspace
            .write_file(&file, "two", Some(&written.token))
            .expect("the overwrite succeeds");
        let names: Vec<String> = fs::read_dir(dir.path())
            .expect("the granted directory is listable")
            .map(|entry| {
                entry
                    .expect("the entry is readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            ["notes.txt"],
            "the atomic write's temp file must not survive the write"
        );
    }

    #[test]
    fn paths_outside_every_grant_are_rejected() {
        let workspace = Workspace::new();
        let dir = tempfile::TempDir::new().expect("tempdir");
        fs::write(dir.path().join("a.txt"), "a").expect("seed outside the grants");
        let error = workspace
            .read_file(&dir.path().join("a.txt"))
            .expect_err("an ungranted path must be rejected");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
    }

    #[test]
    fn parent_components_are_rejected() {
        let (workspace, dir) = granted_dir();
        let escape = dir.path().join("..").join("anything.txt");
        let error = workspace
            .read_file(&escape)
            .expect_err("a .. component must be rejected");
        assert!(
            matches!(error, WorkspaceError::ForbiddenComponent),
            "expected ForbiddenComponent, got {error:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn alternate_data_stream_names_are_rejected() {
        let (workspace, dir) = granted_dir();
        let stream = dir.path().join("notes.txt:secret");
        let error = workspace
            .write_file(&stream, "hidden", None)
            .expect_err("an alternate data stream name must be rejected");
        assert!(
            matches!(error, WorkspaceError::ForbiddenComponent),
            "expected ForbiddenComponent, got {error:?}"
        );
    }

    #[test]
    fn a_symlink_escape_is_rejected() {
        let (workspace, dir) = granted_dir();
        let outside = tempfile::TempDir::new().expect("outside tempdir");
        fs::write(outside.path().join("secret.txt"), "secret").expect("seed the secret");
        let link = dir.path().join("link");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), &link);
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
        let Ok(()) = linked else {
            // Symlink creation needs a privilege some Windows hosts lack.
            eprintln!("skipping: symlink creation failed");
            return;
        };
        let error = workspace
            .read_file(&link.join("secret.txt"))
            .expect_err("a symlink escape must be rejected");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
    }

    #[test]
    fn a_dangling_symlink_write_is_rejected() {
        let (workspace, dir) = granted_dir();
        let outside = tempfile::TempDir::new().expect("outside tempdir");
        let target = outside.path().join("new.txt");
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&target, &link);
        let Ok(()) = linked else {
            // Symlink creation needs a privilege some Windows hosts lack.
            eprintln!("skipping: symlink creation failed");
            return;
        };
        let error = workspace
            .write_file(&link, "payload", None)
            .expect_err("a write through a dangling symlink must be rejected");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
        assert!(!target.exists(), "nothing may be written outside the grant");
    }

    #[test]
    fn binary_files_are_rejected() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("bin.dat");
        fs::write(&file, [0x66, 0x00, 0x66]).expect("seed a binary file");
        let error = workspace
            .read_file(&file)
            .expect_err("a binary file must be rejected");
        assert!(
            matches!(error, WorkspaceError::BinaryFile),
            "expected BinaryFile, got {error:?}"
        );
    }

    #[test]
    fn oversized_files_are_rejected() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("big.txt");
        let big = vec![b'x'; usize::try_from(MAX_FILE_BYTES).expect("the limit fits") + 1];
        fs::write(&file, big).expect("seed an oversized file");
        let error = workspace
            .read_file(&file)
            .expect_err("an oversized file must be rejected");
        assert!(
            matches!(error, WorkspaceError::FileTooLarge { .. }),
            "expected FileTooLarge, got {error:?}"
        );
    }

    #[test]
    fn oversized_writes_are_rejected() {
        let (workspace, dir) = granted_dir();
        let text = "x".repeat(usize::try_from(MAX_FILE_BYTES).expect("the limit fits") + 1);
        let error = workspace
            .write_file(&dir.path().join("big.txt"), &text, None)
            .expect_err("an oversized write must be rejected");
        assert!(
            matches!(error, WorkspaceError::FileTooLarge { .. }),
            "expected FileTooLarge, got {error:?}"
        );
    }

    #[test]
    fn a_stale_modified_token_conflicts() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("a.txt");
        let written = workspace
            .write_file(&file, "one", None)
            .expect("initial write");
        let stale = format!("{}-stale", written.token);
        let error = workspace
            .write_file(&file, "two", Some(&stale))
            .expect_err("a stale token must conflict");
        assert!(
            matches!(error, WorkspaceError::ModifiedConflict),
            "expected ModifiedConflict, got {error:?}"
        );
        let rewritten = workspace
            .write_file(&file, "two", Some(&written.token))
            .expect("the fresh token writes");
        assert_eq!(rewritten.text, "two");
    }

    #[test]
    fn a_tokenless_write_to_an_existing_file_conflicts() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("a.txt");
        workspace
            .write_file(&file, "one", None)
            .expect("initial write");
        let error = workspace
            .write_file(&file, "two", None)
            .expect_err("a write with no token over an existing file must conflict");
        assert!(
            matches!(error, WorkspaceError::ModifiedConflict),
            "expected ModifiedConflict, got {error:?}"
        );
    }

    #[test]
    fn the_token_tracks_mtime_and_length_not_write_count() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("t.txt");
        let written = workspace.write_file(&file, "one", None).expect("write");
        let metadata = fs::metadata(&file).expect("metadata");
        // The token is a pure function of full-precision mtime plus length:
        // a same-content rewrite changes it exactly when the filesystem
        // reports a new mtime or length, and never otherwise.
        assert_eq!(Some(written.token.clone()), mtime_token(&metadata));
        let rewritten = workspace
            .write_file(&file, "one", Some(&written.token))
            .expect("same-content rewrite");
        let metadata = fs::metadata(&file).expect("metadata after rewrite");
        assert_eq!(Some(rewritten.token), mtime_token(&metadata));
        // Reading without a write in between re-derives the same token.
        let reread = workspace.read_file(&file).expect("read");
        let again = workspace.read_file(&file).expect("second read");
        assert_eq!(reread.token, again.token);
    }

    #[test]
    fn the_hash_fallback_token_round_trips() {
        let token = hash_token(b"same contents");
        assert_eq!(
            token,
            hash_token(b"same contents"),
            "the fallback token must be stable for identical contents"
        );
        assert!(token.starts_with("h-"), "got {token}");
        assert_ne!(token, hash_token(b"different contents"));
    }

    #[cfg(unix)]
    #[test]
    fn colon_named_files_read_and_write_on_unix() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("backup-12:30.log");
        let written = workspace
            .write_file(&file, "ok", None)
            .expect("a colon-named file writes on unix");
        let read = workspace
            .read_file(&file)
            .expect("a colon-named file reads on unix");
        assert_eq!(read.text, "ok");
        assert_eq!(read.token, written.token);
    }

    #[test]
    fn tree_lists_directories_before_files_with_stable_ordering() {
        let (workspace, dir) = granted_dir();
        fs::create_dir(dir.path().join("zeta")).expect("dir");
        fs::create_dir(dir.path().join("alpha")).expect("dir");
        fs::write(dir.path().join("b.txt"), "b").expect("file");
        fs::write(dir.path().join("a.txt"), "a").expect("file");
        let listing = workspace.tree(Some(dir.path())).expect("tree");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "zeta", "a.txt", "b.txt"]);
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);
        assert_eq!(listing.entries[3].kind, EntryKind::File);
    }

    #[test]
    fn a_tree_without_a_path_lists_the_granted_roots() {
        let (workspace, dir) = granted_dir();
        let listing = workspace.tree(None).expect("roots listing");
        assert_eq!(listing.path, None);
        assert_eq!(listing.entries.len(), 1);
        let root = simplified(dir.path());
        assert_eq!(listing.entries[0].path, root);
        // A root row shows the folder's own name, not the whole path.
        assert_eq!(
            listing.entries[0].name,
            root.file_name().expect("leaf").to_string_lossy()
        );
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);
    }

    #[test]
    fn percent_sequences_decode_before_validation() {
        assert_eq!(decode_path_param("%2e%2e/x"), "../x");
        assert_eq!(decode_path_param("plain.txt"), "plain.txt");
        // An invalid sequence is not an encoding; the literal survives.
        assert_eq!(decode_path_param("100%.txt"), "100%.txt");
    }

    /// A double-encoded traversal (`%252e%252e` in the raw query) survives
    /// the query layer's single decode as `%2e%2e`; the handler's explicit
    /// decode must still reveal it to the lexical `..` check.
    #[tokio::test]
    async fn an_encoded_traversal_in_the_query_is_rejected() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt as _;

        let (workspace, _dir) = granted_dir();
        let router = crate::routes::workspace::routes(workspace);
        for uri in [
            "/workspace/file?path=%252e%252e%2Fsecret.txt",
            "/workspace/tree?path=%252e%252e",
        ] {
            let request = Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("static request parts are valid");
            let response = router
                .clone()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "for {uri}");
            let body = crate::app::fixtures::body_bytes(response).await;
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("the envelope is JSON");
            assert_eq!(
                json["error"]["code"], "forbidden_component",
                "the traversal must be caught lexically, not by a lookup miss: {uri}"
            );
        }
    }

    #[test]
    fn a_revoke_removes_the_granted_root() {
        let (workspace, dir) = granted_dir();
        let revoked = workspace.revoke(dir.path()).expect("revoke the grant");
        assert_eq!(revoked, simplified(dir.path()));
        assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
    }

    #[test]
    fn revoking_an_unknown_root_errors() {
        let workspace = Workspace::new();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let error = workspace
            .revoke(dir.path())
            .expect_err("an ungranted root must not revoke");
        assert!(
            matches!(error, WorkspaceError::NotGranted),
            "expected NotGranted, got {error:?}"
        );
    }

    #[test]
    fn a_deleted_root_can_still_be_revoked() {
        let (workspace, dir) = granted_dir();
        let root = simplified(dir.path());
        dir.close().expect("delete the granted directory");
        let revoked = workspace.revoke(&root).expect("revoke the deleted root");
        assert_eq!(revoked, root);
        assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_spelling_variant_revokes_the_same_root() {
        let (workspace, dir) = granted_dir();
        let variant = PathBuf::from(format!(
            "{}{}",
            dir.path().display(),
            std::path::MAIN_SEPARATOR
        ));
        let revoked = workspace
            .revoke(&variant)
            .expect("a trailing separator names the same root");
        assert_eq!(revoked, simplified(dir.path()));
        assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
    }

    #[test]
    fn reads_and_writes_under_a_revoked_root_are_rejected() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("notes.txt");
        let written = workspace
            .write_file(&file, "hello", None)
            .expect("write before the revoke");
        workspace.revoke(dir.path()).expect("revoke the grant");
        let error = workspace
            .read_file(&file)
            .expect_err("a read under a revoked root must be rejected");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
        let error = workspace
            .write_file(&file, "later", Some(&written.token))
            .expect_err("a write under a revoked root must be rejected");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
    }

    #[test]
    fn a_nested_grant_survives_its_parents_revoke() {
        let workspace = Workspace::new();
        let parent = tempfile::TempDir::new().expect("tempdir");
        let child = parent.path().join("child");
        fs::create_dir(&child).expect("create the nested directory");
        fs::write(parent.path().join("outer.txt"), "outer").expect("seed the parent");
        fs::write(child.join("inner.txt"), "inner").expect("seed the child");
        workspace.grant(parent.path()).expect("grant the parent");
        workspace.grant(&child).expect("grant the child");
        workspace.revoke(parent.path()).expect("revoke the parent");
        assert_eq!(workspace.granted_roots(), vec![simplified(&child)]);
        let read = workspace
            .read_file(&child.join("inner.txt"))
            .expect("the nested grant stays usable");
        assert_eq!(read.text, "inner");
        let error = workspace
            .read_file(&parent.path().join("outer.txt"))
            .expect_err("the parent's own files lose access");
        assert!(
            matches!(error, WorkspaceError::OutsideGrants),
            "expected OutsideGrants, got {error:?}"
        );
    }

    /// Builds a `POST /workspace/revoke` request with a raw JSON body.
    fn revoke_request(body: String) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/workspace/revoke")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .expect("static request parts are valid")
    }

    #[tokio::test]
    async fn a_revoke_over_http_removes_the_root() {
        use tower::ServiceExt as _;

        let (workspace, dir) = granted_dir();
        let router = crate::routes::workspace::routes(workspace.clone());
        let root = simplified(dir.path());
        let body = serde_json::json!({ "path": root }).to_string();
        let response = router
            .oneshot(revoke_request(body))
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = crate::app::fixtures::body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("the body is JSON");
        assert_eq!(json["revoked"], serde_json::json!(root));
        assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
    }

    #[tokio::test]
    async fn an_unknown_root_revoke_answers_not_found() {
        use tower::ServiceExt as _;

        let (workspace, _dir) = granted_dir();
        let outside = tempfile::TempDir::new().expect("outside tempdir");
        let router = crate::routes::workspace::routes(workspace);
        let body = serde_json::json!({ "path": outside.path() }).to_string();
        let response = router
            .oneshot(revoke_request(body))
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = crate::app::fixtures::body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("the body is JSON");
        assert_eq!(json["error"]["code"], "not_granted");
    }

    #[tokio::test]
    async fn a_malformed_revoke_body_answers_bad_request() {
        use tower::ServiceExt as _;

        let (workspace, _dir) = granted_dir();
        let router = crate::routes::workspace::routes(workspace);
        let response = router
            .oneshot(revoke_request("{".to_owned()))
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_tree_of_a_file_is_rejected() {
        let (workspace, dir) = granted_dir();
        let file = dir.path().join("a.txt");
        fs::write(&file, "a").expect("seed");
        let error = workspace
            .tree(Some(&file))
            .expect_err("a file cannot be listed");
        assert!(
            matches!(error, WorkspaceError::NotADirectory),
            "expected NotADirectory, got {error:?}"
        );
    }
}
