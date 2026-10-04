//! Tests for local-model companion parsing and validation.

use super::*;
use crate::config::Config;

const HEADER: &str = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"
"#;

const DIGEST: &str = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32";

fn entry(body: &str) -> String {
    format!(
        "{HEADER}\n[[local_model]]\nname = \"q\"\ndescription = \"a local model\"\nsource = \"/models/q.gguf\"\ncontext = 4096\n{body}"
    )
}

fn parse(body: &str) -> Result<Config, crate::api_error::ConfigError> {
    Config::from_toml_str(&entry(body))
}

#[test]
fn parses_remote_companions_with_pins() {
    let config = parse(&format!(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
    ))
    .unwrap();
    let model = &config.local_models()[0];
    let speculative = model.speculative().unwrap();
    assert_eq!(speculative.kind(), SpeculationType::DraftMtp);
    assert_eq!(speculative.source(), "https://example.com/q-mtp.gguf");
    assert_eq!(speculative.sha256(), Some(DIGEST));
    assert_eq!(speculative.draft_max().get(), 2);
    let projector = model.multimodal_projector().unwrap();
    assert_eq!(projector.source(), "https://example.com/q-mmproj.gguf");
    assert_eq!(projector.sha256(), Some(DIGEST));
}

#[test]
fn projector_implies_images_capability() {
    let config = parse(
        r#"
[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"
"#,
    )
    .unwrap();
    assert!(config.local_models()[0].capabilities().images());
}

#[test]
fn selected_profile_keeps_projector_images_capability() {
    let config = parse(
        r#"
[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"

[[profile]]
name = "work"
models = ["q"]
"#,
    )
    .unwrap();
    let selected = config
        .select_profile(Some(&crate::ProfileName::parse("work").unwrap()))
        .unwrap();
    assert!(selected.local_models()[0].capabilities().images());
}

#[test]
fn no_projector_keeps_images_default() {
    let config = parse("").unwrap();
    assert!(!config.local_models()[0].capabilities().images());
}

#[test]
fn rejects_unknown_speculation_type() {
    let result = parse(&format!(
        r#"
[local_model.speculative]
type = "draft-eagle3"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
    ));
    assert!(result.is_err());
}

#[test]
fn rejects_speculative_on_non_chat_kind() {
    let result = parse(&format!(
        r#"kind = "embedding"

[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
    ));
    assert!(result.is_err());
}

#[test]
fn rejects_projector_on_non_chat_kind() {
    let result = parse(&format!(
        r#"kind = "classifier"

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
    ));
    assert!(result.is_err());
}

#[test]
fn rejects_remote_speculative_without_pin() {
    let result = parse(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
draft_max = 2
"#,
    );
    assert!(result.is_err());
}

#[test]
fn rejects_remote_projector_without_pin() {
    let result = parse(
        r#"
[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
"#,
    );
    assert!(result.is_err());
}

#[test]
fn rejects_http_companion_sources() {
    let speculative = parse(&format!(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "http://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
    ));
    assert!(speculative.is_err());
    let projector = parse(&format!(
        r#"
[local_model.multimodal_projector]
source = "http://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
    ));
    assert!(projector.is_err());
}

#[test]
fn rejects_empty_companion_sources() {
    let speculative = parse(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = ""
draft_max = 2
"#,
    );
    assert!(speculative.is_err());
    let projector = parse(
        r#"
[local_model.multimodal_projector]
source = ""
"#,
    );
    assert!(projector.is_err());
}

#[test]
fn rejects_malformed_companion_pin() {
    let result = parse(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
sha256 = "not-hex"
draft_max = 2
"#,
    );
    assert!(result.is_err());
}

#[test]
fn rejects_out_of_range_draft_max() {
    for draft_max in [0, 17] {
        let result = parse(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
draft_max = {draft_max}
"#
        ));
        assert!(result.is_err(), "draft_max {draft_max} must be rejected");
    }
}

#[test]
fn accepts_local_path_companions_without_pins() {
    let config = parse(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
draft_max = 1

[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"
"#,
    )
    .unwrap();
    let model = &config.local_models()[0];
    assert_eq!(model.speculative().unwrap().sha256(), None);
    assert_eq!(model.speculative().unwrap().draft_max().get(), 1);
    assert!(model.multimodal_projector().is_some());
}

#[test]
fn defaults_to_no_companions() {
    let config = parse("").unwrap();
    let model = &config.local_models()[0];
    assert!(model.speculative().is_none());
    assert!(model.multimodal_projector().is_none());
}

#[test]
fn whole_entry_replacement_round_trips() {
    // The rollout replacement entry: companions included end to end.
    let replaced = parse(&format!(
        r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
    ))
    .unwrap();
    assert!(replaced.local_models()[0].speculative().is_some());
    // The pre-replacement entry, written before companions existed, still
    // parses with both companions absent.
    let legacy = parse("").unwrap();
    let model = &legacy.local_models()[0];
    assert!(model.speculative().is_none());
    assert!(model.multimodal_projector().is_none());
}

#[test]
fn draft_token_max_bounds() {
    assert_eq!(DraftTokenMax::new(1).unwrap().get(), 1);
    assert_eq!(DraftTokenMax::new(DraftTokenMax::MAX).unwrap().get(), 16);
    assert_eq!(DraftTokenMax::new(0).unwrap_err().value(), 0);
    assert_eq!(DraftTokenMax::new(17).unwrap_err().value(), 17);
}

#[test]
fn draft_token_max_serializes_as_a_plain_integer() {
    let max = DraftTokenMax::new(7).unwrap();
    let json = serde_json::to_value(max).unwrap();
    assert_eq!(json, 7);
    let back: DraftTokenMax = serde_json::from_value(json).unwrap();
    assert_eq!(back, max);
}
