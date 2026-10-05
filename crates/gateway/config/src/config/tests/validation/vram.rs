//! Tests for VRAM estimates and dominion co-residency budgets.

use super::*;

#[test]
fn rejects_vram_budget_overflow() {
    let toml = r#"
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
name = "overbooked"
models = ["a", "b"]
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
            assert!(
                message.contains("by 4"),
                "expected the error to name the overflow amount: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn rejects_bound_model_without_vram_estimate() {
    // Budgets must be complete to be meaningful: a model bound to a budgeted
    // dominion without its own estimate is an error.
    let toml = r#"
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

[[profile]]
name = "incomplete"
models = ["a", "b"]
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("local_model b "),
                "expected the error to name the model: {message}"
            );
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn accepts_exact_vram_fit() {
    let toml = r#"
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
vram_gb = 10

[[profile]]
name = "exact"
models = ["a", "b"]
"#;
    assert!(Config::parse_toml(toml).is_ok());
}

#[test]
fn accepts_fractional_local_model_vram_estimate() {
    // The Discover UI writes the quant file size in GiB rounded to two
    // decimals, e.g. 1.22 for a 1.2 GiB download. A u32 schema rejected
    // that for every non-whole-GiB model (workshop finding 30).
    let toml = r#"
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
vram_gb = 1.22

[[profile]]
name = "fractional"
models = ["a"]
"#;
    let config = Config::parse_toml(toml).expect("fractional vram_gb parses");
    assert_eq!(config.local_models[0].vram_gb, Some(1.22));
}

#[test]
fn rejects_non_positive_local_model_vram_estimate() {
    for value in ["0.0", "-1.0", "nan", "inf"] {
        let toml = format!(
            r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
vram_gb = {value}
"#
        );
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => assert!(
                message.contains("vram_gb must be finite and greater than zero"),
                "expected the vram_gb error for {value}: {message}"
            ),
            other => panic!("expected a validation error for {value}, got {other:?}"),
        }
    }
}

#[test]
fn accepts_bound_models_when_dominion_has_no_budget() {
    // A local dominion without vram_gb imposes no co-residency obligation:
    // bound models need no estimate.
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14
"#;
    assert!(Config::parse_toml(toml).is_ok());
}
