//! Private cache root tests: whoami SID parsing, the icacls grant, and owner-only roots.

use super::*;

#[test]
fn whoami_user_parser_accepts_an_ordinary_account_sid() {
    let sid = super::super::confine::parse_whoami_user_sid(
        std::path::Path::new("cache"),
        true,
        br#""DESKTOP-EXAMPLE\alice","S-1-5-21-111111111-222222222-333333333-1001"
"#,
        b"",
    )
    .expect("ordinary account parses");

    assert_eq!(sid, "S-1-5-21-111111111-222222222-333333333-1001");
}

#[test]
fn whoami_user_parser_accepts_a_well_known_service_sid() {
    let sid = super::super::confine::parse_whoami_user_sid(
        std::path::Path::new("cache"),
        true,
        b"\"NT AUTHORITY\\NETWORK SERVICE\",\"S-1-5-20\"\r\n",
        b"",
    )
    .expect("service account parses");

    assert_eq!(sid, "S-1-5-20");
}

#[test]
fn whoami_user_parser_rejects_malformed_or_multiple_csv_records() {
    for output in [
        b"DESKTOP-EXAMPLE\\alice,S-1-5-21-1-2-3-1001".as_slice(),
        b"\"alice\",\"S-1-5-21-1-2-3-1001\",\"extra\"".as_slice(),
        b"\"alice\",\"S-1-5-21-1-2-3-1001\"\r\n\"bob\",\"S-1-5-21-1-2-3-1002\"\r\n".as_slice(),
        b"\"\",\"S-1-5-20\"".as_slice(),
        b"\"alice\",\"s-1-5-20\"".as_slice(),
        b"\"alice\",\"S-1-5-020\"".as_slice(),
        b"\"alice\",\"S-1-5\"".as_slice(),
        b"\"alice\",\"S-1-5-4294967296\"".as_slice(),
    ] {
        assert!(
            super::super::confine::parse_whoami_user_sid(
                std::path::Path::new("cache"),
                true,
                output,
                b""
            )
            .is_err(),
            "unexpectedly accepted {output:?}"
        );
    }
}

#[test]
fn whoami_user_parser_rejects_a_missing_sid() {
    assert!(
        super::super::confine::parse_whoami_user_sid(
            std::path::Path::new("cache"),
            true,
            b"\"alice\",\"\"\r\n",
            b""
        )
        .is_err()
    );
    assert!(
        super::super::confine::parse_whoami_user_sid(std::path::Path::new("cache"), true, b"", b"")
            .is_err()
    );
}

#[test]
fn whoami_user_parser_rejects_command_failure() {
    let error = super::super::confine::parse_whoami_user_sid(
        std::path::Path::new("cache"),
        false,
        b"\"alice\",\"S-1-5-21-1-2-3-1001\"\r\n",
        b"ERROR: access denied\r\n",
    )
    .expect_err("failed whoami must not yield a SID");

    assert!(error.to_string().contains("whoami identity query failed"));
    assert!(error.to_string().contains("access denied"));
}

#[test]
fn windows_sid_grant_uses_the_icacls_sid_prefix() {
    assert_eq!(
        super::super::confine::windows_sid_grant("S-1-5-20"),
        "*S-1-5-20:(OI)(CI)F"
    );
}

#[cfg(windows)]
#[test]
fn artifact_store_enforces_private_windows_dacl() {
    // ART-006: opening the store restricts the cache root's DACL so no broad
    // principal (Everyone / Authenticated Users / Users) retains access, even
    // for a cache path configured outside the default profile tree.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    let _store = ArtifactStore::new(&root).expect("store");
    std::fs::write(root.join("owner-write-probe"), b"private")
        .expect("current process retains cache write access");

    let output = std::process::Command::new("icacls")
        .arg(&root)
        .output()
        .expect("icacls query");
    assert!(output.status.success(), "icacls query failed");
    let listing = String::from_utf8_lossy(&output.stdout);
    for principal in ["Everyone:", "Authenticated Users:", "\\Users:"] {
        assert!(
            !listing.contains(principal),
            "broad principal {principal} still present in DACL:\n{listing}"
        );
    }
}

#[cfg(unix)]
#[test]
fn artifact_store_enforces_owner_private_cache_root() {
    // ART-006: opening the store tightens a group/world-accessible cache root to
    // owner-only, enforcing the private-cache precondition the confinement relies
    // on rather than merely documenting it.
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).expect("loosen");
    let _store = ArtifactStore::new(&root).expect("store");
    let mode = std::fs::metadata(&root)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "cache root must be tightened to owner-only");
}

#[cfg(unix)]
#[test]
fn validate_cache_path_rejects_symlink_component() {
    // ART-007: a symlink planted as an interior component is refused so a write
    // cannot be redirected outside the cache root.
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir root");
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).expect("mkdir outside");
    symlink(&outside, root.join("link")).expect("symlink");
    let escaped = root.join("link").join("f.bin");
    assert!(matches!(
        validate_cache_path(&root, &escaped),
        Err(LocalError::UnsafeCachePath { .. })
    ));
}
