//! Download stop tests: cancellation at chunk boundaries and the idle read bound.

use super::*;

#[test]
fn a_cancelled_download_stops_at_the_next_chunk_boundary() {
    // The fixture serves a huge body one small chunk at a time, so the
    // transfer is always mid-stream when the token fires: the download loop
    // must stop at the next chunk boundary with the staged partial kept.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow server");
    let addr = listener.local_addr().expect("addr");
    let (served_tx, served_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let mut request = Vec::new();
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 1073741824\r\nConnection: close\r\n\r\n";
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        let chunk = [7_u8; 4096];
        // Signal only after the first body bytes are on the wire, so the
        // test cancels a transfer that has genuinely started.
        if stream.write_all(&chunk).is_err() || stream.flush().is_err() {
            return;
        }
        let _ = served_tx.send(());
        loop {
            if stream.write_all(&chunk).is_err() {
                // The client went away: the cancellation landed.
                return;
            }
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(5));
        }
    });

    let client = Client::builder().build().expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("slow.gguf");
    let url = format!("http://{addr}/slow.gguf");
    let token = CancellationToken::new();
    let worker = thread::spawn({
        let token = token.clone();
        let dest = dest.clone();
        move || {
            super::super::download::download_with_progress(
                &client,
                &url,
                &dest,
                &RecordingProgress::new(),
                Some(&token),
            )
        }
    });

    served_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the fixture served the first chunk");
    token.cancel();
    let err = worker
        .join()
        .expect("download thread")
        .expect_err("a cancelled transfer must fail");
    assert!(
        matches!(err, LocalError::Cancelled),
        "a mid-stream cancel surfaces as Cancelled, not a transport error: {err:?}"
    );
    let partial = std::fs::metadata(&dest)
        .expect("the staged partial stays for resume")
        .len();
    assert!(
        partial < 1_073_741_824,
        "the transfer stopped mid-stream at {partial} bytes"
    );
    let _ = server.join();
}

#[test]
fn a_pre_cancelled_download_makes_no_request_and_stages_no_file() {
    // The port is bound and immediately dropped, so any connection attempt
    // fails with a transport error: surfacing Cancelled instead proves the
    // token check ran before the resume negotiation.
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        addr
    };
    let client = Client::builder().build().expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("never.gguf");
    let url = format!("http://{addr}/never.gguf");
    let token = CancellationToken::new();
    token.cancel();
    let err = super::super::download::download_with_progress(
        &client,
        &url,
        &dest,
        &RecordingProgress::new(),
        Some(&token),
    )
    .expect_err("a pre-cancelled transfer must fail");
    assert!(
        matches!(err, LocalError::Cancelled),
        "the entry check precedes any request: {err:?}"
    );
    assert!(!dest.exists(), "a pre-cancelled transfer stages no file");
}

#[test]
fn download_read_timeout_fails_on_a_stalled_body() {
    // ART-003: a peer that sends headers then stalls the body must fail on the
    // client's idle read timeout, not pin the download thread indefinitely.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled server");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume request head
        // Promise a body but send none; the client's read timeout must fire.
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        // Hold the connection open by blocking on a read until the client drops.
        let _ = stream.read(&mut buf);
    });

    let client = Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("stalled.bin");
    let progress = RecordingProgress::new();
    let err = super::super::download::download_with_progress(
        &client,
        &format!("http://{addr}/stalled.bin"),
        &dest,
        &progress,
        None,
    )
    .expect_err("stalled body must fail");
    assert!(
        matches!(
            err,
            LocalError::DownloadRead { .. } | LocalError::Download { .. }
        ),
        "unexpected error {err:?}"
    );
    // Unblock the server's pending read so the thread can exit.
    let _ = TcpStream::connect(addr);
    let _ = handle.join();
}

#[test]
fn an_idle_body_fails_within_the_read_bound_and_keeps_the_partial() {
    // A peer that sends headers and then goes silent must surface as a
    // timed-out read at the chunk boundary: the error lands inside twice the
    // idle bound, and the staged partial plus its provenance marker stay on
    // disk for a later resume.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent server");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume the request head
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        // Headers sent, body never comes. The socket read is bounded so the
        // fixture always exits, even if the client never drops.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.read(&mut buf);
    });

    let idle = Duration::from_secs(1);
    // The client's whole-request ceiling sits past the idle bound, exactly
    // as in production: the idle error under test fires first, and the
    // ceiling later drops the parked body so the fixture thread can exit.
    let client = Client::builder().timeout(idle * 3).build().expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("silent.bin");
    let url = format!("http://{addr}/silent.bin");
    let started = std::time::Instant::now();
    let err = super::super::download::download_with_idle(
        &client,
        &url,
        &dest,
        &RecordingProgress::new(),
        None,
        idle,
    )
    .expect_err("a silent peer must fail the download");
    let elapsed = started.elapsed();
    let LocalError::DownloadRead { source, .. } = &err else {
        panic!("the stall surfaces as a read error: {err:?}");
    };
    assert_eq!(
        source.kind(),
        io::ErrorKind::TimedOut,
        "the stall is an idle timeout: {err:?}"
    );
    assert!(
        elapsed >= idle,
        "the peer is given the whole idle bound: {elapsed:?}"
    );
    assert!(
        elapsed < idle * 2,
        "the timeout fires within twice the idle bound: {elapsed:?}"
    );
    assert_eq!(
        std::fs::read(&dest).expect("partial kept"),
        b"",
        "no bytes were staged"
    );
    assert_eq!(
        std::fs::read_to_string(source_marker_path(&dest)).expect("marker kept"),
        url,
        "the provenance marker survives the failure"
    );
    // Dropping the client shuts down its runtime, which closes the stalled
    // connection and lets the fixture thread exit.
    drop(client);
    let _ = handle.join();
}
