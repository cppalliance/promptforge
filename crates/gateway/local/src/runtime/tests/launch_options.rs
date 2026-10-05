//! Launch option tests: parallel and dominion admission, kind flags, and companions.

use super::super::launch::{launch_options, provision_companions, resolve_admission};
use super::*;
use crate::server::ServeMode;
use gateway_config::ModelKind;
use gateway_routing::dominion_queues;

#[tokio::test]
async fn parallel_field_feeds_parallel_arg_and_queue_limit() {
    // A local model with `parallel = 3` launches its child with
    // `--parallel 3` (launch_options holds the number; the server tests
    // prove it renders into the argv) and admits at most 3 concurrent
    // requests through its per-model queue.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
parallel = 3
"#,
    )
    .expect("config");
    let model = &config.local_models()[0];
    let queues = dominion_queues(&config);
    let admission = resolve_admission(&queues, model).expect("admission");

    assert_eq!(admission.parallel, 3);
    assert_eq!(
        launch_options(model, admission.parallel)
            .expect("launch options")
            .parallel,
        3
    );

    let _first = admission.queue.admit("client").await.unwrap();
    let _second = admission.queue.admit("client").await.unwrap();
    let third = admission.queue.admit("client").await.unwrap();

    // The fourth request exceeds the limit and parks as a waiter.
    let queue = admission.queue.clone();
    let blocked = tokio::spawn(async move { queue.admit("client").await });
    while admission.queue.waiter_count() != 1 {
        tokio::task::yield_now().await;
    }

    // Releasing a slot hands it to the parked waiter.
    drop(third);
    let _promoted = blocked.await.unwrap().unwrap();
}

#[tokio::test]
async fn local_models_on_one_dominion_share_one_limit() {
    // Two local models bound to one local dominion compete for a single
    // pool of slots: filling the only slot through one model's binding
    // parks the other model's admit.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 1

[[local_model]]
name = "a"
description = "model a"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"

[[local_model]]
name = "b"
description = "model b"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
"#,
    )
    .expect("config");
    let queues = dominion_queues(&config);
    let admission_a = resolve_admission(&queues, &config.local_models()[0]).expect("admission a");
    let admission_b = resolve_admission(&queues, &config.local_models()[1]).expect("admission b");
    // No `parallel` set: the child `--parallel` defaults to 1.
    assert_eq!(admission_a.parallel, 1);
    assert_eq!(admission_b.parallel, 1);

    let held = admission_a.queue.admit("client").await.unwrap();
    let queue_b = admission_b.queue.clone();
    let blocked = tokio::spawn(async move { queue_b.admit("client").await });
    while admission_a.queue.waiter_count() != 1 {
        tokio::task::yield_now().await;
    }

    drop(held);
    let _permit = blocked.await.unwrap().unwrap();
}

#[test]
fn embedding_kind_sets_the_embeddings_launch_flag() {
    // `kind = "embedding"` maps to the child's `--embeddings` flag
    // (launch_options holds it; the server tests prove it renders into
    // the argv); a chat child launches without it.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "embed"
kind = "embedding"
description = "a local embedding model"
source = "/models/embed.gguf"
context = 512

[[local_model]]
name = "chatty"
description = "a local chat model"
source = "/models/chat.gguf"
context = 4096
"#,
    )
    .expect("config");
    let embed = &config.local_models()[0];
    let chat = &config.local_models()[1];
    assert_eq!(
        launch_options(embed, 1).expect("launch options").serve_mode,
        ServeMode::Embeddings
    );
    assert_eq!(
        launch_options(chat, 1).expect("launch options").serve_mode,
        ServeMode::Chat
    );
}

#[test]
fn classifier_kind_sets_the_reranking_launch_flag() {
    // `kind = "classifier"` maps to the child's `--reranking` flag
    // (launch_options holds it; the server tests prove it renders into
    // the argv); a chat child launches without it.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "rerank"
kind = "classifier"
description = "a local classifier model"
source = "/models/rerank.gguf"
context = 512

[[local_model]]
name = "chatty"
description = "a local chat model"
source = "/models/chat.gguf"
context = 4096
"#,
    )
    .expect("config");
    let classifier = &config.local_models()[0];
    let chat = &config.local_models()[1];
    assert_eq!(
        launch_options(classifier, 1)
            .expect("launch options")
            .serve_mode,
        ServeMode::Reranking
    );
    assert_eq!(
        launch_options(chat, 1).expect("launch options").serve_mode,
        ServeMode::Chat
    );
}

#[test]
fn speech_kind_refuses_to_launch_as_chat() {
    // A speech model has no `llama-server` serve mode: `launch_options`
    // errors rather than falling through to the chat default.
    let config = Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "tts"
kind = "speech"
description = "a local speech model"
source = "/models/tts.gguf"
context = 4096
"#,
    )
    .expect("config");
    let speech = &config.local_models()[0];
    let error = launch_options(speech, 1).expect_err("a speech model must not launch as chat");
    assert!(
        matches!(
            error,
            LocalError::UnsupportedKind {
                kind: ModelKind::Speech
            }
        ),
        "the refusal names the speech kind: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "local speech models are not yet supported"
    );
}

fn companion_config(body: &str) -> Config {
    Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
{body}"#
    ))
    .expect("config")
}

#[test]
fn provision_companions_resolve_to_independent_pinned_slots() {
    // Each companion resolves through `ensure_model` under its own source
    // identity and its own pin: a shared verification state or a dropped
    // pin breaks the distinct markers, and a wiring slip breaks the
    // resolved paths or the draft maximum.
    use crate::testsupport::hex_sha256;

    let source_dir = tempfile::TempDir::new().expect("source dir");
    let draft = source_dir.path().join("draft.gguf");
    let projector = source_dir.path().join("mmproj.gguf");
    std::fs::write(&draft, b"draft-bytes").expect("write draft");
    std::fs::write(&projector, b"projector-bytes").expect("write projector");
    let config = companion_config(&format!(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = '{}'
sha256 = "{}"
draft_max = 2

[local_model.multimodal_projector]
source = '{}'
sha256 = "{}"
"#,
        draft.display(),
        hex_sha256(b"draft-bytes"),
        projector.display(),
        hex_sha256(b"projector-bytes"),
    ));
    let model = &config.local_models()[0];
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");

    let mut options = launch_options(model, 1).expect("launch options");
    provision_companions(&store, model, &mut options, None).expect("provision companions");

    let speculative = options.speculative.expect("speculative launch state");
    assert_eq!(speculative.draft_model, draft);
    assert_eq!(speculative.draft_max, 2);
    assert_eq!(
        options.multimodal_projector.expect("projector path"),
        projector
    );

    // Each pinned path source records its own verification marker, keyed
    // by its own source identity.
    let draft_key = artifacts::source_cache_key(&draft.to_string_lossy());
    let projector_key = artifacts::source_cache_key(&projector.to_string_lossy());
    assert_ne!(draft_key, projector_key);
    let markers = temp.path().join("markers");
    assert!(markers.join(format!("{draft_key}.verified")).is_file());
    assert!(markers.join(format!("{projector_key}.verified")).is_file());
}

#[test]
fn companion_provisioning_failures_precede_child_spawn() {
    // An unresolvable or pin-mismatching companion fails inside
    // `provision_companions`, which `LocalRuntime::start` calls before
    // `ServerGuard::start`: the error is a `LocalError` from provisioning,
    // never a spawned-then-failing server.
    use crate::testsupport::hex_sha256;

    let source_dir = tempfile::TempDir::new().expect("source dir");
    let draft = source_dir.path().join("draft.gguf");
    std::fs::write(&draft, b"real-draft-bytes").expect("write draft");
    let mismatching = companion_config(&format!(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = '{}'
sha256 = "{}"
draft_max = 2
"#,
        draft.display(),
        hex_sha256(b"different-bytes"),
    ));
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let model = &mismatching.local_models()[0];
    let mut options = launch_options(model, 1).expect("launch options");
    let error = provision_companions(&store, model, &mut options, None)
        .expect_err("pin mismatch must fail provisioning");
    assert!(matches!(error, LocalError::DigestMismatch { .. }));
    assert!(options.speculative.is_none());

    let missing = companion_config(
        r#"
[local_model.multimodal_projector]
source = "/definitely/not/a/real/mmproj.gguf"
"#,
    );
    let model = &missing.local_models()[0];
    let mut options = launch_options(model, 1).expect("launch options");
    let error = provision_companions(&store, model, &mut options, None)
        .expect_err("a missing local source must fail provisioning");
    assert!(matches!(error, LocalError::InvalidSource { .. }));
    assert!(options.multimodal_projector.is_none());
}

#[test]
fn model_without_companions_keeps_launch_options_unset() {
    // Provisioning is a no-op for a companion-less model: the options stay
    // exactly what `launch_options` produced, so the emitted command line
    // is unchanged from before companions existed.
    let config = companion_config("");
    let model = &config.local_models()[0];
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let mut options = launch_options(model, 1).expect("launch options");
    let before = options.clone();
    provision_companions(&store, model, &mut options, None).expect("no companions");
    assert_eq!(options, before);
    assert!(options.speculative.is_none());
    assert!(options.multimodal_projector.is_none());
}
