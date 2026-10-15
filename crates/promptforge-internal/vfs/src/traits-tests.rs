use super::{AllowAll, Op, Policy, Verdict, Vfs, VfsAccess};
use crate::error::VfsError;
use crate::test_support::{TestFs, context, path};

/// A session on the test backend, whose access implements only the
/// required methods, so every provided method below runs its default.
#[track_caller]
fn stub(files: &[(&str, &str)]) -> Result<Box<dyn VfsAccess>, VfsError> {
    TestFs::seeded(files).acquire(&context())
}

#[test]
fn the_default_read_range_slices_a_whole_read() -> Result<(), VfsError> {
    let backend = stub(&[("/a.txt", "hello world")])?;
    let bytes = backend.read_range(&path("/a.txt")?, 6, 5)?;
    assert_eq!(bytes, b"world");
    Ok(())
}

#[test]
fn the_default_read_range_clips_at_the_end_of_the_file() -> Result<(), VfsError> {
    let backend = stub(&[("/a.txt", "hello")])?;
    assert_eq!(backend.read_range(&path("/a.txt")?, 2, 100)?, b"llo");
    assert!(backend.read_range(&path("/a.txt")?, 100, 5)?.is_empty());
    Ok(())
}

#[test]
fn the_default_str_replace_rewrites_the_unique_occurrence() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "alpha beta gamma")])?;
    backend.str_replace(&path("/a.txt")?, "beta", "BETA")?;
    assert_eq!(backend.read(&path("/a.txt")?)?, b"alpha BETA gamma");
    Ok(())
}

#[test]
fn the_default_str_replace_rejects_zero_matches() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "alpha beta")])?;
    let result = backend.str_replace(&path("/a.txt")?, "missing", "x");
    assert_eq!(
        result,
        Err(VfsError::Anchor {
            path: "/a.txt".to_owned(),
            anchor: "missing".to_owned(),
            count: 0,
        })
    );
    assert_eq!(backend.read(&path("/a.txt")?)?, b"alpha beta");
    Ok(())
}

#[test]
fn the_default_str_replace_rejects_multiple_matches() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "foo and foo")])?;
    let result = backend.str_replace(&path("/a.txt")?, "foo", "bar");
    assert_eq!(
        result,
        Err(VfsError::Anchor {
            path: "/a.txt".to_owned(),
            anchor: "foo".to_owned(),
            count: 2,
        })
    );
    assert_eq!(backend.read(&path("/a.txt")?)?, b"foo and foo");
    Ok(())
}

#[test]
fn the_default_str_replace_refuses_an_empty_anchor_on_an_empty_file() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "")])?;
    let result = backend.str_replace(&path("/a.txt")?, "", "x");
    match result {
        Err(VfsError::Anchor {
            path,
            anchor,
            count,
        }) => {
            assert_eq!(path, "/a.txt");
            assert!(anchor.is_empty());
            assert_eq!(count, 0);
        }
        other => panic!("expected the empty-anchor refusal, got {other:?}"),
    }
    assert_eq!(backend.read(&path("/a.txt")?)?, b"");
    Ok(())
}

#[test]
fn the_default_str_replace_refuses_an_empty_anchor_on_a_non_empty_file() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "alpha beta")])?;
    let result = backend.str_replace(&path("/a.txt")?, "", "x");
    match result {
        Err(VfsError::Anchor {
            path,
            anchor,
            count,
        }) => {
            assert_eq!(path, "/a.txt");
            assert!(anchor.is_empty());
            assert_eq!(count, 0);
        }
        other => panic!("expected the empty-anchor refusal, got {other:?}"),
    }
    assert_eq!(backend.read(&path("/a.txt")?)?, b"alpha beta");
    Ok(())
}

#[test]
fn the_default_str_replace_reports_non_utf8_text() -> Result<(), VfsError> {
    let mut backend = stub(&[])?;
    backend.write(&path("/bin.dat")?, &[0xff, 0xfe])?;
    assert_eq!(
        backend.str_replace(&path("/bin.dat")?, "x", "y"),
        Err(VfsError::NotUtf8 {
            path: "/bin.dat".to_owned(),
        })
    );
    Ok(())
}

#[test]
fn unsupported_posix_defaults_return_the_right_error_kind() -> Result<(), VfsError> {
    let mut backend = stub(&[("/a.txt", "x")])?;
    assert!(matches!(
        backend.symlink(&path("/a.txt")?, &path("/b.txt")?),
        Err(VfsError::Unsupported { .. })
    ));
    assert!(matches!(
        backend.read_link(&path("/a.txt")?),
        Err(VfsError::Unsupported { .. })
    ));
    assert!(matches!(
        backend.chmod(&path("/a.txt")?, 0o644),
        Err(VfsError::Unsupported { .. })
    ));
    Ok(())
}

#[test]
fn allow_all_permits_every_operation() -> Result<(), VfsError> {
    let policy = AllowAll;
    assert_eq!(policy.check(Op::Write, &path("/a.txt")?), Verdict::Allow);
    Ok(())
}
