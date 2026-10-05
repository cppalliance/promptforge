//! Archive extraction tests: entry path safety, entry classes, executables, and the entry percent.

use super::super::archive::find_executable;
use super::super::assets::ArchiveKind;
use super::*;

#[test]
fn safe_archive_path_rejects_traversal_and_absolute() {
    assert!(!safe_archive_path(std::path::Path::new("../evil")));
    assert!(!safe_archive_path(std::path::Path::new("/etc/passwd")));
    assert!(!safe_archive_path(std::path::Path::new("a/../../b")));
    assert!(safe_archive_path(std::path::Path::new("bin/llama-server")));
}

#[test]
fn extract_zip_rejects_traversal_entry_and_cleans_up() {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.zip");
    // Build a zip whose single entry escapes the destination. `start_file`
    // does not sanitize the name, so this exercises the extractor's own guard.
    {
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("../escape.txt", SimpleFileOptions::default())
            .expect("start traversal entry");
        writer.write_all(b"pwned").expect("write entry");
        writer.finish().expect("finish zip");
    }

    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::Zip);
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));
    // The traversal target must never have been written outside the destination.
    assert!(!dir.path().join("escape.txt").exists());
}

fn tar_gz_with_symlink() -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_path("evil-link").expect("set path");
    header.set_link_name("/etc/passwd").expect("set link");
    header.set_cksum();
    builder
        .append(&header, io::empty())
        .expect("append symlink");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gz")
}

#[test]
fn extract_tar_gz_rejects_symlink_entries() {
    // ART-007: a tar entry that is neither a regular file nor a directory (here
    // a symlink) is rejected rather than materialized in the cache tree.
    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.tar.gz");
    std::fs::write(&archive, tar_gz_with_symlink()).expect("write archive");
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::TarGz);
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));
    assert!(!dest.join("evil-link").exists());
}

fn tar_gz_entry(entry_type: tar::EntryType, name: &str, link: Option<&str>) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_size(0);
    header.set_mode(0o644);
    header.set_path(name).expect("set path");
    if let Some(link) = link {
        header.set_link_name(link).expect("set link");
    }
    header.set_cksum();
    builder.append(&header, io::empty()).expect("append entry");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gz")
}

fn zip_symlink(name: &str, target: &str) -> Vec<u8> {
    use zip::write::SimpleFileOptions;

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        writer
            .add_symlink(name, target, SimpleFileOptions::default())
            .expect("add symlink");
        writer.finish().expect("finish zip");
    }
    cursor.into_inner()
}

#[test]
fn extract_rejects_every_non_regular_entry_class() {
    // ART-007: table-driven rejection of each unsafe/unsupported archive entry
    // class - tar symlink/hardlink/char/block/fifo and a zip symlink - so none
    // is materialized in the cache tree.
    let tar_cases: &[(tar::EntryType, Option<&str>)] = &[
        (tar::EntryType::Symlink, Some("/etc/passwd")),
        (tar::EntryType::Link, Some("llama-server")),
        (tar::EntryType::Char, None),
        (tar::EntryType::Block, None),
        (tar::EntryType::Fifo, None),
    ];
    for (entry_type, link) in tar_cases {
        let dir = TempDir::new().expect("tempdir");
        let archive = dir.path().join("evil.tar.gz");
        std::fs::write(&archive, tar_gz_entry(*entry_type, "entry", *link)).expect("write archive");
        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).expect("mkdir dest");
        let result = extract_archive(&archive, &dest, ArchiveKind::TarGz);
        assert!(
            matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })),
            "tar {entry_type:?} should be rejected, got {result:?}"
        );
        assert!(!dest.join("entry").exists());
    }

    // A zip symlink entry (unix mode S_IFLNK) is likewise rejected.
    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("link.zip");
    std::fs::write(&archive, zip_symlink("link", "/etc/passwd")).expect("write archive");
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::Zip);
    assert!(
        matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })),
        "zip symlink should be rejected, got {result:?}"
    );
    assert!(!dest.join("link").exists());
}

#[test]
fn find_executable_rejects_duplicates_and_reports_missing() {
    // ART-007: two matching executables are a hard error, not a silent pick;
    // zero matches is a distinct missing error.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir(root.join("a")).expect("mkdir a");
    std::fs::create_dir(root.join("b")).expect("mkdir b");
    std::fs::write(root.join("a").join("llama-server"), b"x").expect("write a");
    std::fs::write(root.join("b").join("llama-server"), b"y").expect("write b");
    assert!(matches!(
        find_executable(root, "llama-server", "arc"),
        Err(LocalError::DuplicateExecutable { .. })
    ));
    assert!(matches!(
        find_executable(root, "absent", "arc"),
        Err(LocalError::MissingExecutable { .. })
    ));
}

#[test]
fn extract_zip_writes_the_entry_percent() {
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("bundle.zip");
    {
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        for index in 0..4 {
            writer
                .start_file(format!("file-{index}.txt"), SimpleFileOptions::default())
                .expect("start entry");
            writer.write_all(b"data").expect("write entry");
        }
        writer.finish().expect("finish zip");
    }
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");

    let hub = ProgressHub::new();
    let activity = hub.begin("extract");

    extract_archive_with_progress(&archive, &dest, ArchiveKind::Zip, Some(&activity))
        .expect("extract");
    assert_eq!(hub.current().text, "Extracting bundle.zip 100%");
}

#[test]
fn extract_tar_gz_writes_the_entry_percent() {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("bundle.tar.gz");
    {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for name in ["a.txt", "b.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(4);
            header.set_mode(0o644);
            header.set_path(name).expect("set path");
            header.set_cksum();
            builder.append(&header, &b"data"[..]).expect("append entry");
        }
        let bytes = builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gz");
        std::fs::write(&archive, bytes).expect("write archive");
    }
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");

    let hub = ProgressHub::new();
    let activity = hub.begin("extract");

    extract_archive_with_progress(&archive, &dest, ArchiveKind::TarGz, Some(&activity))
        .expect("extract");
    assert_eq!(hub.current().text, "Extracting bundle.tar.gz 100%");
}
