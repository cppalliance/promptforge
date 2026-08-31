//! Launch-template resolution regression tests.

use super::*;
use promptforge_gateway_config::Config;

fn push_gguf_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn write_test_gguf(path: &Path, chat_template: Option<&str>) {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    let metadata_count = u64::from(chat_template.is_some());
    bytes.extend_from_slice(&metadata_count.to_le_bytes());
    if let Some(template) = chat_template {
        push_gguf_string(&mut bytes, "tokenizer.chat_template");
        bytes.extend_from_slice(&8_u32.to_le_bytes());
        push_gguf_string(&mut bytes, template);
    }
    std::fs::write(path, bytes).expect("write test GGUF");
}

fn chat_model_config(name: &str, chat_template_file: Option<&str>) -> Config {
    let configured = chat_template_file.map_or_else(String::new, |value| {
        format!("chat_template_file = \"{value}\"\n")
    });
    Config::from_toml_str(&format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "{name}"
description = "a local chat model"
source = "/unused/model.gguf"
context = 4096
{configured}"#,
    ))
    .expect("chat model config")
}

#[test]
fn chat_template_precedence_prefers_custom_then_builtin_then_known_override() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let model_path = temp.path().join("model.gguf");
    let broken_edge = include_str!("../chat_templates/fixtures/broken-gemma-4-edge.jinja");
    write_test_gguf(&model_path, Some(broken_edge));
    sidecar::write_sidecar(
        &model_path,
        &sidecar::SidecarMeta {
            source: Some(
                "https://huggingface.co/unsloth/gemma-4-31B-it-GGUF/resolve/main/model.gguf"
                    .to_owned(),
            ),
            fetched: None,
            chat_template: Some("sidecar evidence".to_owned()),
            card: None,
        },
    )
    .expect("write sidecar");

    let custom = chat_model_config("priority", Some("custom/template.jinja"));
    let custom_path =
        resolve_chat_template_file(&store, &custom.local_models()[0], Path::new("missing.gguf"))
            .expect("custom path short-circuits inspection");
    assert_eq!(
        custom_path.as_deref(),
        Some(Path::new("custom/template.jinja"))
    );

    let builtin = chat_model_config("priority", Some("builtin:qwen-3"));
    let builtin_path = resolve_chat_template_file(
        &store,
        &builtin.local_models()[0],
        Path::new("missing.gguf"),
    )
    .expect("builtin short-circuits inspection")
    .expect("builtin uses a staged file");
    assert!(builtin_path.ends_with("chat-templates/qwen-3.jinja"));

    let automatic = chat_model_config("priority", None);
    let override_path =
        resolve_chat_template_file(&store, &automatic.local_models()[0], &model_path)
            .expect("known override")
            .expect("known override uses a staged file");
    assert!(
        override_path.ends_with("chat-templates/gemma-4-edge.jinja"),
        "the embedded hash must beat the conflicting standard-model sidecar ID"
    );
}

#[test]
fn clean_embedded_chat_template_needs_no_file_override() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let model_path = temp.path().join("clean.gguf");
    write_test_gguf(&model_path, Some("{{ messages }} clean"));
    let config = chat_model_config("clean-model", None);

    let resolved = resolve_chat_template_file(&store, &config.local_models()[0], &model_path)
        .expect("clean embedded template resolves");

    assert!(resolved.is_none());
    assert!(!temp.path().join("cache/chat-templates").exists());
}

#[test]
fn inspection_reports_auto_and_known_broken_reasons_without_staging() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let clean_path = temp.path().join("clean.gguf");
    write_test_gguf(&clean_path, Some("{{ messages }} clean"));
    let clean = chat_model_config("clean-model", None);
    let automatic = inspect_chat_template(&clean.local_models()[0], Some(&clean_path))
        .expect("clean template inspects");
    assert_eq!(automatic.source(), ChatTemplateSource::Embedded);
    assert_eq!(automatic.reason(), "Auto uses the GGUF embedded template.");

    let broken_path = temp.path().join("broken.gguf");
    write_test_gguf(
        &broken_path,
        Some(include_str!(
            "../chat_templates/fixtures/broken-gemma-4-edge.jinja"
        )),
    );
    let known = inspect_chat_template(&clean.local_models()[0], Some(&broken_path))
        .expect("broken template inspects");
    assert_eq!(known.source(), ChatTemplateSource::KnownOverride);
    assert_eq!(known.family(), Some(Family::Gemma4));
    assert!(known.reason().contains("Known-broken"));
    assert!(!temp.path().join("chat-templates").exists());
}

#[test]
fn blank_configured_path_does_not_hide_a_valid_embedded_template() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let model_path = temp.path().join("clean.gguf");
    write_test_gguf(&model_path, Some("{{ messages }} clean"));
    let config = chat_model_config("clean-model", Some("   "));

    let resolved = resolve_chat_template_file(&store, &config.local_models()[0], &model_path)
        .expect("blank custom path falls through");

    assert!(resolved.is_none());
}

#[test]
fn both_current_template_hashes_stage_their_exact_overrides() {
    let cases = [
        (
            include_str!("../chat_templates/fixtures/broken-gemma-4-edge.jinja"),
            "gemma-4-edge.jinja",
            crate::chat_templates::KNOWN_OVERRIDES[0].template,
        ),
        (
            include_str!("../chat_templates/fixtures/broken-gemma-4-standard.jinja"),
            "gemma-4-standard.jinja",
            crate::chat_templates::KNOWN_OVERRIDES[1].template,
        ),
    ];
    for (embedded, asset_name, expected) in cases {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
        let model_path = temp.path().join("broken.gguf");
        write_test_gguf(&model_path, Some(embedded));
        let config = chat_model_config("broken-model", None);

        let resolved = resolve_chat_template_file(&store, &config.local_models()[0], &model_path)
            .expect("known hash resolves")
            .expect("known hash stages a file");

        assert!(resolved.ends_with(Path::new("chat-templates").join(asset_name)));
        assert_eq!(
            std::fs::read_to_string(resolved).expect("read staged override"),
            expected
        );
    }
}

#[test]
fn builtin_family_alias_stages_the_canonical_asset() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let config = chat_model_config("builtin-model", Some("builtin:QWEN3"));

    let resolved =
        resolve_chat_template_file(&store, &config.local_models()[0], Path::new("missing.gguf"))
            .expect("builtin resolves")
            .expect("builtin stages a file");

    assert_eq!(
        resolved,
        temp.path().join("cache/chat-templates/qwen-3.jinja")
    );
    assert_eq!(
        std::fs::read_to_string(resolved).expect("read staged family"),
        Family::Qwen3.template()
    );
}

#[test]
fn unknown_builtin_family_error_lists_every_valid_family() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let config = chat_model_config("unknown-family-model", Some("builtin:not-real"));

    let error =
        resolve_chat_template_file(&store, &config.local_models()[0], Path::new("missing.gguf"))
            .expect_err("unknown family must fail");
    assert!(matches!(
        error,
        LocalError::UnknownChatTemplateFamily { .. }
    ));
    let message = error.to_string();
    assert!(message.contains("unknown-family-model"));
    assert!(message.contains("custom Jinja path"));
    for family in Family::ALL {
        assert!(
            message.contains(family.canonical_name()),
            "missing family {} from {message}",
            family.canonical_name()
        );
    }
}

#[test]
fn chat_model_without_any_template_refuses_to_launch() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let model_path = temp.path().join("missing-template.gguf");
    write_test_gguf(&model_path, None);
    let config = chat_model_config("template-less-model", None);

    let error = resolve_chat_template_file(&store, &config.local_models()[0], &model_path)
        .expect_err("missing template must fail");

    assert!(matches!(error, LocalError::MissingChatTemplate { .. }));
    let message = error.to_string();
    assert!(message.contains("template-less-model"));
    assert!(message.contains("custom Jinja path"));
    assert!(message.contains("builtin:<family>"));
    assert!(message.contains("tokenizer.chat_template"));
}

#[test]
fn sidecar_model_id_is_the_only_secondary_override_evidence() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path().join("cache")).expect("store");
    let model_path = temp.path().join("model.gguf");
    write_test_gguf(&model_path, Some("{{ messages }} clean"));
    sidecar::write_sidecar(
        &model_path,
        &sidecar::SidecarMeta {
            source: Some(
                "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/model.gguf"
                    .to_owned(),
            ),
            fetched: None,
            chat_template: None,
            card: None,
        },
    )
    .expect("write provenance sidecar");
    let config = chat_model_config("ordinary-local-name", None);

    let resolved = resolve_chat_template_file(&store, &config.local_models()[0], &model_path)
        .expect("sidecar model ID resolves")
        .expect("sidecar model ID stages an override");
    assert!(resolved.ends_with("chat-templates/gemma-4-edge.jinja"));

    std::fs::remove_file(sidecar::sidecar_path(&model_path)).expect("remove sidecar");
    let weak_name = chat_model_config("gemma-4-E2B-it-GGUF", None);
    let resolved = resolve_chat_template_file(&store, &weak_name.local_models()[0], &model_path)
        .expect("clean embedded template remains usable");
    assert!(
        resolved.is_none(),
        "the configured display name must not select a family or override"
    );
}
