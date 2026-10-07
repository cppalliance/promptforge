//! The pinned Silero VAD model, provisioned beside the whisper models.

use std::path::PathBuf;
use std::sync::Arc;

use gateway_config::SILERO_VAD_MODEL;
use gateway_local::LocalError;
use gateway_local::artifacts::ArtifactStore;
use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

use super::SpeechError;

/// A generation's digest-verified Silero model path, or the cause it has
/// none, which the load already reported.
pub(crate) type SileroModel = Result<PathBuf, Arc<LocalError>>;

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

/// Fetches the Silero model and checks its digest, cache hits included.
/// Only the load's cancellation fails the load; any other failure is
/// reported here, once, and leaves the generation without the model.
pub(super) fn provision(
    store: &ArtifactStore,
    pin: SileroPin<'_>,
    activity: Option<&Activity>,
    cancel: &CancellationToken,
) -> Result<SileroModel, SpeechError> {
    match store.ensure_model_with_cancellation(pin.source, Some(pin.sha256), activity, Some(cancel))
    {
        Ok(path) => {
            tracing::info!(path = %path.display(), "provisioned Silero VAD model");
            Ok(Ok(path))
        }
        Err(LocalError::Cancelled) => Err(SpeechError::InitialLoadCancelled),
        Err(error) => {
            let cause = chain(&error);
            tracing::warn!(
                cause = %cause,
                "Silero VAD model unavailable; speech detection uses loudness"
            );
            if let Some(activity) = activity {
                activity.set_text(format!("Silero VAD model unavailable: {cause}"));
            }
            Ok(Err(Arc::new(error)))
        }
    }
}

/// `error` and each of its sources, joined by `: `.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use gateway_local::LocalError;
    use gateway_progress::{Activity, ProgressHub};
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    use super::super::tests::selected;
    use super::super::{PreparedGeneration, SpeechError, prepare_impl};
    use super::SileroPin;

    /// The message of every WARN-level event.
    #[derive(Clone, Default)]
    struct Warnings(Arc<Mutex<Vec<String>>>);

    struct Message(String);

    impl tracing::field::Visit for Message {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl tracing::Subscriber for Warnings {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                let mut message = Message(String::new());
                event.record(&mut message);
                self.0.lock().expect("warning capture lock").push(message.0);
            }
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

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
        progress: Option<&Arc<Activity>>,
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
            progress,
            cancel,
            |_store, _backend, _activity, _token| Ok(library.clone()),
            silero,
        )
        .map(|prepared| prepared.generation.expect("a speech model prepares"))
    }

    /// Prepares with an unusable Silero `pin`, returning the generation's
    /// Silero cause, the progress text the load left, and every warning.
    fn prepare_unavailable(
        dir: &Path,
        pin: SileroPin<'_>,
    ) -> (Arc<LocalError>, String, Vec<String>) {
        let hub = ProgressHub::new();
        let activity = Arc::new(hub.begin("Loading speech"));
        let warnings = Warnings::default();
        let generation = tracing::subscriber::with_default(warnings.clone(), || {
            prepare_with(dir, pin, Some(&activity), &CancellationToken::new())
        })
        .expect("the generation loads without its Silero model");
        let cause = generation
            .silero
            .expect_err("an unusable Silero model leaves no path");
        let text = hub.current().text;
        let warnings = std::mem::take(&mut *warnings.0.lock().expect("warning capture lock"));
        (cause, text, warnings)
    }

    #[test]
    fn a_verified_silero_model_path_reaches_the_prepared_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let silero = dir.path().join("ggml-silero.bin");
        std::fs::write(&silero, b"silero bytes").expect("fixture writes");
        let source = silero.display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let generation = prepare_with(dir.path(), pin, None, &CancellationToken::new())
            .expect("speech prepares");

        assert_eq!(generation.silero.as_ref().ok(), Some(&silero));
    }

    #[test]
    fn a_silero_digest_mismatch_leaves_no_path_reports_once_and_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let silero = dir.path().join("ggml-silero.bin");
        std::fs::write(&silero, b"tampered bytes").expect("fixture writes");
        let source = silero.display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let (cause, text, warnings) = prepare_unavailable(dir.path(), pin);

        assert!(
            matches!(*cause, LocalError::DigestMismatch { .. }),
            "{cause}"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(text.contains("sha-256 mismatch"), "{text}");
    }

    #[test]
    fn a_failed_silero_fetch_leaves_no_path_reports_once_and_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("missing-silero.bin").display().to_string();
        let sha256 = hex_sha256(b"silero bytes");
        let pin = SileroPin {
            source: &source,
            sha256: &sha256,
        };

        let (cause, text, warnings) = prepare_unavailable(dir.path(), pin);

        assert!(
            matches!(*cause, LocalError::InvalidSource { .. }),
            "{cause}"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(text.contains("not an existing file"), "{text}");
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

        let error = prepare_with(dir.path(), pin, None, &cancel)
            .expect_err("a cancelled Silero fetch cancels the load");

        assert!(
            matches!(error, SpeechError::InitialLoadCancelled),
            "{error:?}"
        );
    }
}
