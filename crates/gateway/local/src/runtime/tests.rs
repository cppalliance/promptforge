//! Local runtime startup, sidecar, and partial-policy tests.

use super::hf_sidecar::{sidecar_is_current, write_sidecar_metadata};
use super::start::retain_start;
use super::*;
use crate::sidecar;
use gateway_config::Config;
use tokio_util::sync::CancellationToken;

mod launch_options;
mod provision;
mod startup;

#[test]
fn sidecar_keeps_model_id_provenance_when_remote_template_is_unavailable() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let model_path = temp.path().join("model.gguf");
    let source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/model.gguf";

    write_sidecar_metadata(source, &model_path, None, None);

    let metadata = sidecar::read_sidecar(&model_path)
        .expect("read provenance sidecar")
        .expect("sidecar exists");
    assert_eq!(
        metadata.source_model_id().as_deref(),
        Some("unsloth/gemma-4-E2B-it-GGUF")
    );
    assert!(metadata.fetched.is_none());
    assert!(metadata.chat_template.is_none());
}

#[test]
fn sidecar_cache_hit_requires_template_and_matching_source_provenance() {
    let source = "https://huggingface.co/org/model/resolve/main/model.gguf";
    let mut metadata = sidecar::SidecarMeta {
        source: Some(source.to_owned()),
        fetched: None,
        chat_template: Some("{{ messages }}".to_owned()),
        card: None,
    };
    assert!(sidecar_is_current(&metadata, source));

    metadata.source = Some("https://huggingface.co/other/model/resolve/main/model.gguf".to_owned());
    assert!(!sidecar_is_current(&metadata, source));
    metadata.source = Some(source.to_owned());
    metadata.chat_template = None;
    assert!(!sidecar_is_current(&metadata, source));
}

#[test]
fn empty_local_models_starts_noop_runtime() {
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
    let runtime = LocalRuntime::start(&config, None).expect("empty local runtime");
    assert_eq!(runtime.child_count(), 0);
    assert!(runtime.models().is_empty());
    assert!(runtime.diagnostics().is_empty());
}

#[test]
fn start_partial_with_no_local_models_returns_an_empty_runtime_and_no_failures() {
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test"
"#,
    )
    .expect("config");
    let outcome = LocalRuntime::start_partial(&config, None).expect("empty partial start");
    assert_eq!(outcome.runtime().child_count(), 0);
    assert!(outcome.failures().is_empty());
    let (runtime, failures) = outcome.into_parts();
    assert_eq!(runtime.child_count(), 0);
    assert!(failures.is_empty());
}

#[test]
fn partial_policy_retains_successes_and_collects_each_failure() {
    let mut started = Vec::new();
    let mut failures = Vec::new();
    retain_start(
        StartPolicy::KeepReady,
        "ready",
        Ok(7),
        &mut started,
        &mut failures,
    )
    .expect("partial startup keeps ready models");
    retain_start(
        StartPolicy::KeepReady,
        "first",
        Err(LocalError::EarlyExit {
            status: "first stopped".to_owned(),
        }),
        &mut started,
        &mut failures,
    )
    .expect("partial startup continues");
    retain_start(
        StartPolicy::KeepReady,
        "second",
        Err(LocalError::EarlyExit {
            status: "second stopped".to_owned(),
        }),
        &mut started,
        &mut failures,
    )
    .expect("partial startup continues");

    assert_eq!(started, [7]);
    assert_eq!(
        failures
            .iter()
            .map(LocalStartFailure::model)
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn cancellation_is_fatal_even_under_the_partial_policy() {
    // A cancelled command must not start later models: the Cancelled
    // error escapes instead of being collected as a per-model failure.
    let mut started: Vec<()> = Vec::new();
    let mut failures = Vec::new();
    let error = retain_start(
        StartPolicy::KeepReady,
        "cancelled",
        Err(LocalError::Cancelled),
        &mut started,
        &mut failures,
    )
    .expect_err("cancellation aborts the start");
    assert!(matches!(error, LocalError::Cancelled));
    assert!(started.is_empty() && failures.is_empty());
}

#[test]
fn a_pre_cancelled_token_starts_nothing() {
    // The token is checked before the server binary provisions: a
    // cancelled start does no download and no spawn. The config's
    // deliberately missing `llama_server_path` proves the point - an
    // uncancelled start would fail with `InvalidSource` instead.
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
    let interrupted = Arc::new(AtomicBool::new(false));
    let error = LocalRuntime::start_partial_with_cancellation(&config, None, &token, &interrupted)
        .expect_err("a cancelled start provisions nothing");
    assert!(
        matches!(error, LocalError::Cancelled),
        "the token check precedes provisioning: {error:?}"
    );
}

#[test]
fn unload_model_on_an_empty_runtime_returns_none() {
    let mut runtime = LocalRuntime::empty();
    assert!(runtime.unload_model("ghost").is_none());
    assert_eq!(runtime.child_count(), 0);
}
