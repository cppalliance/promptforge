//! Tests for profile checklists, membership, STT role pairing, and per-profile budgets.

use super::*;

/// One remote, one local, and one STT model, plus a profile over `models`.
fn mixed_catalog_with_profile(models: &str) -> String {
    format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "remote"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "local"
description = "prose"
source = "/models/local.gguf"
context = 4096

[[stt_model]]
name = "interim"
role = "interim"
source = "/models/base.en.bin"
vram_gb = 1.0

[[profile]]
name = "work"
models = {models}
"#
    )
}

#[test]
fn selected_profile_narrows_local_and_stt_and_serves_every_remote_model() {
    let toml = mixed_catalog_with_profile("[\"interim\"]");
    let catalog = Config::from_toml_str(&toml).expect("catalog parses");
    let selected = catalog
        .select_profile(Some(
            &crate::ProfileName::parse("work").expect("profile name"),
        ))
        .expect("profile selects");
    assert_eq!(selected.models().len(), catalog.models().len());
    assert_eq!(selected.models()[0].name(), "remote");
    assert!(selected.local_models().is_empty());
    assert_eq!(selected.stt_models()[0].name(), "interim");
    assert_eq!(selected.catalog_local_models()[0].name(), "local");
    assert_eq!(selected.catalog_stt_models()[0].name(), "interim");
}

#[test]
fn selecting_no_profile_serves_remote_models_and_nothing_local() {
    let toml = mixed_catalog_with_profile("[\"local\", \"interim\"]");
    let catalog = Config::from_toml_str(&toml).expect("catalog parses");
    let selected = catalog.select_profile(None).expect("no profile selects");
    assert!(selected.active_profile().is_none());
    assert_eq!(selected.models()[0].name(), "remote");
    assert!(selected.local_models().is_empty());
    assert!(selected.stt_models().is_empty());
    assert_eq!(selected.catalog_local_models()[0].name(), "local");
    assert_eq!(selected.catalog_stt_models()[0].name(), "interim");
}

#[test]
fn profile_listing_a_remote_model_names_the_profile_and_model() {
    let toml = mixed_catalog_with_profile("[\"remote\", \"local\"]");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("profile \"work\""),
                "profile named: {message}"
            );
            assert!(
                message.contains("remote model \"remote\""),
                "remote model named: {message}"
            );
            assert!(
                message.contains("remote models are always served"),
                "boundary explained: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn every_profile_reference_must_exist() {
    let toml = format!("{SAMPLE}\n[[profile]]\nname = \"unused\"\nmodels = [\"ghost\"]\n");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(message.contains("unused"), "profile named: {message}");
            assert!(message.contains("ghost"), "missing model named: {message}");
            assert!(
                message.contains("undefined catalog model"),
                "unknown names keep the undefined-model error: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn unselected_catalog_entries_still_validate_references() {
    let toml = format!(
        "{SAMPLE}\n\
         [[model]]\nname = \"dangling\"\ndescription = \"prose\"\ncontext = 1\n\
         upstream = \"u\"\nendpoints = [\"ghost\"]\n\
         [[profile]]\nname = \"work\"\nmodels = []\n"
    );
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Validation(message)) if message.contains("ghost")
    ));
}

/// A catalog whose two local models over-book `gpu0`; `models` is one
/// profile's checklist.
fn overbooked_catalog_with_profile(models: &str) -> String {
    format!(
        r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[profile]]
name = "selected"
models = {models}
"#
    )
}

#[test]
fn each_profile_vram_check_uses_its_own_subset() {
    let toml = overbooked_catalog_with_profile("[\"a\"]");
    let config = Config::from_toml_str(&toml).expect("single model fits");
    assert_eq!(config.profiles()[0].models(), ["a"]);
}

#[test]
fn any_overbooked_profile_rejects_the_whole_catalog() {
    let toml = overbooked_catalog_with_profile("[\"a\", \"b\"]");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

fn stt_profile_config(entries: &str, models: &str) -> String {
    format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

{entries}

[[profile]]
name = "work"
models = {models}
"#
    )
}

#[test]
fn profile_names_and_membership_are_unique() {
    let duplicate_names = format!(
        "{SAMPLE}\n\
         [[profile]]\nname = \"work\"\nmodels = []\n\
         [[profile]]\nname = \"work\"\nmodels = []\n"
    );
    assert!(matches!(
        Config::parse_toml(&duplicate_names),
        Err(ConfigError::Validation(message)) if message.contains("duplicate profile")
    ));

    let illegal_name = format!("{SAMPLE}\n[[profile]]\nname = \"../work\"\nmodels = []\n");
    assert!(matches!(
        Config::parse_toml(&illegal_name),
        Err(ConfigError::Validation(message)) if message.contains("../work")
    ));

    let duplicate_member = format!(
        "{SAMPLE}\n\
         [[local_model]]\nname = \"q\"\ndescription = \"prose\"\n\
         source = \"/models/q.gguf\"\ncontext = 4096\n\
         [[profile]]\nname = \"work\"\nmodels = [\"q\", \"q\"]\n"
    );
    assert!(matches!(
        Config::parse_toml(&duplicate_member),
        Err(ConfigError::Validation(message)) if message.contains("duplicate model q")
    ));
}

#[test]
fn stt_role_pairing_rejects_duplicate_slots() {
    for (role, models) in [
        ("interim", "[\"a\", \"b\"]"),
        ("final", "[\"interim\", \"a\", \"b\"]"),
    ] {
        let prefix = if role == "final" {
            "[[stt_model]]\nname = \"interim\"\nrole = \"interim\"\n\
             source = \"/models/interim.bin\"\nvram_gb = 1.0\n"
        } else {
            ""
        };
        let entries = format!(
            "{prefix}\
             [[stt_model]]\nname = \"a\"\nrole = \"{role}\"\n\
             source = \"/models/a.bin\"\nvram_gb = 1.0\n\
             [[stt_model]]\nname = \"b\"\nrole = \"{role}\"\n\
             source = \"/models/b.bin\"\nvram_gb = 1.0\n"
        );
        assert!(
            matches!(
                Config::parse_toml(&stt_profile_config(&entries, models)),
                Err(ConfigError::Validation(message))
                    if message.contains("more than one") && message.contains("work")
            ),
            "duplicate {role} slot must fail"
        );
    }
}

#[test]
fn final_without_interim_names_the_fix() {
    let entries = r#"
[[stt_model]]
name = "final"
role = "final"
source = "/models/final.bin"
vram_gb = 2.0
"#;
    match Config::parse_toml(&stt_profile_config(entries, "[\"final\"]")) {
        Err(ConfigError::Validation(message)) => {
            assert!(message.contains("final"), "final model named: {message}");
            assert!(message.contains("add one interim"), "fix named: {message}");
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn interim_without_final_is_supported_degraded_mode() {
    let entries = r#"
[[stt_model]]
name = "interim"
role = "interim"
source = "/models/interim.bin"
vram_gb = 1.0
"#;
    assert!(Config::parse_toml(&stt_profile_config(entries, "[\"interim\"]")).is_ok());
}

#[test]
fn remote_stt_source_may_omit_optional_digest() {
    let entries = r#"
[[stt_model]]
name = "interim"
role = "interim"
source = "https://example.com/models/interim.bin"
vram_gb = 1.0
"#;
    assert!(Config::parse_toml(&stt_profile_config(entries, "[\"interim\"]")).is_ok());
}

#[test]
fn stt_models_count_toward_each_profile_vram_budget() {
    let entries = r#"
[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 2

[[local_model]]
name = "chat"
description = "prose"
source = "/models/chat.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 1

[[stt_model]]
name = "interim"
role = "interim"
source = "/models/interim.bin"
vram_gb = 1.5
dominion = "gpu0"
"#;
    assert!(matches!(
        Config::parse_toml(&stt_profile_config(entries, "[\"chat\", \"interim\"]")),
        Err(ConfigError::Validation(message))
            if message.contains("work") && message.contains("gpu0")
    ));
}

#[test]
fn stt_catalog_validates_source_pin_vram_and_dominion() {
    for (field, entry) in [
        (
            "source",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='http://x/s.bin'\nvram_gb=1.0",
        ),
        (
            "sha256",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nsha256='bad'\nvram_gb=1.0",
        ),
        (
            "vram_gb",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nvram_gb=0.0",
        ),
        (
            "dominion",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nvram_gb=1.0\ndominion='missing'",
        ),
    ] {
        assert!(
            matches!(
                Config::parse_toml(&stt_profile_config(entry, "[\"s\"]")),
                Err(ConfigError::Validation(message)) if message.contains(field)
            ),
            "invalid STT {field} must fail"
        );
    }
}
