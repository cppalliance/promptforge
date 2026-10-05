//! Download transfer tests: HF auth scoping, progress callbacks, and resume.

use super::*;

#[test]
fn hf_token_host_allowlist() {
    assert!(is_huggingface_https(
        "https://huggingface.co/org/repo/resolve/main/model.gguf"
    ));
    assert!(is_huggingface_https(
        "https://cdn-lfs.huggingface.co/repo/model.gguf"
    ));
    // Plaintext HTTP, arbitrary hosts, and look-alikes get no token.
    assert!(!is_huggingface_https(
        "http://huggingface.co/org/repo/model.gguf"
    ));
    assert!(!is_huggingface_https("https://evil.example/model.gguf"));
    assert!(!is_huggingface_https(
        "https://huggingface.co.evil.example/model.gguf"
    ));
    assert!(!is_huggingface_https("not a url"));
}

#[test]
fn download_with_progress_reports_content_length_and_bytes() {
    let body = b"progress-fixture-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let progress = RecordingProgress::new();
    let dest = temp.path().join("out.gguf");
    let digest = store
        .download_with_progress(&server.url("out.gguf"), &dest, &progress)
        .expect("download");
    assert_eq!(digest, hex_sha256(body));
    assert_eq!(
        *progress.total.lock().expect("total"),
        Some(body.len() as u64)
    );
    assert_eq!(progress.bytes.load(Ordering::Relaxed), body.len() as u64);
}

/// Seeds an interrupted download: `partial` bytes at `dest` plus the
/// provenance marker naming `source`.
fn seed_partial(dest: &std::path::Path, partial: &[u8], source: &str) {
    std::fs::write(dest, partial).expect("write partial");
    std::fs::write(source_marker_path(dest), source).expect("write provenance marker");
}

#[test]
fn an_interrupted_download_resumes_from_the_partials_offset() {
    let body = b"resume-fixture: a body long enough to have a middle";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("resumed.gguf");
    let dest = temp.path().join("resumed.gguf.part");
    let offset = 20_u64;
    seed_partial(
        &dest,
        &body[..usize::try_from(offset).expect("fixture offset")],
        &url,
    );
    let progress = RecordingProgress::new();

    let digest = store
        .download_with_progress(&url, &dest, &progress)
        .expect("resume completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(offset)],
        "the retry continues at the partial's offset"
    );
    assert_eq!(
        *progress.total.lock().expect("total"),
        Some(body.len() as u64),
        "the declared total covers the whole blob"
    );
    assert_eq!(
        progress.bytes.load(Ordering::Relaxed),
        body.len() as u64,
        "the resumed bytes count toward the total"
    );
    assert!(
        !source_marker_path(&dest).exists(),
        "a completed transfer removes the marker"
    );
}

#[test]
fn a_200_answer_to_a_range_request_restarts_from_zero() {
    // A server that ignores the Range header answers 200 with the whole
    // body; the partial is truncated and the transfer starts over.
    let body = b"restart-fixture-body";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("restart.gguf");
    let dest = temp.path().join("restart.gguf.part");
    seed_partial(&dest, &body[..10], &url);

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("restart completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(10), None],
        "the Range attempt is followed by a plain GET"
    );
}

#[test]
fn a_partial_larger_than_the_declared_size_restarts() {
    // The partial cannot belong to a blob smaller than itself: the Range
    // request is unsatisfiable (416) and the transfer restarts from zero.
    let body = b"declared-size-fixture";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("oversized.gguf");
    let dest = temp.path().join("oversized.gguf.part");
    let oversized = body.len() as u64 + 9;
    seed_partial(
        &dest,
        &vec![b'x'; usize::try_from(oversized).expect("fixture size")],
        &url,
    );

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("restart completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(oversized), None],
        "the unsatisfiable Range is followed by a plain GET"
    );
}

#[test]
fn a_partial_with_a_mismatched_marker_is_discarded() {
    // Provenance is the resume gate: a partial recorded against another
    // source is never appended to.
    let body = b"provenance-fixture-body";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("provenance.gguf");
    let dest = temp.path().join("provenance.gguf.part");
    seed_partial(&dest, b"foreign-bytes", "http://other.example/foreign.gguf");

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("fresh download completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[None],
        "no Range is sent for a foreign partial"
    );
}

#[test]
fn a_short_transfer_keeps_the_partial_and_marker_for_resume() {
    // A body that ends early against its declared length is a failed
    // transfer: the partial and its provenance marker stay on disk so the
    // next attempt resumes from the offset.
    let body = b"short-transfer-fixture-body";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind short server");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume the request head
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body[..8]); // part of the body, then close
        let _ = stream.flush();
    });

    let client = Client::builder().build().expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("short.bin");
    let url = format!("http://{addr}/short.bin");
    let err = super::super::download::download_with_progress(
        &client,
        &url,
        &dest,
        &RecordingProgress::new(),
        None,
    )
    .expect_err("a short body must fail");
    assert!(
        matches!(
            err,
            LocalError::DownloadRead { .. } | LocalError::Download { .. }
        ),
        "unexpected error {err:?}"
    );
    assert_eq!(
        std::fs::read(&dest).expect("partial kept"),
        &body[..8],
        "the transferred prefix stays on disk"
    );
    assert_eq!(
        std::fs::read_to_string(source_marker_path(&dest)).expect("marker kept"),
        url,
        "the provenance marker survives the failure"
    );
    // Unblock the server's pending state so the thread can exit.
    let _ = TcpStream::connect(addr);
    let _ = handle.join();
}

#[test]
fn hub_bearer_token_prefers_hf_token() {
    let token = hub_bearer_token(|key| match key {
        "HF_TOKEN" => Some(" hf_primary ".to_owned()),
        "HUGGING_FACE_HUB_TOKEN" => Some("hf_secondary".to_owned()),
        _ => None,
    });
    assert_eq!(token.as_deref(), Some("hf_primary"));
}

#[test]
fn hub_bearer_token_falls_back_to_hugging_face_hub_token() {
    let token = hub_bearer_token(|key| match key {
        "HUGGING_FACE_HUB_TOKEN" => Some("hf_fallback".to_owned()),
        _ => None,
    });
    assert_eq!(token.as_deref(), Some("hf_fallback"));
}

#[test]
fn hub_bearer_token_ignores_empty_and_missing() {
    assert!(hub_bearer_token(|_| None).is_none());
    assert!(hub_bearer_token(|_| Some(String::new())).is_none());
    assert!(hub_bearer_token(|_| Some("   ".to_owned())).is_none());
    assert_eq!(
        hub_bearer_token(|key| match key {
            "HF_TOKEN" => Some(String::new()),
            "HUGGING_FACE_HUB_TOKEN" => Some("hf_ok".to_owned()),
            _ => None,
        })
        .as_deref(),
        Some("hf_ok")
    );
}
