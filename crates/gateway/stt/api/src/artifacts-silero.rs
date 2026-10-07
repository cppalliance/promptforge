//! The pinned Silero VAD model, provisioned beside the whisper models.

use std::path::PathBuf;

use gateway_config::SILERO_VAD_MODEL;
use gateway_local::artifacts::ArtifactStore;
use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

use super::{SpeechError, cancelled_or};

/// Where the Silero model comes from and the digest it must match.
#[derive(Debug, Clone, Copy)]
pub(super) struct SileroPin<'a> {
    pub(super) source: &'a str,
    pub(super) sha256: &'a str,
}

impl SileroPin<'static> {
    pub(super) const PINNED: Self = Self {
        source: SILERO_VAD_MODEL.source(),
        sha256: SILERO_VAD_MODEL.sha256(),
    };
}

/// Fetches the Silero model and checks its digest, cache hits included,
/// returning the verified path. Any failure fails the load: the load's
/// cancellation as [`SpeechError::InitialLoadCancelled`], anything else as
/// [`SpeechError::Silero`] carrying the cause.
pub(super) fn provision(
    store: &ArtifactStore,
    pin: SileroPin<'_>,
    activity: Option<&Activity>,
    cancel: &CancellationToken,
) -> Result<PathBuf, SpeechError> {
    let path = store
        .ensure_model_with_cancellation(pin.source, Some(pin.sha256), activity, Some(cancel))
        .map_err(|source| cancelled_or(source, SpeechError::Silero))?;
    tracing::info!(path = %path.display(), "provisioned Silero VAD model");
    Ok(path)
}

/// A Silero model source and digest a scripted load provisions through an
/// artifact store at `cache`, as the Whisper backend's load does.
#[cfg(feature = "test-fixtures")]
#[derive(Debug, Clone)]
pub(crate) struct ScriptedSileroPin {
    pub(crate) cache: PathBuf,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[cfg(feature = "test-fixtures")]
impl ScriptedSileroPin {
    /// Provisions the pinned model, failing the way [`provision`] does.
    pub(crate) fn provision(&self, cancel: &CancellationToken) -> Result<PathBuf, SpeechError> {
        let store = ArtifactStore::new(self.cache.clone()).map_err(SpeechError::Store)?;
        let pin = SileroPin {
            source: &self.source,
            sha256: &self.sha256,
        };
        provision(&store, pin, None, cancel)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use gateway_local::LocalError;
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    use super::super::tests::selected;
    use super::super::{PreparedGeneration, SpeechError, prepare_impl};
    use super::SileroPin;

    fn hex_sha256(bytes: &[u8]) -> String {
        let mut hex = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            write!(&mut hex, "{byte:02x}").expect("writing to String is infallible");
        }
        hex
    }

    /// Prepares speech whose interim model and whisper library are local
    /// stand-ins, so the Silero pin alone decides the Silero outcome.
    fn prepare_with(
        dir: &Path,
        silero: SileroPin<'_>,
        cancel: &CancellationToken,
    ) -> Result<PreparedGeneration, SpeechError> {
        let model = dir.join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let cache = dir.join("cache").display().to_string();
        let sections = format!("[local]\ncache_dir = {cache:?}\n");
        let config = selected(&model.display().to_string(), None, &sections);
        let library = dir.join("whisper-library");
        prepare_impl(
            &config,
            None,
            cancel,
            |_store, _backend, _activity, _token| Ok(library.clone()),
            silero,
        )
        .map(|prepared| prepared.generation.expect("a speech model prepares"))
    }

    /// The provisioning failure `error` carries, or a panic naming `error`.
    fn silero_cause(error: &SpeechError) -> &LocalError {
        let SpeechError::Silero(cause) = error else {
            panic!("expected a Silero provisioning failure, got {error:?}");
        };
        cause
    }

    #[test]
    fn the_whisper_library_and_verified_silero_model_paths_reach_the_prepared_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let silero = dir.path().join("ggml-silero.bin");
        std::fs::write(&silero, b"silero bytes").expect("fixture writes");
        let source = silero.display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let generation =
            prepare_with(dir.path(), pin, &CancellationToken::new()).expect("speech prepares");

        assert_eq!(generation.library, dir.path().join("whisper-library"));
        assert_eq!(generation.silero, silero);
    }

    #[test]
    fn a_silero_digest_mismatch_fails_the_load_naming_the_cause() {
        let dir = tempfile::tempdir().expect("tempdir");
        let silero = dir.path().join("ggml-silero.bin");
        std::fs::write(&silero, b"tampered bytes").expect("fixture writes");
        let source = silero.display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let error = prepare_with(dir.path(), pin, &CancellationToken::new())
            .expect_err("a tampered Silero model fails the load");

        let cause = silero_cause(&error);
        assert!(
            matches!(cause, LocalError::DigestMismatch { .. }),
            "{cause}"
        );
        assert!(cause.to_string().contains("sha-256 mismatch"), "{cause}");
    }

    #[test]
    fn a_failed_silero_fetch_fails_the_load_naming_the_cause() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("missing-silero.bin").display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let error = prepare_with(dir.path(), pin, &CancellationToken::new())
            .expect_err("a missing Silero model fails the load");

        let cause = silero_cause(&error);
        assert!(matches!(cause, LocalError::InvalidSource { .. }), "{cause}");
        assert!(
            cause.to_string().contains("not an existing file"),
            "{cause}"
        );
    }

    #[test]
    fn a_cancelled_silero_fetch_cancels_the_load() {
        // The port is bound and dropped, so a request would fail as a
        // transport error; `InitialLoadCancelled` proves none was made.
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr")
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let source = format!("https://{addr}/ggml-silero.bin");
        let sha256 = "0".repeat(64);
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = prepare_with(dir.path(), pin, &cancel)
            .expect_err("a cancelled Silero fetch cancels the load");

        assert!(
            matches!(error, SpeechError::InitialLoadCancelled),
            "{error:?}"
        );
    }
}
