//! Startup flows over mock layouts: activity text and the kind preflight.

use super::*;
use crate::artifacts::ProvisionedServer;
use gateway_config::ModelKind;

#[test]
fn start_with_no_local_models_writes_no_activity_text() {
    // An empty `[[local_model]]` set is a no-op start: the caller's
    // activity text is left exactly as it was.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "remote"
context = 8192
upstream = "u"
endpoints = ["e"]
"#,
    )
    .expect("config");
    let hub = gateway_progress::ProgressHub::new();
    let activity = hub.begin("local-models");
    let runtime = LocalRuntime::start(&config, Some(&activity)).expect("empty local runtime");
    assert_eq!(runtime.child_count(), 0);
    assert_eq!(hub.current().text, "local-models");
}

#[test]
fn start_over_a_mock_layout_writes_the_starting_text_before_the_spawn() {
    use crate::testsupport::hex_sha256;

    // The mock layout provisions for real - a path source with a true pin
    // - but has no `llama-server` binary to spawn, so the start fails at
    // launch, after the activity text reached the spawn stage.
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
kind = "embedding"
description = "a mock local model"
source = '{}'
sha256 = "{}"
context = 512
"#,
        temp.path().join("cache").display(),
        model_file.display(),
        hex_sha256(b"mock-gguf-bytes"),
    ))
    .expect("config");

    let hub = gateway_progress::ProgressHub::new();
    let activity = hub.begin("local-models");
    let seen = std::sync::Mutex::new(Vec::new());

    let error = start_impl(
        &config,
        Some(&activity),
        &startup_interrupt_flag(),
        None,
        |_store, _selection, server| {
            // An already-staged server has no download/verify/extract
            // work; the runtime named the stage before calling in.
            seen.lock()
                .expect("seen lock")
                .push(server.map(|_| hub.current().text));
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
        |_, _, _, _| {
            seen.lock()
                .expect("seen lock")
                .push(Some(hub.current().text));
            Err(LocalError::EarlyExit {
                status: "the mock layout has no llama-server to spawn".to_owned(),
            })
        },
        StartPolicy::FailFast,
    )
    .expect_err("the mock layout cannot launch a real child");
    assert!(matches!(error, LocalError::EarlyExit { .. }));

    assert_eq!(
        seen.lock().expect("seen lock").as_slice(),
        [
            Some("Provisioning llama-server".to_owned()),
            Some("Starting mock".to_owned()),
        ],
        "the server stage is named before provisioning and the model before its spawn"
    );
    assert!(
        hub.current().busy,
        "the caller's activity is still live after a failed start"
    );
}

#[test]
fn start_with_an_all_speech_profile_has_no_side_effects() {
    use crate::testsupport::hex_sha256;

    // The kind preflight (A6) runs before the shared server provisions:
    // an all-speech profile collects one refusal per model and does
    // nothing else - the server provisioner never runs and the cache
    // directory is never even created.
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
    let outcome = start_impl(
        &config,
        None,
        &startup_interrupt_flag(),
        None,
        |_store, _selection, _server| {
            server_provisions.fetch_add(1, Ordering::Relaxed);
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
        |_, _, _, _| panic!("a refused model never spawns"),
        StartPolicy::KeepReady,
    )
    .expect("a per-model refusal is not fatal under the partial policy");
    assert_eq!(outcome.runtime().child_count(), 0);
    assert_eq!(outcome.failures().len(), 1);
    assert!(
        matches!(
            outcome.failures()[0].error(),
            LocalError::UnsupportedKind {
                kind: ModelKind::Speech
            }
        ),
        "the refusal names the speech kind: {:?}",
        outcome.failures()[0].error()
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
fn start_with_a_mixed_profile_provisions_the_server_once() {
    use crate::testsupport::hex_sha256;

    // A mixed profile keeps supported-model progress: the server
    // provisions once, the embedding model's blob is verified into the
    // cache and reaches the spawn, and the speech model is refused as a
    // per-model failure whose blob is never provisioned.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let embed_file = temp.path().join("embed.gguf");
    std::fs::write(&embed_file, b"mock-embed-bytes").expect("write embed model");
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
name = "embed"
kind = "embedding"
description = "a local embedding model"
source = '{}'
sha256 = "{}"
context = 512
[[local_model]]
name = "tts"
kind = "speech"
description = "a local speech model"
source = '{}'
context = 4096
"#,
        cache_dir.display(),
        embed_file.display(),
        hex_sha256(b"mock-embed-bytes"),
        tts_file.display(),
    ))
    .expect("config");
    let server_provisions = std::sync::atomic::AtomicUsize::new(0);
    let spawns = std::sync::atomic::AtomicUsize::new(0);
    let outcome = start_impl(
        &config,
        None,
        &startup_interrupt_flag(),
        None,
        |_store, _selection, _server| {
            server_provisions.fetch_add(1, Ordering::Relaxed);
            Ok(ProvisionedServer {
                executable: PathBuf::from("mock-llama-server"),
                path_prefix: Vec::new(),
            })
        },
        |_, _, _, _| {
            spawns.fetch_add(1, Ordering::Relaxed);
            Err(LocalError::EarlyExit {
                status: "the mock layout has no llama-server to spawn".to_owned(),
            })
        },
        StartPolicy::KeepReady,
    )
    .expect("per-model failures are not fatal under the partial policy");
    assert_eq!(outcome.runtime().child_count(), 0);
    assert_eq!(
        server_provisions.load(Ordering::Relaxed),
        1,
        "the shared server provisions once for the supported model"
    );
    assert_eq!(
        spawns.load(Ordering::Relaxed),
        1,
        "only the supported model reaches the spawn"
    );
    assert!(
        outcome.failures().iter().any(|failure| {
            failure.model() == "tts"
                && matches!(
                    failure.error(),
                    LocalError::UnsupportedKind {
                        kind: ModelKind::Speech
                    }
                )
        }),
        "the speech refusal is a per-model failure naming the kind: {:?}",
        outcome.failures()
    );
    let embed_key = artifacts::source_cache_key(&embed_file.to_string_lossy());
    let tts_key = artifacts::source_cache_key(&tts_file.to_string_lossy());
    assert!(
        cache_dir
            .join("markers")
            .join(format!("{embed_key}.verified"))
            .is_file(),
        "the supported model's blob is verified into the cache"
    );
    assert!(
        !cache_dir
            .join("markers")
            .join(format!("{tts_key}.verified"))
            .exists(),
        "the refused speech model's blob is never provisioned"
    );
}
