//! Jail edge cases: the path-spelling tricks that must never escape the
//! grants, and the CI-aware helper that turns a silent symlink skip into
//! a CI failure.
//!
//! The confinement pipeline in `workspace/confine.rs` rejects `..` and
//! Windows alternate-data-stream colons lexically, canonicalizes the rest
//! (resolving symlinks, junctions, case, and verbatim `\\?\` prefixes), and
//! prefix-matches the canonical path against the canonical grants. These
//! tests pin that behavior for the spellings a request can arrive in.

#[cfg(windows)]
use super::*;

/// Turns a silent skip into a failure under CI, and prints the reason
/// otherwise so the caller can `return`. The `ci` flag is read by the
/// caller through `std::env::var_os("CI").is_some()`, so no test ever calls
/// `std::env::set_var`, which is `unsafe` in Rust 2024 and forbidden here.
pub(super) fn symlink_unavailable(ci: bool, reason: &str) {
    assert!(!ci, "{reason}");
    eprintln!("skipping: {reason}");
}

#[test]
#[should_panic(expected = "symlink creation failed")]
fn the_ci_flag_turns_a_skip_into_a_failure() {
    symlink_unavailable(true, "symlink creation failed");
}

#[test]
fn without_ci_a_skip_prints_and_returns() {
    symlink_unavailable(false, "symlink creation failed");
}

#[cfg(windows)]
fn verbatim(path: &Path) -> PathBuf {
    // `\\?\` is the Win32 verbatim (extended-length) prefix: the same file,
    // spelled a different way. Build it from the simplified DOS form so the
    // test always exercises the prefix; `canonicalize_simplified` strips it.
    PathBuf::from(format!("\\\\?\\{}", simplified(path).display()))
}

#[cfg(windows)]
fn unc(path: &Path) -> PathBuf {
    // `\\localhost\C$\...` is the administrative-share spelling of a local
    // path; it canonicalizes to a UNC form that never matches a local grant.
    let text = simplified(path).to_string_lossy().into_owned();
    let drive = &text[..1];
    let rest = &text[3..];
    PathBuf::from(format!("\\\\localhost\\{drive}$\\{rest}"))
}

#[cfg(windows)]
#[test]
fn a_verbatim_spelling_of_a_granted_path_is_admitted() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    let read = workspace
        .read_file(&verbatim(&file))
        .expect("a verbatim spelling of a granted path reads");
    assert_eq!(read.text, "hello");
}

#[cfg(windows)]
#[test]
fn a_verbatim_spelling_of_an_ungranted_path_is_rejected() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("seed the ungranted file");
    let error = workspace
        .read_file(&verbatim(&dir.path().join("a.txt")))
        .expect_err("a verbatim spelling of an ungranted path is rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_unc_spelling_never_escapes_the_grants() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("seed the local file");
    // The UNC spelling either canonicalizes to a form that no local grant
    // prefix-matches (OutsideGrants) or fails to resolve on a host without
    // the administrative share (NotFound or ResolvePath). It must never
    // admit the path.
    let error = workspace
        .read_file(&unc(&dir.path().join("a.txt")))
        .expect_err("a UNC spelling must never be admitted");
    assert!(
        matches!(
            error,
            WorkspaceError::OutsideGrants
                | WorkspaceError::NotFound
                | WorkspaceError::ResolvePath { .. }
        ),
        "expected a rejection, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_unc_spelling_of_a_granted_path_is_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    // A UNC spelling canonicalizes to a UNC form that never prefix-matches a
    // local grant, so even a granted path is refused (OutsideGrants); on a
    // host without the administrative share the resolution fails instead
    // (NotFound or ResolvePath). It is never admitted.
    let error = workspace
        .read_file(&unc(&file))
        .expect_err("a UNC spelling of a granted path is never admitted");
    assert!(
        matches!(
            error,
            WorkspaceError::OutsideGrants
                | WorkspaceError::NotFound
                | WorkspaceError::ResolvePath { .. }
        ),
        "expected a rejection, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_case_only_respelling_of_a_granted_root_is_admitted() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    let respelled = PathBuf::from(file.to_string_lossy().to_ascii_uppercase());
    let read = workspace
        .read_file(&respelled)
        .expect("a case-only respelling of a granted path reads");
    assert_eq!(read.text, "hello");
}

#[cfg(windows)]
#[test]
fn a_junction_inside_a_grant_pointing_outside_is_rejected() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    fs::write(outside.path().join("secret.txt"), "secret").expect("seed the secret");
    let junction = dir.path().join("junction");
    let outcome = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(outside.path())
        .output();
    let created = matches!(&outcome, Ok(output) if output.status.success());
    if !created {
        symlink_unavailable(std::env::var_os("CI").is_some(), "junction creation failed");
        return;
    }
    let error = workspace
        .read_file(&junction.join("secret.txt"))
        .expect_err("a junction escape must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}
