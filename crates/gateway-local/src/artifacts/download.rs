//! HTTP blob download with connect timeout, size cap, scoped HF auth, and
//! resume of interrupted transfers.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use sha2::{Digest, Sha256};
use shared_progress::ProgressHandle;
use tokio_util::sync::CancellationToken;

use super::Result;
use super::confine::source_marker_path;
use super::digest::hex_digest;
use super::progress::{DownloadProgress, NoopProgress, TreeProgress};
use crate::error::LocalError;

/// Hard ceiling on a single artifact, guarding the cache volume against a
/// malicious or mistaken endpoint. Generous enough for large GGUF weights.
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024 * 1024;

/// Idle bound on a single body read: a peer that stops sending mid-body
/// surfaces as a timed-out read within this window, so Cancel and Quit are
/// honored at the next chunk boundary instead of waiting out the client's
/// whole-request ceiling. A healthy slow transfer never trips it: any byte
/// inside the window resets it.
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether `url` is an HTTPS Hugging Face host eligible for the hub bearer token.
///
/// The token is attached only to these hosts so an operator's `HF_TOKEN` is
/// never disclosed to an arbitrary (or plaintext-HTTP) endpoint named in a
/// `[[local_model]].source`.
pub(super) fn is_huggingface_https(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => parsed.scheme() == "https" && parsed.host_str().is_some_and(is_hf_host),
        Err(_) => false,
    }
}

fn is_hf_host(host: &str) -> bool {
    host == "huggingface.co" || host.ends_with(".huggingface.co")
}

/// Reads a process environment variable as UTF-8 text.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Reads the HF bearer token from the process environment.
pub(crate) fn hub_bearer_token_from_env() -> Option<String> {
    hub_bearer_token(env_var)
}

/// Hugging Face hub bearer token for gated downloads.
///
/// Prefers `HF_TOKEN`, then `HUGGING_FACE_HUB_TOKEN`. Empty or whitespace-only
/// values are ignored. The token is never logged.
pub(super) fn hub_bearer_token(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in ["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"] {
        if let Some(value) = lookup(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Downloads `url` to `destination`, reporting byte counts into a leaf of
/// the progress tree when `tree` is given, and returns the SHA-256 hex
/// digest of the streamed bytes.
///
/// # Errors
/// Returns [`LocalError`] on transport, size-cap, or filesystem failure.
pub(super) fn download(
    client: &Client,
    url: &str,
    destination: &Path,
    tree: Option<&ProgressHandle>,
    token: Option<&CancellationToken>,
) -> Result<String> {
    match tree {
        Some(handle) => {
            let leaf = TreeProgress::new(handle.clone());
            run_download(client, url, destination, &leaf, token)
        }
        None => run_download(client, url, destination, &NoopProgress, token),
    }
}

/// Runs the download, reporting the terminal outcome to `progress`.
fn run_download(
    client: &Client,
    url: &str,
    destination: &Path,
    progress: &dyn DownloadProgress,
    token: Option<&CancellationToken>,
) -> Result<String> {
    match download_with_progress(client, url, destination, progress, token) {
        Ok(digest) => {
            progress.finish();
            Ok(digest)
        }
        Err(error) => {
            progress.abandon();
            Err(error)
        }
    }
}

/// Records the partial's source URL for a later resume. Best-effort: a
/// marker that cannot be written costs resume on the next attempt, never
/// the download itself.
fn write_source_marker(part: &Path, url: &str) {
    if let Err(error) = fs::write(source_marker_path(part), url) {
        tracing::warn!(
            path = %part.display(),
            error = %error,
            "could not write the download provenance marker; a retry restarts from zero"
        );
    }
}

/// Removes the provenance marker on a completed transfer.
fn remove_source_marker(part: &Path) {
    let _ignored = fs::remove_file(source_marker_path(part));
}

/// The length of a resumable partial at `destination`: its byte length when
/// the provenance marker names this same `url`, or zero when there is no
/// partial or the provenance is unknown or foreign.
fn resumable_len(destination: &Path, url: &str) -> Result<u64> {
    let Ok(recorded) = fs::read_to_string(source_marker_path(destination)) else {
        return Ok(0);
    };
    if recorded != url {
        return Ok(0);
    }
    match fs::metadata(destination) {
        Ok(metadata) => Ok(metadata.len()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(source) => Err(LocalError::Io {
            operation: "stat partial download",
            path: destination.to_owned(),
            source,
        }),
    }
}

/// Issues the GET: the HF bearer token when eligible, and a `Range` header
/// when resuming past `resume_from`.
fn send(client: &Client, url: &str, resume_from: u64) -> Result<Response> {
    let mut request = client.get(url);
    if is_huggingface_https(url)
        && let Some(token) = hub_bearer_token(env_var)
    {
        request = request.bearer_auth(token);
    }
    if resume_from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
    }
    request.send().map_err(|source| LocalError::Download {
        url: url.to_owned(),
        source,
    })
}

/// The range start and declared total from a 206 answer's `Content-Range`
/// header (`bytes <start>-<end>/<total>`), `None` when the header is absent
/// or malformed.
fn content_range(response: &Response) -> Option<(u64, u64)> {
    let value = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let range = value.strip_prefix("bytes ")?;
    let (span, total) = range.split_once('/')?;
    let (start, _end) = span.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let total = total.trim().parse().ok()?;
    Some((start, total))
}

/// Issues the GET and settles the resume offset. A 206 must continue
/// exactly at the partial's end within the declared total; a 200 means the
/// server ignored the Range; a 416 means the partial meets or exceeds the
/// blob. Any doubt restarts the transfer from zero with a fresh request.
fn negotiate_resume(client: &Client, url: &str, resume_from: u64) -> Result<(Response, u64)> {
    let response = send(client, url, resume_from)?;
    if resume_from == 0 {
        return Ok((response, 0));
    }
    let restart = if response.status() == StatusCode::PARTIAL_CONTENT {
        match content_range(&response) {
            Some((start, total)) => start != resume_from || resume_from > total,
            None => true,
        }
    } else {
        true
    };
    if restart {
        drop(response);
        return Ok((send(client, url, 0)?, 0));
    }
    Ok((response, resume_from))
}

/// Opens `destination` for the transfer. Resuming opens the existing
/// partial in append mode after passing its bytes through the hasher, so
/// the digest covers the whole blob; a fresh transfer truncates and writes
/// the provenance marker.
fn open_transfer(
    destination: &Path,
    url: &str,
    resume_from: u64,
    hasher: &mut Sha256,
    buffer: &mut [u8],
    progress: &dyn DownloadProgress,
) -> Result<File> {
    if resume_from == 0 {
        write_source_marker(destination, url);
        return File::create(destination).map_err(|source| LocalError::Io {
            operation: "create partial download",
            path: destination.to_owned(),
            source,
        });
    }
    let mut existing = File::open(destination).map_err(|source| LocalError::Io {
        operation: "open partial download for resume",
        path: destination.to_owned(),
        source,
    })?;
    loop {
        let count = existing.read(buffer).map_err(|source| LocalError::Io {
            operation: "hash partial download",
            path: destination.to_owned(),
            source,
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    progress.inc(resume_from);
    OpenOptions::new()
        .append(true)
        .open(destination)
        .map_err(|source| LocalError::Io {
            operation: "append to partial download",
            path: destination.to_owned(),
            source,
        })
}

/// Streams a blocking response body through a channel so the download loop
/// receives each chunk under an idle deadline. The pinned blocking reqwest
/// client (0.12) exposes no per-read timeout, so the reader thread owns the
/// response and forwards every read; a peer that goes silent past the
/// deadline surfaces as [`std::io::ErrorKind::TimedOut`] at the chunk
/// boundary where the cancellation token is already checked, and the staged
/// partial stays resumable. A read parked past the deadline stays parked
/// until the client's whole-request ceiling drops the body, which ends the
/// thread - the wait is bounded and the thread always reaps.
struct IdleReader {
    chunks: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    idle: Duration,
}

impl IdleReader {
    /// Spawns the reader thread draining `response`.
    ///
    /// # Errors
    /// Returns the thread-spawn failure.
    fn new(mut response: Response, idle: Duration) -> std::io::Result<IdleReader> {
        let (sender, chunks) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("artifact-download-reader".to_owned())
            .spawn(move || {
                let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
                loop {
                    let chunk = response
                        .read(&mut buffer)
                        .map(|count| buffer[..count].to_vec());
                    // An empty chunk is the end of the stream; after an error
                    // or EOF there is nothing more to send.
                    let terminal =
                        chunk.is_err() || matches!(&chunk, Ok(bytes) if bytes.is_empty());
                    if sender.send(chunk).is_err() || terminal {
                        break;
                    }
                }
            })?;
        Ok(IdleReader { chunks, idle })
    }

    /// The next body chunk; an empty chunk is the end of the stream.
    ///
    /// # Errors
    /// Returns the read error the thread forwarded, or
    /// [`std::io::ErrorKind::TimedOut`] when no chunk arrived inside the
    /// idle window.
    fn read_chunk(&self) -> std::io::Result<Vec<u8>> {
        match self.chunks.recv_timeout(self.idle) {
            Ok(chunk) => chunk,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "the peer sent nothing for {:?}; the transfer stalled",
                    self.idle
                ),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the download reader thread stopped without reporting",
            )),
        }
    }
}

/// Streams `url` to `destination`, reporting to `progress`, enforcing the
/// [`MAX_ARTIFACT_BYTES`] ceiling on both the declared and streamed length.
///
/// An interrupted attempt resumes: when `destination` exists with a
/// provenance marker naming this same `url`, the transfer continues at the
/// partial's length through a `Range` request. The marker is written when a
/// fresh transfer starts and removed when one completes, so a partial whose
/// transfer finished (and then failed the caller's digest gate) is never
/// resumed. A transfer that ends short of the declared length keeps both,
/// so the next attempt resumes.
///
/// # Errors
/// Returns [`LocalError`] on transport, size-cap, or filesystem failure, and
/// [`LocalError::Cancelled`] when `token` fires; the check runs at entry and
/// between chunks, so a cancelled transfer makes no request at all or stops
/// at the next chunk boundary, and the staged partial stays in place for a
/// later resume. A body read is bounded by [`DOWNLOAD_IDLE_TIMEOUT`]: a
/// stalled peer fails the transfer instead of parking the token check.
pub(crate) fn download_with_progress(
    client: &Client,
    url: &str,
    destination: &Path,
    progress: &dyn DownloadProgress,
    token: Option<&CancellationToken>,
) -> Result<String> {
    download_with_idle(
        client,
        url,
        destination,
        progress,
        token,
        DOWNLOAD_IDLE_TIMEOUT,
    )
}

/// [`download_with_progress`] with the idle read bound explicit, so a test
/// can shrink it.
pub(super) fn download_with_idle(
    client: &Client,
    url: &str,
    destination: &Path,
    progress: &dyn DownloadProgress,
    token: Option<&CancellationToken>,
    idle: Duration,
) -> Result<String> {
    // A token already fired makes no request and stages no file.
    if token.is_some_and(CancellationToken::is_cancelled) {
        return Err(LocalError::Cancelled);
    }
    let (response, resume_from) = negotiate_resume(client, url, resumable_len(destination, url)?)?;
    let response = response
        .error_for_status()
        .map_err(|source| LocalError::Download {
            url: url.to_owned(),
            source,
        })?;
    let total = response
        .content_length()
        .map(|remaining| remaining + resume_from);
    if let Some(total) = total
        && total > MAX_ARTIFACT_BYTES
    {
        return Err(LocalError::ArtifactTooLarge {
            url: url.to_owned(),
            limit: MAX_ARTIFACT_BYTES,
        });
    }
    progress.set_len(total);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut downloaded: u64 = resume_from;
    let file = open_transfer(
        destination,
        url,
        resume_from,
        &mut hasher,
        &mut buffer,
        progress,
    )?;
    let mut writer = BufWriter::new(file);
    let reader = IdleReader::new(response, idle).map_err(|source| LocalError::DownloadRead {
        url: url.to_owned(),
        source,
    })?;
    loop {
        // The cancellation check sits between chunks: a fired token stops the
        // transfer at the next boundary, keeping the staged partial and its
        // provenance marker so a later attempt resumes where this one stopped.
        if token.is_some_and(CancellationToken::is_cancelled) {
            return Err(LocalError::Cancelled);
        }
        let chunk = reader
            .read_chunk()
            .map_err(|source| LocalError::DownloadRead {
                url: url.to_owned(),
                source,
            })?;
        if chunk.is_empty() {
            break;
        }
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > MAX_ARTIFACT_BYTES {
            return Err(LocalError::ArtifactTooLarge {
                url: url.to_owned(),
                limit: MAX_ARTIFACT_BYTES,
            });
        }
        writer.write_all(&chunk).map_err(|source| LocalError::Io {
            operation: "write partial download",
            path: destination.to_owned(),
            source,
        })?;
        hasher.update(&chunk);
        progress.inc(chunk.len() as u64);
    }
    writer.flush().map_err(|source| LocalError::Io {
        operation: "flush partial download",
        path: destination.to_owned(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| LocalError::Io {
            operation: "sync partial download",
            path: destination.to_owned(),
            source,
        })?;
    // A body short of the declared length is a failed transfer, not a
    // complete one: keep the partial and its marker so the next attempt
    // resumes from the offset.
    if let Some(total) = total
        && downloaded != total
    {
        return Err(LocalError::DownloadRead {
            url: url.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("transfer ended at {downloaded} of {total} bytes"),
            ),
        });
    }
    remove_source_marker(destination);
    Ok(hex_digest(hasher))
}
