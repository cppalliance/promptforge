//! Cache listing, removal, and orphan scan tests.

use super::*;

#[test]
fn list_returns_sidecar_bearing_blobs_with_metadata() {
    let body_a = b"listing-fixture-a";
    let body_b = b"listing-fixture-bb";
    let server_a = FakeServer::new(body_a);
    let server_b = FakeServer::new(body_b);
    let temp = TempDir::new().expect("tempdir");
    let cache = BlobCache::new(temp.path()).expect("cache");
    let url_a = server_a.url("a.gguf");
    let url_b = server_b.url("b.gguf");
    let blob_a = cache
        .download_to_cache(&url_a, None, &RecordingProgress::new())
        .expect("download a");
    let blob_b = cache
        .download_to_cache(&url_b, None, &RecordingProgress::new())
        .expect("download b");

    // A bare blob without a sidecar (a pre-existing local model file) is
    // not listed; nor is a stale sidecar whose blob is gone.
    let bare_dir = temp.path().join("models").join("0123456789abcdef");
    fs::create_dir_all(&bare_dir).expect("mkdir bare slot");
    fs::write(bare_dir.join("bare.gguf"), b"bare").expect("write bare blob");
    let stale_dir = temp.path().join("models").join("fedcba9876543210");
    fs::create_dir_all(&stale_dir).expect("mkdir stale slot");
    fs::write(
        stale_dir.join("gone.gguf.meta.json"),
        r#"{"source":"http://x/gone.gguf","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":4}"#,
    )
    .expect("write stale sidecar");

    let entries = cache.list().expect("list");
    // Sorted by source for a stable response. The fake servers bind
    // ephemeral ports, so which of the two sources sorts first is not
    // fixed; build the expectation and sort it the same way.
    let mut expected = vec![
        CacheEntry {
            source: url_a,
            path: blob_a.path,
            sha256: hex_sha256(body_a),
            size_bytes: body_a.len() as u64,
        },
        CacheEntry {
            source: url_b,
            path: blob_b.path,
            sha256: hex_sha256(body_b),
            size_bytes: body_b.len() as u64,
        },
    ];
    expected.sort_by(|left, right| left.source.cmp(&right.source));
    assert_eq!(entries, expected);
}

#[test]
fn remove_deletes_blob_and_sidecar_by_digest() {
    let body = b"delete-me-fixture";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let cache = BlobCache::new(temp.path()).expect("cache");
    let url = server.url("gone.gguf");
    let blob = cache
        .download_to_cache(&url, None, &RecordingProgress::new())
        .expect("download");
    let sidecar = meta_path(&blob.path);
    assert!(blob.path.is_file() && sidecar.is_file());

    assert!(cache.remove(&blob.sha256).expect("remove"));
    assert!(!blob.path.exists(), "blob removed");
    assert!(!sidecar.exists(), "sidecar removed");
    assert!(cache.list().expect("list").is_empty());

    // A second removal of the same digest reports not-found.
    assert!(!cache.remove(&blob.sha256).expect("remove again"));
    // A malformed digest is rejected at the boundary.
    assert!(matches!(
        cache.remove("not-hex"),
        Err(LocalError::InvalidDigest { .. })
    ));
}

#[test]
fn orphans_reports_only_unreferenced_files() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    let models = root.join("models");
    fs::create_dir_all(&models).expect("mkdir models");

    // Configured coverage: a URL source in its provisioning cache slot,
    // a path source, and companion (speculative + projector) path
    // sources. None of these may appear as orphans.
    let url = "https://example.test/repo/pinned.gguf";
    let slot = models.join(source_cache_key(url));
    fs::create_dir_all(&slot).expect("mkdir slot");
    fs::write(slot.join("pinned.gguf"), b"pinned-bytes").expect("write pinned");
    let local = models.join("local.gguf");
    let draft = models.join("draft.gguf");
    let projector = models.join("mmproj.gguf");
    fs::write(&local, b"local-bytes").expect("write local");
    fs::write(&draft, b"draft-bytes").expect("write draft");
    fs::write(&projector, b"mmproj-bytes").expect("write projector");
    // A path source spelled through a redundant `..` component never
    // equals the walked path component-wise, so only the canonicalize
    // fallback can match it; this file turning up as an orphan means
    // that fallback broke.
    let variant = models.join("variant.gguf");
    fs::write(&variant, b"variant-bytes").expect("write variant");
    let variant_spelling = models.join("..").join("models").join("variant.gguf");

    // Orphans: a bare top-level file, a slot-nested file with a cache
    // sidecar (its digest is reused, never re-hashed), and a file whose
    // corrupt sidecar downgrades to no digest.
    fs::write(models.join("stray.gguf"), b"stray-bytes").expect("write stray");
    let cached_body: &[u8] = b"cached-bytes";
    let cached_slot = models.join("0123456789abcdef");
    fs::create_dir_all(&cached_slot).expect("mkdir cached slot");
    fs::write(cached_slot.join("cached.gguf"), cached_body).expect("write cached");
    let cached_digest = hex_sha256(cached_body);
    fs::write(
        cached_slot.join("cached.gguf.meta.json"),
        serde_json::json!({
            "source": "http://seeded.example/cached.gguf",
            "sha256": cached_digest,
            "size_bytes": cached_body.len(),
        })
        .to_string(),
    )
    .expect("write cached sidecar");
    fs::write(models.join("corrupt.gguf"), b"corrupt-body").expect("write corrupt");
    fs::write(models.join("corrupt.gguf.meta.json"), b"not json").expect("corrupt sidecar");

    // Bookkeeping noise that must never be listed: a model-card sidecar
    // and a staging file.
    fs::write(models.join("local.md"), b"card").expect("write card");
    fs::write(models.join("local.gguf.verified"), b"marker").expect("write marker");
    fs::write(models.join("stray.gguf.part"), b"partial").expect("write staging");

    let config = gateway_config::Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "pinned"
description = "a url-sourced model"
source = "{url}"
sha256 = "{pin}"
context = 4096

[[local_model]]
name = "local"
description = "a path-sourced model with companions"
source = '{local}'
context = 4096

[local_model.speculative]
type = "draft-mtp"
source = '{draft}'
draft_max = 2

[local_model.multimodal_projector]
source = '{projector}'

[[local_model]]
name = "variant"
description = "a path-sourced model spelled through a redundant component"
source = '{variant_spelling}'
context = 4096
"#,
        pin = hex_sha256(b"pinned-bytes"),
        local = local.display(),
        draft = draft.display(),
        projector = projector.display(),
        variant_spelling = variant_spelling.display(),
    ))
    .expect("config");

    let entries = orphans(root, config.local_models(), &[]).expect("orphans");
    assert_eq!(
        entries,
        vec![
            OrphanEntry {
                path: "models/0123456789abcdef/cached.gguf".to_owned(),
                size_bytes: cached_body.len() as u64,
                sha256: Some(cached_digest),
            },
            OrphanEntry {
                path: "models/corrupt.gguf".to_owned(),
                size_bytes: b"corrupt-body".len() as u64,
                sha256: None,
            },
            OrphanEntry {
                path: "models/stray.gguf".to_owned(),
                size_bytes: b"stray-bytes".len() as u64,
                sha256: None,
            },
        ]
    );
}

#[test]
fn orphans_without_a_models_directory_is_empty() {
    let temp = TempDir::new().expect("tempdir");
    assert_eq!(orphans(temp.path(), &[], &[]).expect("orphans"), Vec::new());
}
