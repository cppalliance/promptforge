//! HTTP blob download with connect timeout, size cap, and scoped HF auth.

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Read, Write};
use std::path::Path;

use promptforge_progress::ProgressHandle;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use super::Result;
use super::digest::hex_digest;
use super::progress::{
    DownloadProgress, FanoutProgress, TreeProgress, download_label, progress_for_download,
};
use crate::error::LocalError;

/// Hard ceiling on a single artifact, guarding the cache volume against a
/// malicious or mistaken endpoint. Generous enough for large GGUF weights.
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024 * 1024;

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

/// Downloads `url` to `destination`, driving TTY/log progress - plus the
/// progress-tree leaf when `tree` is given - and returns the SHA-256 hex
/// digest of the streamed bytes.
///
/// # Errors
/// Returns [`LocalError`] on transport, size-cap, or filesystem failure.
pub(super) fn download(
    client: &Client,
    url: &str,
    destination: &Path,
    tree: Option<&ProgressHandle>,
) -> Result<String> {
    let label = download_label(url);
    let presentation = progress_for_download(&label, io::stderr().is_terminal());
    match tree {
        Some(handle) => {
            let leaf = TreeProgress::new(handle.clone());
            let progress = FanoutProgress::new(&leaf, presentation.as_ref());
            run_download(client, url, destination, &progress)
        }
        None => run_download(client, url, destination, presentation.as_ref()),
    }
}

/// Runs the download, reporting the terminal outcome to `progress`.
fn run_download(
    client: &Client,
    url: &str,
    destination: &Path,
    progress: &dyn DownloadProgress,
) -> Result<String> {
    match download_with_progress(client, url, destination, progress) {
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

/// Streams `url` to `destination`, reporting to `progress`, enforcing the
/// [`MAX_ARTIFACT_BYTES`] ceiling on both the declared and streamed length.
///
/// # Errors
/// Returns [`LocalError`] on transport, size-cap, or filesystem failure.
pub(crate) fn download_with_progress(
    client: &Client,
    url: &str,
    destination: &Path,
    progress: &dyn DownloadProgress,
) -> Result<String> {
    let mut request = client.get(url);
    if is_huggingface_https(url)
        && let Some(token) = hub_bearer_token(env_var)
    {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|source| LocalError::Download {
            url: url.to_owned(),
            source,
        })?;
    let total = response.content_length();
    if let Some(total) = total
        && total > MAX_ARTIFACT_BYTES
    {
        return Err(LocalError::ArtifactTooLarge {
            url: url.to_owned(),
            limit: MAX_ARTIFACT_BYTES,
        });
    }
    progress.set_len(total);
    let file = File::create(destination).map_err(|source| LocalError::Io {
        operation: "create partial download",
        path: destination.to_owned(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut downloaded: u64 = 0;
    loop {
        let count = response
            .read(&mut buffer)
            .map_err(|source| LocalError::DownloadRead {
                url: url.to_owned(),
                source,
            })?;
        if count == 0 {
            break;
        }
        downloaded = downloaded.saturating_add(count as u64);
        if downloaded > MAX_ARTIFACT_BYTES {
            return Err(LocalError::ArtifactTooLarge {
                url: url.to_owned(),
                limit: MAX_ARTIFACT_BYTES,
            });
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|source| LocalError::Io {
                operation: "write partial download",
                path: destination.to_owned(),
                source,
            })?;
        hasher.update(&buffer[..count]);
        progress.inc(count as u64);
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
    Ok(hex_digest(hasher))
}
