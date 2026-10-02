//! Stage text tests for model, blob, and runtime provisioning, and percent republishing.

use gateway_config::WhisperBackend;

use super::super::assets::{ArchiveKind, LLAMA_RELEASE, X86_BASELINE, server_asset, whisper_asset};
use super::*;

#[test]
fn ensure_model_with_progress_writes_the_download_text_and_a_cache_hit_writes_nothing() {
    // A URL source flows through `ensure_blob`: the transfer writes its
    // percent into the activity, and the pin check reads the inline digest,
    // so no verify text follows the download.
    let body = b"model-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let hub = ProgressHub::new();
    let model = hub.begin("model");
    let url = server.url("model.gguf");
    let pin = hex_sha256(body);

    let path = store
        .ensure_model_with_progress(&url, Some(&pin), Some(&model))
        .expect("ensure model");
    assert_eq!(std::fs::read(&path).expect("read model"), body);
    assert_eq!(
        hub.current().text,
        "Downloading model.gguf 100%",
        "the pinned download ends at its last whole percent"
    );

    // A warm-cache repeat under a fresh activity: the marker hit runs no
    // hash pass and no transfer, so the activity text is untouched.
    let cached = hub.begin("cached");
    store
        .ensure_model_with_progress(&url, Some(&pin), Some(&cached))
        .expect("ensure model from cache");
    assert_eq!(
        hub.current().text,
        "cached",
        "a cache hit writes no stage text"
    );
    assert_eq!(server.requests(), 1, "the cache hit re-downloads nothing");
}

#[test]
fn ensure_blob_mismatch_repair_hashes_then_downloads() {
    // A cached blob whose content no longer matches the pin is repaired by
    // re-downloading: the hash pass names the verify stage, then the fresh
    // transfer names the download, and the pin recheck against the inline
    // digest adds no second verify text.
    let body = b"repaired-blob-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let destination = temp.path().join("downloads").join("model.gguf");
    std::fs::create_dir_all(destination.parent().expect("downloads parent"))
        .expect("mkdir downloads");
    std::fs::write(&destination, b"stale-bytes").expect("write stale blob");

    let hub = ProgressHub::new();
    let blob = hub.begin("blob");
    let url = server.url("model.gguf");
    let pin = hex_sha256(body);
    let asset = FileAsset {
        name: "model.gguf",
        url: &url,
        sha256: Some(&pin),
    };

    store
        .ensure_blob_with_progress(asset, &destination, Some(&blob), None)
        .expect("mismatch repair re-downloads");
    assert_eq!(std::fs::read(&destination).expect("read blob"), body);
    assert_eq!(server.requests(), 1, "the repair downloads once");
    assert_eq!(
        hub.current().text,
        "Downloading model.gguf 100%",
        "the transfer is the last stage; no verify text follows the inline pin check"
    );
}

#[test]
fn extract_failure_leaves_the_extracting_text_and_propagates() {
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.zip");
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

    let hub = ProgressHub::new();
    let activity = hub.begin("extract");

    let result = extract_archive_with_progress(&archive, &dest, ArchiveKind::Zip, Some(&activity));
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));
    assert_eq!(
        hub.current().text,
        "Extracting evil.zip",
        "the stage was named before the unsafe entry stopped it; the error reports the failure"
    );
}

#[test]
fn ensure_model_with_progress_rejects_a_bad_pin_before_any_stage_text() {
    // A path source whose pin cannot be parsed returns before any verify
    // work, so the activity text never moves.
    let dir = TempDir::new().expect("tempdir");
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"model-bytes").expect("write model");
    let store = ArtifactStore::new(dir.path().join("cache")).expect("store");

    let hub = ProgressHub::new();
    let parent = hub.begin("model");

    let result = store.ensure_model_with_progress(
        model.to_str().expect("utf8 path"),
        Some("abc"),
        Some(&parent),
    );
    assert!(matches!(result, Err(LocalError::InvalidDigest { .. })));
    assert_eq!(hub.current().text, "model");
}

#[test]
fn provision_server_writes_no_stage_text_on_a_warm_cache() {
    // A warm cache - the archive blob with a current verified marker and a
    // valid install tree - runs no download, hash, or extraction, so no
    // stage names reach the activity.
    // An explicit backend keeps the test deterministic on GPU machines:
    // provisioning must not probe nvidia-smi.
    let asset = server_asset(
        std::env::consts::OS,
        std::env::consts::ARCH,
        LlamaBackend::Vulkan,
        None,
    )
    .expect("host asset");
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");

    let archive_ref = &asset.archives[0];
    let archive = temp.path().join("downloads").join(archive_ref.archive_name);
    std::fs::create_dir_all(archive.parent().expect("downloads parent")).expect("mkdir downloads");
    std::fs::write(&archive, b"mock-archive-bytes").expect("write archive");
    // A marker hit trusts the recorded digest plus size and mtime without
    // re-hashing, so the fixture can record the pinned digest directly.
    write_marker(&blob_marker_path(&archive), &archive, archive_ref.sha256).expect("write marker");

    let install = temp
        .path()
        .join("llama.cpp")
        .join(format!("{LLAMA_RELEASE}-{}", asset.platform));
    std::fs::create_dir_all(&install).expect("mkdir install");
    std::fs::write(install.join(asset.executable_name), b"mock-server").expect("write executable");
    let tree_digest = super::super::digest::tree_digest(&install).expect("tree digest");
    let mut marker_text = String::new();
    for archive_ref in asset.archives {
        marker_text.push_str(archive_ref.sha256);
        marker_text.push('\n');
    }
    marker_text.push_str(&tree_digest);
    marker_text.push('\n');
    std::fs::write(install.join(INSTALL_MARKER), marker_text).expect("write install marker");

    let hub = ProgressHub::new();
    let server = hub.begin("llama-server");

    let provisioned = store
        .provision_llama_server_with_progress(
            &ServerSelection {
                server_path: None,
                backend: LlamaBackend::Vulkan,
            },
            Some(&server),
        )
        .expect("warm-cache provision");
    assert_eq!(provisioned.executable, install.join(asset.executable_name));
    assert!(provisioned.path_prefix.is_empty());
    assert_eq!(
        hub.current().text,
        "llama-server",
        "a valid install runs no stage and names none: {:?}",
        hub.current()
    );
}

#[test]
fn provision_whisper_library_reuses_a_verified_install() {
    // Explicit backends keep the host's GPU probe out of the test; on a
    // platform with both builds each one reuses its own install. The
    // provision itself checks this CPU against the x86 baseline.
    for backend in [WhisperBackend::Cpu, WhisperBackend::Cuda] {
        let asset = whisper_asset(
            std::env::consts::OS,
            std::env::consts::ARCH,
            backend,
            None,
            None,
            X86_BASELINE,
        )
        .expect("host whisper asset");
        let temp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(temp.path()).expect("store");

        let archive = temp
            .path()
            .join("downloads")
            .join(asset.archive.archive_name);
        std::fs::create_dir_all(archive.parent().expect("downloads parent"))
            .expect("mkdir downloads");
        std::fs::write(&archive, b"mock-archive-bytes").expect("write archive");
        write_marker(&blob_marker_path(&archive), &archive, asset.archive.sha256)
            .expect("write marker");

        let install = temp
            .path()
            .join("whisper.cpp")
            .join(format!("{WHISPER_RELEASE}-{}", asset.platform));
        std::fs::create_dir_all(&install).expect("mkdir install");
        std::fs::write(install.join(asset.library_name), b"mock-library").expect("write library");
        let tree_digest = super::super::digest::tree_digest(&install).expect("tree digest");
        std::fs::write(
            install.join(INSTALL_MARKER),
            format!("{}\n{tree_digest}\n", asset.archive.sha256),
        )
        .expect("write install marker");

        let hub = ProgressHub::new();
        let whisper = hub.begin("whisper-library");
        let provisioned = store
            .provision_whisper_library_with_cancellation(backend, Some(&whisper), None)
            .expect("warm-cache provision");
        assert_eq!(provisioned, install.join(asset.library_name), "{backend:?}");
        assert_eq!(
            hub.current().text,
            "whisper-library",
            "a verified whisper install runs no stage and names none: {backend:?}"
        );
    }
}

#[test]
fn a_cancelled_whisper_provision_downloads_nothing() {
    // The explicit CPU backend keeps the host's GPU probe out of the test,
    // and the empty cache would otherwise start the pinned download.
    let asset = whisper_asset(
        std::env::consts::OS,
        std::env::consts::ARCH,
        WhisperBackend::Cpu,
        None,
        None,
        X86_BASELINE,
    )
    .expect("host whisper asset");
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let token = CancellationToken::new();
    token.cancel();

    let err = store
        .provision_whisper_library_with_cancellation(WhisperBackend::Cpu, None, Some(&token))
        .expect_err("a fired token must stop the provision");
    assert!(
        matches!(err, LocalError::Cancelled),
        "a fired token surfaces as Cancelled: {err:?}"
    );
    let archive = temp
        .path()
        .join("downloads")
        .join(asset.archive.archive_name);
    assert!(!archive.exists(), "no archive was downloaded");
    assert!(!part_path(&archive).exists(), "no download was staged");
    assert!(
        !temp.path().join("whisper.cpp").exists(),
        "no install was created"
    );
}

#[test]
fn whisper_installs_never_fall_back_to_an_older_abi() {
    let asset = whisper_asset(
        std::env::consts::OS,
        std::env::consts::ARCH,
        WhisperBackend::Cpu,
        None,
        None,
        X86_BASELINE,
    )
    .expect("host whisper asset");
    let archives = [asset.archive];
    let install = whisper_install_asset(asset, &archives);
    assert!(
        !install.allow_cached_fallback,
        "an older self-consistent install may not match the pinned FFI layout"
    );
}

#[test]
fn llama_server_path_from_the_config_wins_over_the_download() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let exe = temp.path().join("llama-server.exe");
    std::fs::write(&exe, b"external").expect("write external server");
    let selection = ServerSelection {
        server_path: Some(exe.to_str().expect("utf8 path")),
        backend: LlamaBackend::Auto,
    };
    let provisioned = store
        .provision_llama_server_with_progress(&selection, None)
        .expect("an explicit server path provisions without a download");
    assert_eq!(provisioned.executable, exe);
    assert!(
        !temp.path().join("downloads").exists(),
        "no download ran for an explicit path"
    );
}

#[test]
fn a_missing_llama_server_path_is_an_error() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let missing = temp.path().join("absent.exe");
    let selection = ServerSelection {
        server_path: Some(missing.to_str().expect("utf8 path")),
        backend: LlamaBackend::Auto,
    };
    let error = store
        .provision_llama_server_with_progress(&selection, None)
        .expect_err("a missing explicit server must fail");
    assert!(error.to_string().contains("llama_server_path"), "{error}");
}

#[test]
fn percent_text_republishes_on_whole_percent_changes_only() {
    let hub = ProgressHub::new();
    let mut rx = hub.subscribe();
    let activity = hub.begin("download");
    let text = PercentText::new("Downloading", "m.gguf");
    assert_eq!(text.label(), "Downloading m.gguf");

    text.report(&activity, 50, 200);
    assert_eq!(hub.current().text, "Downloading m.gguf 25%");
    rx.mark_unchanged();
    // Sub-percent movement changes nothing and wakes no subscriber.
    text.report(&activity, 51, 200);
    assert!(
        !rx.has_changed().expect("the hub is alive"),
        "a move inside one percent republishes nothing"
    );
    text.report(&activity, 200, 200);
    assert_eq!(hub.current().text, "Downloading m.gguf 100%");
    // Past-the-end counts clamp rather than printing nonsense.
    text.report(&activity, 300, 200);
    assert_eq!(hub.current().text, "Downloading m.gguf 100%");

    // Without a total the bare label is published once.
    let unknown = PercentText::new("Downloading", "unknown.bin");
    unknown.report(&activity, 10, 0);
    assert_eq!(hub.current().text, "Downloading unknown.bin");
    rx.mark_unchanged();
    unknown.report(&activity, 20, 0);
    assert!(
        !rx.has_changed().expect("the hub is alive"),
        "an unknown length republishes the bare label only once"
    );
}

#[test]
fn download_without_a_content_length_keeps_the_bare_text() {
    // A `DownloadProgress` fed no length never sees a percent: the text
    // stays at the stage name set when the transfer began.
    let hub = ProgressHub::new();
    let activity = hub.begin("download");
    let progress = ActivityProgress::new(&activity, "blob.bin");
    assert_eq!(hub.current().text, "Downloading blob.bin");
    progress.set_len(None);
    progress.inc(10);
    assert_eq!(hub.current().text, "Downloading blob.bin");
    progress.set_len(Some(100));
    progress.inc(40);
    assert_eq!(
        hub.current().text,
        "Downloading blob.bin 50%",
        "the first length makes every byte so far count"
    );
}
