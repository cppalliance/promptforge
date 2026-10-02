//! Tests for the capability's file operations: reads and writes through
//! the handle, the empty-anchor refusal, line ranges, and UTF-8 reads.

use super::*;

#[test]
fn an_access_reads_and_writes_through_the_handle() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let access = vfs.acquire(test_origin())?;
    access.write("/notes/a.txt", b"hello")?;
    assert_eq!(access.read("/notes/a.txt")?, b"hello");
    assert!(access.exists("/notes/a.txt")?);
    Ok(())
}

#[test]
fn every_acquire_vends_a_process_unique_identity() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let first = vfs.acquire(test_origin())?;
    let second = vfs.acquire(test_origin())?;
    assert_ne!(first.id, second.id);
    Ok(())
}

#[test]
fn an_empty_anchor_on_an_empty_file_is_refused_and_leaves_the_file_empty() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "")]));
    let access = vfs.acquire(test_origin())?;
    match access.str_replace("/f.txt", "", "x") {
        Err(VfsError::Anchor {
            path,
            anchor,
            count,
        }) => {
            assert_eq!(path, "/f.txt");
            assert!(anchor.is_empty());
            assert_eq!(count, 0);
        }
        other => panic!("expected the empty-anchor refusal, got {other:?}"),
    }
    assert_eq!(access.read("/f.txt")?, b"");
    Ok(())
}

#[test]
fn an_empty_anchor_on_a_non_empty_file_is_refused_and_leaves_it_unchanged() -> Result<(), VfsError>
{
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "hello")]));
    let access = vfs.acquire(test_origin())?;
    match access.str_replace("/f.txt", "", "x") {
        Err(VfsError::Anchor {
            path,
            anchor,
            count,
        }) => {
            assert_eq!(path, "/f.txt");
            assert!(anchor.is_empty());
            assert_eq!(count, 0);
        }
        other => panic!("expected the empty-anchor refusal, got {other:?}"),
    }
    assert_eq!(access.read("/f.txt")?, b"hello");
    Ok(())
}

#[test]
fn read_range_slices_lines_one_based_and_inclusive() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
    let access = vfs.acquire(test_origin())?;
    assert_eq!(access.read_range("/f.txt", 2, None)?, "two\nthree");
    assert_eq!(access.read_range("/f.txt", 2, Some(99))?, "two\nthree");
    assert_eq!(access.read_range("/f.txt", 99, None)?, "");
    assert_eq!(access.read_range("/f.txt", 1, Some(1))?, "one");
    Ok(())
}

#[test]
fn read_range_rejects_invalid_bounds() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\n")]));
    let access = vfs.acquire(test_origin())?;
    assert_eq!(
        access.read_range("/f.txt", 0, None),
        Err(VfsError::InvalidRange {
            path: "/f.txt".to_owned(),
            reason: "start is below 1",
        })
    );
    assert_eq!(
        access.read_range("/f.txt", 2, Some(1)),
        Err(VfsError::InvalidRange {
            path: "/f.txt".to_owned(),
            reason: "end is before start",
        })
    );
    Ok(())
}

#[test]
fn read_range_numbered_numbers_absolutely_from_start() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
    let access = vfs.acquire(test_origin())?;
    assert_eq!(
        access.read_range_numbered("/f.txt", 1, None)?,
        "1| one\n2| two\n3| three"
    );
    assert_eq!(
        access.read_range_numbered("/f.txt", 2, Some(3))?,
        "2| two\n3| three"
    );
    assert_eq!(access.read_range_numbered("/f.txt", 99, None)?, "");
    Ok(())
}

#[test]
fn read_range_numbered_pads_to_the_widest_emitted_number() -> Result<(), VfsError> {
    let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
    let text = lines.join("\n");
    let vfs = handle(&StubFs::seeded(&[("/f.txt", &text)]));
    let access = vfs.acquire(test_origin())?;
    assert_eq!(
        access.read_range_numbered("/f.txt", 9, Some(10))?,
        " 9| line9\n10| line10"
    );
    Ok(())
}

#[test]
fn read_string_rejects_non_utf8() -> Result<(), VfsError> {
    let vfs = handle(&StubFs::default());
    let access = vfs.acquire(test_origin())?;
    access.write("/bin.dat", &[0xff, 0xfe])?;
    match access.read_string("/bin.dat") {
        Err(VfsError::NotUtf8 { path }) => {
            assert_eq!(path, "/bin.dat");
        }
        other => panic!("expected a UTF-8 failure, got {other:?}"),
    }
    Ok(())
}

#[test]
fn the_handle_and_capability_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VfsRef>();
    assert_send_sync::<Access>();
    assert_send_sync::<AcquireContext>();
}
