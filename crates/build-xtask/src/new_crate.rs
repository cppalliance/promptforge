//! `cargo xtask new-crate` scaffolder for `workshop-*` crates.

use std::fs;
use std::path::{Path, PathBuf};

/// Scaffold `crates/<name>/` with a manifest, a facade `lib.rs` holding
/// the invariant docs, and the crate's integration-test binary.
///
/// # Errors
///
/// Returns an error when `name` is not a kebab-case `workshop-*` name, when
/// the crate directory already exists, or when any file cannot be written.
pub(crate) fn scaffold(root: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let short = validate_name(name)?;
    let dir = root.join("crates").join("workshop").join(short);
    anyhow::ensure!(
        !dir.exists(),
        "crate directory already exists: {}",
        dir.display()
    );
    fs::create_dir_all(dir.join("src"))?;
    fs::create_dir_all(dir.join("tests").join("it"))?;
    fs::write(dir.join("Cargo.toml"), manifest(name))?;
    fs::write(dir.join("src").join("lib.rs"), lib_rs(name))?;
    fs::write(
        dir.join("tests").join("it").join("main.rs"),
        test_main_rs(name),
    )?;
    Ok(dir)
}

/// Kebab-case `workshop-<name>`: the server decomposition's crate namespace.
/// Returns the validated `<name>` suffix.
fn validate_name(name: &str) -> anyhow::Result<&str> {
    let suffix = name.strip_prefix("workshop-").unwrap_or("");
    anyhow::ensure!(
        !suffix.is_empty()
            && !suffix.starts_with('-')
            && !suffix.ends_with('-')
            && !suffix.contains("--")
            && suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "invalid crate name `{name}`: expected kebab-case `workshop-<name>`"
    );
    Ok(suffix)
}

fn manifest(name: &str) -> String {
    format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.0.0\"\n\
         publish = false\n\
         edition.workspace = true\n\
         license.workspace = true\n\
         repository.workspace = true\n\
         \n\
         [dependencies]\n\
         \n\
         [lints]\n\
         workspace = true\n"
    )
}

fn lib_rs(name: &str) -> String {
    format!(
        "//! {name} - TODO: one-line purpose.\n\
         //!\n\
         //! ## Invariants\n\
         //!\n\
         //! - Tier: TODO (vocabulary | services | features | shell); may depend\n\
         //!   on: TODO. Read `AGENTS.md` before adding an import.\n\
         //! - Every file in this crate stays under 500 lines; split first, then\n\
         //!   edit.\n"
    )
}

fn test_main_rs(name: &str) -> String {
    format!("//! Integration tests for `{name}`.\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scaffold_scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("crates")).expect("crates dir");
        let dir = scaffold(&root, "workshop-scratch").expect("scaffold");
        (temp, root, dir)
    }

    #[test]
    fn scaffold_creates_manifest_lib_and_test_binary() {
        let (_temp, _root, dir) = scaffold_scratch();
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).expect("manifest");
        assert!(manifest.contains("name = \"workshop-scratch\""));
        assert!(manifest.contains("version = \"0.0.0\""));
        assert!(manifest.contains("publish = false"));
        assert!(manifest.contains("[lints]\nworkspace = true"));
        let lib = fs::read_to_string(dir.join("src").join("lib.rs")).expect("lib.rs");
        assert!(lib.contains("//! ## Invariants"));
        assert!(dir.join("tests").join("it").join("main.rs").is_file());
    }

    #[test]
    fn scaffolded_manifest_parses_and_inherits_workspace_metadata() {
        let (_temp, _root, dir) = scaffold_scratch();
        let text = fs::read_to_string(dir.join("Cargo.toml")).expect("manifest");
        let manifest: toml::Value = toml::from_str(&text).expect("valid toml");
        let workspace_bool = |key: &str| {
            manifest
                .get("package")
                .and_then(|p| p.get(key))
                .and_then(|v| v.get("workspace"))
                .and_then(toml::Value::as_bool)
        };
        assert_eq!(workspace_bool("edition"), Some(true));
        assert_eq!(workspace_bool("license"), Some(true));
        assert_eq!(workspace_bool("repository"), Some(true));
        let lints = manifest
            .get("lints")
            .and_then(|l| l.get("workspace"))
            .and_then(toml::Value::as_bool);
        assert_eq!(lints, Some(true));
    }

    #[test]
    fn scaffold_refuses_to_overwrite_an_existing_crate() {
        let (_temp, root, _dir) = scaffold_scratch();
        assert!(scaffold(&root, "workshop-scratch").is_err());
    }

    #[test]
    fn scaffold_rejects_names_outside_the_workshop_namespace() {
        let temp = tempfile::tempdir().expect("tempdir");
        for name in [
            "scratch",
            "Workshop-Scratch",
            "workshop-",
            "workshop-Bad",
            "workshop--double",
        ] {
            assert!(scaffold(temp.path(), name).is_err(), "accepted {name}");
        }
    }
}
