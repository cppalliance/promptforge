//! Tests for speech model provisioning and the load's library handoff.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use super::*;

/// A Silero pin naming no file, so a load reaching it fails instead of
/// downloading the real model.
const NO_SILERO: SileroPin<'static> = SileroPin {
    source: "/missing-silero.bin",
    sha256: "0000000000000000000000000000000000000000000000000000000000000000",
};

/// A selected profile whose one interim model is `source`. `sections`
/// holds whole top-level tables, such as `[local]` or `[stt]`.
pub(super) fn selected(source: &str, sha256: Option<&str>, sections: &str) -> Config {
    let pin = sha256.map_or_else(String::new, |pin| format!("sha256 = \"{pin}\"\n"));
    let catalog = Config::from_toml_str(&format!(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [workshop]\n\
         {sections}\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
         {pin}vram_gb = 1.0\n\
         [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
    ))
    .expect("catalog parses");
    catalog
        .select_profile(Some(
            &gateway_config::ProfileName::parse("work").expect("name"),
        ))
        .expect("profile selects")
}

#[test]
fn a_pinned_model_rejects_the_wrong_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("model.bin");
    std::fs::write(&model, b"model bytes").expect("fixture writes");
    let config = selected(&model.display().to_string(), Some(&"0".repeat(64)), "");
    let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
    let error = provision_models(&config, &store, None, &CancellationToken::new())
        .expect_err("bad pin must fail");
    assert!(matches!(error, SpeechError::Artifact { .. }));
}

#[test]
fn an_unpinned_local_model_provisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("model.bin");
    std::fs::write(&model, b"model bytes").expect("fixture writes");
    let config = selected(&model.display().to_string(), None, "");
    let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
    let provisioned = provision_models(&config, &store, None, &CancellationToken::new())
        .expect("unpinned path works");
    assert_eq!(
        provisioned.interim.as_ref().map(|(_, path)| path),
        Some(&model)
    );
}

#[test]
fn a_pinned_model_accepts_the_matching_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("model.bin");
    std::fs::write(&model, b"model bytes").expect("fixture writes");
    let mut pin = String::with_capacity(64);
    for byte in Sha256::digest(b"model bytes") {
        write!(&mut pin, "{byte:02x}").expect("writing to String is infallible");
    }
    let config = selected(&model.display().to_string(), Some(&pin), "");
    let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
    let provisioned = provision_models(&config, &store, None, &CancellationToken::new())
        .expect("matching pin works");
    assert_eq!(
        provisioned.interim.as_ref().map(|(_, path)| path),
        Some(&model)
    );
}

#[test]
fn prepare_passes_the_stt_whisper_backend_to_the_library_provision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("model.bin");
    std::fs::write(&model, b"model bytes").expect("fixture writes");
    let cache = dir.path().join("cache").display().to_string();
    let library = dir.path().join("whisper-library");
    for (stt, expected) in [
        ("", WhisperBackend::Auto),
        ("[stt]\nwhisper_backend = \"cpu\"\n", WhisperBackend::Cpu),
        ("[stt]\nwhisper_backend = \"cuda\"\n", WhisperBackend::Cuda),
    ] {
        let sections = format!("[local]\ncache_dir = {cache:?}\n{stt}");
        let config = selected(&model.display().to_string(), None, &sections);
        let mut received = None;
        let error = prepare_impl(
            &config,
            None,
            &CancellationToken::new(),
            |_store, backend, _activity, _token| {
                received = Some(backend);
                Ok(library.clone())
            },
            NO_SILERO,
        )
        .expect_err("the missing Silero model fails the load");
        assert_eq!(received, Some(expected), "{stt:?}");
        assert!(
            matches!(error, SpeechError::Silero(_)),
            "{stt:?}: {error:?}"
        );
    }
}

#[test]
fn prepare_hands_the_load_token_to_the_library_provision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("model.bin");
    std::fs::write(&model, b"model bytes").expect("fixture writes");
    let cache = dir.path().join("cache").display().to_string();
    let sections = format!("[local]\ncache_dir = {cache:?}\n");
    let config = selected(&model.display().to_string(), None, &sections);
    let cancel = CancellationToken::new();
    let load_token = &cancel;
    let mut handed = false;
    let error = prepare_impl(
        &config,
        None,
        load_token,
        |_store, _backend, _activity, token| {
            handed = token.is_some_and(|token| std::ptr::eq(token, load_token));
            Err(gateway_local::LocalError::Cancelled)
        },
        NO_SILERO,
    )
    .expect_err("a cancelled library provision fails the load");
    assert!(handed, "the provision received the load's own token");
    assert!(
        matches!(error, SpeechError::InitialLoadCancelled),
        "a cancelled provision is the load's cancellation: {error:?}"
    );
}

#[test]
fn a_fired_token_stops_a_speech_model_download() {
    // The port is bound and dropped, so a request would fail as a
    // transport error; `InitialLoadCancelled` proves none was made.
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr")
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = dir.path().join("cache").display().to_string();
    let sections = format!("[local]\ncache_dir = {cache:?}\n");
    let source = format!("https://{addr}/ggml-speech.bin");
    let config = selected(&source, Some(&"0".repeat(64)), &sections);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let library = dir.path().join("whisper-library");
    let error = prepare_impl(
        &config,
        None,
        &cancel,
        |_store, _backend, _activity, _token| Ok(library.clone()),
        NO_SILERO,
    )
    .expect_err("a fired token stops the model download");
    assert!(
        matches!(error, SpeechError::InitialLoadCancelled),
        "a cancelled model download is the load's cancellation: {error:?}"
    );
}
