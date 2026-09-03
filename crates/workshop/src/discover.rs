//! Gateway boot-config discovery for the desktop shell.
//!
//! Search order, first found wins: beside the executable, then the current
//! directory, then `%USERPROFILE%\.promptforge\` (the user profile's
//! `.promptforge` directory). When no file is found, returns `None` so the
//! caller can generate a default boot config, plus the `default` profile
//! the gateway requires, in the profile directory (see [`generate_default`]).

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// Canonical file name searched for at each candidate location.
const CONFIG_FILE_NAME: &str = "gateway.toml";

/// The profile the shell boots the gateway into; generated on first run.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// The boot configuration written on first run, with a freshly generated
/// bearer key baked in.
///
/// The gateway binds loopback-only, hosts the workshop on a second listener,
/// and provisions the recommended STT pair through the artifact store.
fn default_boot_config(api_key: &str) -> String {
    format!(
        r#"config-version = 2

# PromptForge gateway configuration
# Generated on first run. Edit as needed.
# See: crates/gateway/README.md

[server]
bind = "127.0.0.1:8081"
api_key = "{api_key}"

# The workshop UI, hosted by the gateway on a second loopback listener.
[workshop]
bind = "127.0.0.1:7910"

[workshop.stt]
window_seconds = 15
interval_ms = 500
# Domain terms whisper is biased toward, passed as a glossary prompt:
# vocabulary = ["MCP", "GGUF", "Lua"]

[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0

[[stt_model]]
name = "whisper-small-en"
role = "final"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
sha256 = "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"
vram_gb = 2.0

[[profile]]
name = "default"
models = ["whisper-base-en", "whisper-small-en"]
"#
    )
}

/// Locates `gateway.toml`, searching beside the executable first, then the
/// current directory, then the user profile's `.promptforge` directory.
///
/// Returns `None` when no location holds a config file, allowing the caller
/// to generate a default configuration instead.
///
/// # Errors
/// Returns an error when the executable or current directory cannot be
/// determined.
pub(crate) fn discover_config() -> anyhow::Result<Option<PathBuf>> {
    let exe_dir = std::env::current_exe()
        .context("locate the executable")
        .and_then(|exe| {
            exe.parent()
                .map(Path::to_path_buf)
                .context("the executable has no parent directory")
        })?;
    let cwd = std::env::current_dir().context("locate the current directory")?;
    let home = std::env::home_dir().context("locate the user profile directory")?;
    let candidates = candidates_from(&exe_dir, &cwd, &home);
    Ok(first_existing(&candidates))
}

/// The profile candidate: `<home>/.promptforge/gateway.toml`. This is
/// the one place that knows where the profile configuration lives, so
/// first-run generation writes where discovery reads.
pub(crate) fn profile_config_path(home: &Path) -> PathBuf {
    home.join(".promptforge").join(CONFIG_FILE_NAME)
}

/// Builds the candidate list in search order from the three base
/// directories.
fn candidates_from(exe_dir: &Path, cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        exe_dir.join(CONFIG_FILE_NAME),
        cwd.join(CONFIG_FILE_NAME),
        home.join(".promptforge").join(CONFIG_FILE_NAME),
    ]
}

/// Returns the first candidate path that exists, if any.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// Writes the default single-file configuration to `path` with a fresh
/// random api_key and the `default` profile checklist.
/// Returns the boot config path written.
///
/// # Errors
/// Returns an error when a directory or file cannot be created.
pub(crate) fn generate_default(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = path
        .parent()
        .context("the boot config path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::write(path, default_boot_config(&generate_api_key()))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// A fresh random bearer key for the generated `[server]` section, using
/// the OS-seeded cryptographic RNG (`rand::rng`, a ChaCha-based CSPRNG)
/// rather than a fast non-cryptographic generator, since the key guards
/// the gateway's listener.
fn generate_api_key() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hex characters [`generate_api_key`] produces (two u64s, 128 bits).
    const API_KEY_LENGTH: usize = 32;

    #[test]
    fn candidates_are_ordered_exe_then_cwd_then_profile() {
        let candidates = candidates_from(
            Path::new("exe-dir"),
            Path::new("cwd-dir"),
            Path::new("home-dir"),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("exe-dir/gateway.toml"),
                PathBuf::from("cwd-dir/gateway.toml"),
                PathBuf::from("home-dir/.promptforge/gateway.toml"),
            ]
        );
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let exe_dir = tempfile::TempDir::new().expect("tempdir");
        let cwd_dir = tempfile::TempDir::new().expect("tempdir");
        let home_dir = tempfile::TempDir::new().expect("tempdir");
        let promptforge = home_dir.path().join(".promptforge");
        std::fs::create_dir(&promptforge).expect("create profile dir");
        let in_cwd = cwd_dir.path().join(CONFIG_FILE_NAME);
        let in_home = promptforge.join(CONFIG_FILE_NAME);
        std::fs::write(&in_cwd, "").expect("write fixture");
        std::fs::write(&in_home, "").expect("write fixture");

        let candidates = candidates_from(exe_dir.path(), cwd_dir.path(), home_dir.path());
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_cwd.as_path()),
            "the current directory beats the profile"
        );

        let in_exe = exe_dir.path().join(CONFIG_FILE_NAME);
        std::fs::write(&in_exe, "").expect("write fixture");
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_exe.as_path()),
            "beside the executable beats everything"
        );
    }

    #[test]
    fn no_config_returns_none() {
        let exe_dir = tempfile::TempDir::new().expect("tempdir");
        let cwd_dir = tempfile::TempDir::new().expect("tempdir");
        let home_dir = tempfile::TempDir::new().expect("tempdir");
        let candidates = candidates_from(exe_dir.path(), cwd_dir.path(), home_dir.path());
        assert_eq!(first_existing(&candidates), None);
    }

    #[test]
    fn generate_default_writes_one_file_the_gateway_can_boot() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join(CONFIG_FILE_NAME);
        let written = generate_default(&path).expect("first run generates");
        assert_eq!(written, path);

        let profile = gateway::ProfileName::parse(DEFAULT_PROFILE).expect("valid profile name");
        let catalog = gateway::Config::from_toml_str(
            &std::fs::read_to_string(&path).expect("generated config reads"),
        )
        .expect("generated catalog parses");
        let config = catalog
            .select_profile(&profile)
            .expect("generated default profile selects");
        assert_eq!(
            config.server().bind().to_string(),
            "127.0.0.1:8081",
            "the generated gateway bind is loopback on 8081"
        );
        assert_eq!(
            config.server().api_key().expose().len(),
            API_KEY_LENGTH,
            "the generated api_key survives the profile resolution"
        );
    }

    #[test]
    fn the_generated_config_hosts_the_workshop_with_recommended_stt() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = generate_default(&dir.path().join(CONFIG_FILE_NAME)).expect("generates");
        let raw = std::fs::read_to_string(&path).expect("read back");
        let config = gateway::Config::from_toml_str(&raw).expect("the boot config is valid");

        let workshop = config
            .workshop()
            .expect("the generated config carries a [workshop] section");
        assert!(
            workshop.bind().ip().is_loopback(),
            "the workshop listener is loopback-only"
        );
        let stt = workshop.stt().expect("STT tuning section present");
        assert_eq!(stt.window_seconds(), 15);
        assert_eq!(stt.interval_ms(), 500);
        assert_eq!(config.catalog_stt_models().len(), 2);
        assert!(raw.contains("sha256 = "));
        assert!(raw.contains("models = [\"whisper-base-en\", \"whisper-small-en\"]"));
    }

    #[test]
    fn generated_api_keys_are_random_hex() {
        let first = generate_api_key();
        let second = generate_api_key();
        assert_eq!(first.len(), API_KEY_LENGTH);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "two first runs must not share a bearer key");
    }
}
