//! The best-effort HF metadata sidecar written beside a provisioned GGUF.

use std::path::Path;
use std::time::Duration;

use crate::artifacts::{self, ArtifactStore};
use crate::sidecar;

/// Best-effort: fetch HF metadata and write a sidecar `.md` beside the GGUF.
///
/// Only attempts the fetch for HF URLs. Failures are logged at debug level
/// and swallowed - the sidecar is supplementary, never required.
pub(super) fn maybe_write_sidecar(_store: &ArtifactStore, source: &str, model_path: &Path) {
    if !source.starts_with("https://huggingface.co/") {
        return;
    }
    let sidecar_file = sidecar::sidecar_path(model_path);
    if sidecar_file.is_file() {
        // Validate the existing sidecar rather than blindly trusting the file's
        // presence (SIDECAR-004): only skip the refetch when it reads back as a
        // current, usable sidecar. An unversioned, oversized, or template-less
        // sidecar falls through and is rewritten.
        match sidecar::read_sidecar(model_path) {
            Ok(Some(meta)) if sidecar_is_current(&meta, source) => {
                tracing::debug!(path = %sidecar_file.display(), "valid sidecar already exists");
                return;
            }
            _ => {
                tracing::debug!(
                    path = %sidecar_file.display(),
                    "existing sidecar invalid or incomplete; refetching"
                );
            }
        }
    }

    let client = match reqwest::blocking::Client::builder()
        .user_agent(concat!("gateway/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "could not build sidecar HTTP client");
            write_sidecar_metadata(source, model_path, None, None);
            return;
        }
    };

    let bearer = artifacts::hub_bearer_token_from_env();
    let (chat_template, fetched) =
        match sidecar::fetch_hf_chat_template(&client, source, bearer.as_deref()) {
            Ok(Some(template)) => (Some(template), Some(sidecar::utc_now_iso())),
            Ok(None) => {
                tracing::debug!(source = %source, "no chat_template from HF metadata");
                (None, Some(sidecar::utc_now_iso()))
            }
            Err(error) => {
                // Deliberate downgrade (SIDECAR-006): the sidecar is supplementary,
                // so a fetch failure is logged and skipped, not propagated. Source
                // provenance is still persisted for conservative model-ID matching.
                tracing::debug!(source = %source, error = %error, "sidecar fetch failed");
                (None, None)
            }
        };
    write_sidecar_metadata(source, model_path, fetched, chat_template);
}

pub(super) fn sidecar_is_current(metadata: &sidecar::SidecarMeta, source: &str) -> bool {
    metadata.chat_template.is_some() && metadata.source.as_deref() == Some(source)
}

pub(super) fn write_sidecar_metadata(
    source: &str,
    model_path: &Path,
    fetched: Option<String>,
    chat_template: Option<String>,
) {
    let meta = sidecar::SidecarMeta {
        source: Some(source.to_owned()),
        fetched,
        chat_template,
        card: None,
    };
    let sidecar_file = sidecar::sidecar_path(model_path);
    if let Err(e) = sidecar::write_sidecar(model_path, &meta) {
        tracing::debug!(
            path = %sidecar_file.display(),
            error = %e,
            "failed to write sidecar"
        );
    } else {
        tracing::info!(path = %sidecar_file.display(), "wrote HF metadata sidecar");
    }
}
