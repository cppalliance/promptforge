//! Compile boundary for the feature-gated fixture surface.

#![expect(
    clippy::expect_used,
    reason = "the compile fixture fails with the subprocess invariant named"
)]

#[cfg(feature = "test-fixtures")]
use std::path::Path;

#[cfg(feature = "test-fixtures")]
const CHILD_FALLBACK_ROOT: &str = "PROMPTFORGE_RESOLVER_CHILD_FALLBACK_ROOT";
#[cfg(feature = "test-fixtures")]
const CHILD_FALLBACK_NAME: &str = "PROMPTFORGE_RESOLVER_CHILD_FALLBACK_NAME";
#[cfg(feature = "test-fixtures")]
const CHILD_EXPECTED: &str = "PROMPTFORGE_RESOLVER_CHILD_EXPECTED";
#[cfg(feature = "test-fixtures")]
const RESOLVER_OVERRIDE: &str = "PROMPTFORGE_RESOLVER_TEST_OVERRIDE";

#[cfg(not(feature = "test-fixtures"))]
#[test]
fn default_dependency_does_not_expose_test_fixtures() {
    let temp = tempfile::tempdir().expect("temporary consumer directory");
    let source = temp.path().join("src");
    std::fs::create_dir(&source).expect("consumer source directory creates");
    let engine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .display()
        .to_string()
        .replace('\\', "/");
    std::fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"fixture-boundary-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngateway-stt-engine = {{ path = {engine:?}, default-features = false }}\n"
        ),
    )
    .expect("consumer manifest writes");
    std::fs::write(
        source.join("lib.rs"),
        "pub use gateway_stt_engine::test_fixtures::native::require_fixture;\n",
    )
    .expect("consumer source writes");

    let output = std::process::Command::new(env!("CARGO"))
        .arg("check")
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(temp.path())
        .output()
        .expect("consumer cargo check runs");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "default consumer unexpectedly compiled"
    );
    assert!(
        stderr.contains("could not find `test_fixtures` in `gateway_stt_engine`"),
        "failure must prove fixture symbols are absent: {stderr}"
    );
}

#[cfg(feature = "test-fixtures")]
#[test]
fn public_resolver_uses_the_callers_fallback_root() {
    let temp = tempfile::tempdir().expect("temporary fixture directory");
    let fallback_root = temp.path().join("caller-owned");
    std::fs::create_dir(&fallback_root).expect("caller fixture directory creates");
    let fallback = fallback_root.join("model.bin");
    std::fs::write(&fallback, b"fallback").expect("fallback fixture writes");

    let output = resolver_child(&fallback_root, "model.bin", &fallback, None);

    assert_child_succeeded(&output);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn public_resolver_prefers_the_process_environment_override() {
    let temp = tempfile::tempdir().expect("temporary fixture directory");
    let fallback_root = temp.path().join("caller-owned");
    std::fs::create_dir(&fallback_root).expect("caller fixture directory creates");
    std::fs::write(fallback_root.join("model.bin"), b"fallback").expect("fallback fixture writes");
    let override_path = temp.path().join("override.bin");
    std::fs::write(&override_path, b"override").expect("override fixture writes");

    let output = resolver_child(
        &fallback_root,
        "model.bin",
        &override_path,
        Some(&override_path),
    );

    assert_child_succeeded(&output);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn public_resolver_missing_file_diagnostic_names_the_resolved_path() {
    let temp = tempfile::tempdir().expect("temporary fixture directory");
    let fallback_root = temp.path().join("caller-owned");
    let missing = fallback_root.join("whisper.dll");

    let output = resolver_child(&fallback_root, "whisper.dll", &missing, None);
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !output.status.success(),
        "missing fixture unexpectedly resolved"
    );
    assert!(
        diagnostic.contains("native test fixture is missing"),
        "diagnostic classifies the failure: {diagnostic}"
    );
    assert!(
        diagnostic.contains(&missing.display().to_string()),
        "diagnostic names the resolved path: {diagnostic}"
    );
}

#[cfg(feature = "test-fixtures")]
fn resolver_child(
    fallback_root: &Path,
    fallback_name: &str,
    expected: &Path,
    override_path: Option<&Path>,
) -> std::process::Output {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args([
            "--exact",
            "public_resolver_child",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_FALLBACK_ROOT, fallback_root)
        .env(CHILD_FALLBACK_NAME, fallback_name)
        .env(CHILD_EXPECTED, expected)
        .env_remove(RESOLVER_OVERRIDE);
    if let Some(override_path) = override_path {
        command.env(RESOLVER_OVERRIDE, override_path);
    }
    command.output().expect("resolver child runs")
}

#[cfg(feature = "test-fixtures")]
fn assert_child_succeeded(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "resolver child failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "test-fixtures")]
#[test]
#[ignore = "run only in an isolated environment by the public resolver contract tests"]
fn public_resolver_child() {
    let Some(fallback_root) = std::env::var_os(CHILD_FALLBACK_ROOT) else {
        return;
    };
    let fallback_name =
        std::env::var_os(CHILD_FALLBACK_NAME).expect("child fallback name is supplied");
    let expected = std::env::var_os(CHILD_EXPECTED).expect("child expected path is supplied");

    let resolved = gateway_stt_engine::test_fixtures::native::require_fixture(
        RESOLVER_OVERRIDE,
        Path::new(&fallback_root),
        Path::new(&fallback_name)
            .to_str()
            .expect("fixture name is valid UTF-8"),
    );

    assert_eq!(resolved, std::path::PathBuf::from(expected));
}
