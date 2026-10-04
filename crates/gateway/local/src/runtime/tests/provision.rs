//! Artifact-only provisioning tests: staged blobs, cancellation, and the kind preflight.

use super::*;
use crate::artifacts::ProvisionedServer;
use gateway_config::ModelKind;

#[test]
fn provision_artifacts_stages_every_blob_and_spawns_nothing() {
    use crate::testsupport::hex_sha256;

    // The artifact step provisions for real - the pinned path source is
    // hashed and its verification marker written - and never reaches a
    // spawn. A model whose source is missing is collected as a per-model
    // failure, not a fatal one, so the model that did provision is not
    // held back.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let model_file = temp.path().join("mock.gguf");
    std::fs::write(&model_file, b"mock-gguf-bytes").expect("write model");
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
cache_dir = '{}'

[[local_model]]
name = "mock"
description = "a mock local model"
source = '{}'
sha256 = "{}"
context = 512

[[local_model]]
name = "absent"
description = "a model whose source is missing"
source = '{}'
context = 512
"#,
        temp.path().join("cache").display(),
        model_file.display(),
        hex_sha256(b"mock-gguf-bytes"),
        temp.path().join("absent.gguf").display(),
    ))
    .expect("config");

    let hub = gateway_progress::ProgressHub::new();
    let activity = hub.begin("downloading-models");
    let failures = provision_artifacts_impl(
        &config,
        Some(&activity),
        &CancellationToken::new(),
        |_store, _selection, _server| {
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
    )
    .expect("the artifact step tolerates a per-model failure");
    assert_eq!(
        failures
            .iter()
            .map(LocalStartFailure::model)
            .collect::<Vec<_>>(),
        ["absent"]
    );
    assert!(matches!(
        failures[0].error(),
        LocalError::InvalidSource { .. }
    ));

    let key = artifacts::source_cache_key(&model_file.to_string_lossy());
    assert!(
        temp.path()
            .join("cache")
            .join("markers")
            .join(format!("{key}.verified"))
            .is_file(),
        "the pinned blob is verified into the cache, so the start finds it"
    );
    assert_eq!(
        hub.current().text,
        "Verifying mock.gguf 100%",
        "the last stage the artifact step wrote is the pinned model's hash pass: nothing spawns"
    );
}

#[test]
fn provision_artifacts_with_a_cancelled_token_provisions_nothing() {
    // The token is checked before the server binary provisions; the
    // deliberately missing `llama_server_path` would otherwise fail with
    // `InvalidSource`.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
llama_server_path = "/definitely/missing/llama-server"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
"#,
    )
    .expect("config");
    let token = CancellationToken::new();
    token.cancel();
    let error = LocalRuntime::provision_artifacts_with_cancellation(&config, None, &token)
        .expect_err("a cancelled artifact step provisions nothing");
    assert!(
        matches!(error, LocalError::Cancelled),
        "the token check precedes provisioning: {error:?}"
    );
}

#[test]
fn provision_artifacts_with_an_all_speech_profile_has_no_side_effects() {
    use crate::testsupport::hex_sha256;

    // The kind preflight (A6) runs before the shared server or any model
    // provisions: an all-speech profile collects one refusal per model
    // and does nothing else - the server provisioner never runs and the
    // cache directory is never even created.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let model_file = temp.path().join("tts.gguf");
    std::fs::write(&model_file, b"mock-tts-bytes").expect("write model");
    let cache_dir = temp.path().join("cache");
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
cache_dir = '{}'

[[local_model]]
name = "tts"
kind = "speech"
description = "a local speech model"
source = '{}'
sha256 = "{}"
context = 4096
"#,
        cache_dir.display(),
        model_file.display(),
        hex_sha256(b"mock-tts-bytes"),
    ))
    .expect("config");

    let server_provisions = std::sync::atomic::AtomicUsize::new(0);
    let failures = provision_artifacts_impl(
        &config,
        None,
        &CancellationToken::new(),
        |_store, _selection, _server| {
            server_provisions.fetch_add(1, Ordering::Relaxed);
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
    )
    .expect("an unsupported kind is a per-model failure, not a fatal one");
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].model(), "tts");
    assert!(
        matches!(
            failures[0].error(),
            LocalError::UnsupportedKind {
                kind: ModelKind::Speech
            }
        ),
        "the refusal names the speech kind: {:?}",
        failures[0].error()
    );
    assert_eq!(
        server_provisions.load(Ordering::Relaxed),
        0,
        "the shared server is never provisioned"
    );
    assert!(
        !cache_dir.exists(),
        "the model store is never touched: the cache directory is never created"
    );
}

#[test]
fn provision_artifacts_with_a_mixed_profile_provisions_only_supported_models() {
    use crate::testsupport::hex_sha256;

    // A mixed profile keeps supported-model progress: the chat model's
    // blob is verified into the cache while the speech model is refused
    // as a per-model failure and never provisioned.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let chat_file = temp.path().join("chat.gguf");
    std::fs::write(&chat_file, b"mock-chat-bytes").expect("write chat model");
    let tts_file = temp.path().join("tts.gguf");
    std::fs::write(&tts_file, b"mock-tts-bytes").expect("write tts model");
    let cache_dir = temp.path().join("cache");
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
cache_dir = '{}'

[[local_model]]
name = "chat"
description = "a local chat model"
source = '{}'
sha256 = "{}"
context = 4096

[[local_model]]
name = "tts"
kind = "speech"
description = "a local speech model"
source = '{}'
sha256 = "{}"
context = 4096
"#,
        cache_dir.display(),
        chat_file.display(),
        hex_sha256(b"mock-chat-bytes"),
        tts_file.display(),
        hex_sha256(b"mock-tts-bytes"),
    ))
    .expect("config");

    let server_provisions = std::sync::atomic::AtomicUsize::new(0);
    let failures = provision_artifacts_impl(
        &config,
        None,
        &CancellationToken::new(),
        |_store, _selection, _server| {
            server_provisions.fetch_add(1, Ordering::Relaxed);
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
    )
    .expect("a per-model refusal is not fatal");
    assert_eq!(
        failures
            .iter()
            .map(LocalStartFailure::model)
            .collect::<Vec<_>>(),
        ["tts"]
    );
    assert!(
        matches!(
            failures[0].error(),
            LocalError::UnsupportedKind {
                kind: ModelKind::Speech
            }
        ),
        "the refusal names the speech kind: {:?}",
        failures[0].error()
    );
    assert_eq!(
        server_provisions.load(Ordering::Relaxed),
        1,
        "the shared server provisions once for the supported model"
    );
    let chat_key = artifacts::source_cache_key(&chat_file.to_string_lossy());
    assert!(
        cache_dir
            .join("markers")
            .join(format!("{chat_key}.verified"))
            .is_file(),
        "the supported model's blob is verified into the cache"
    );
    let tts_key = artifacts::source_cache_key(&tts_file.to_string_lossy());
    assert!(
        !cache_dir
            .join("markers")
            .join(format!("{tts_key}.verified"))
            .exists(),
        "the refused speech model's blob is never provisioned"
    );
}
