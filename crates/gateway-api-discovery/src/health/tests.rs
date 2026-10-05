//! Unit tests for the health wait and the bearer probe.

use super::*;

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

#[path = "tests-connection.rs"]
mod connection;

/// Answers every connection with a canned response until the test
/// stops it.
fn spawn_stub_server(response: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let address = listener.local_addr().expect("stub server address");
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(response);
        }
    });
    format!("http://{address}")
}

#[test]
fn a_200_answer_satisfies_the_probe() {
    let base_url = spawn_stub_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
    wait_for_health(&base_url, Duration::from_secs(5)).expect("the stub answers 200");
}

#[test]
fn a_non_200_answer_is_retried_until_timeout() {
    let base_url =
        spawn_stub_server(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
    let error = wait_for_health(&base_url, Duration::from_millis(150))
        .expect_err("a 503 never satisfies the probe");
    assert!(
        matches!(error, HealthError::Timeout { .. }),
        "a 503 past the budget is a timeout"
    );
    let message = error.to_string();
    assert!(
        message.contains("did not answer"),
        "the error names the timeout: {message}"
    );
}

#[test]
fn a_dead_port_times_out() {
    // Port 1 is never listening, so every connect fails fast.
    let error = wait_for_health("http://127.0.0.1:1", Duration::from_millis(150))
        .expect_err("a dead port never satisfies the probe");
    assert!(
        matches!(error, HealthError::Timeout { .. }),
        "a dead port past the budget is a timeout"
    );
    let message = error.to_string();
    assert!(
        message.contains("did not answer"),
        "the error names the timeout: {message}"
    );
}

#[test]
fn a_non_http_url_is_rejected() {
    let error = wait_for_health("ftp://127.0.0.1:7910", Duration::from_millis(10))
        .expect_err("a non-http URL must fail");
    assert!(
        matches!(error, HealthError::NotHttp { .. }),
        "a non-http URL is rejected before any probe"
    );
    assert!(
        error.to_string().contains("http://"),
        "the error names the scheme requirement: {error}"
    );
}

#[test]
fn the_probe_sends_the_bound_address_as_host() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind recording server");
    let address = listener.local_addr().expect("recording server address");
    let (requests, received) = mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 1024];
            if let Ok(read) = stream.read(&mut buffer) {
                let _ = requests.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
            }
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        }
    });
    wait_for_health(&format!("http://{address}"), Duration::from_secs(5))
        .expect("the stub answers 200");
    let request = received
        .recv_timeout(Duration::from_secs(5))
        .expect("the probe's request arrived");
    assert!(
        request.contains(&format!("Host: {address}\r\n")),
        "the Host header is the bound address, not localhost: {request:?}"
    );
}

#[test]
fn a_bearer_probe_accepts_a_2xx_and_rejects_a_401() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind key fixture");
    let address = listener
        .local_addr()
        .expect("key fixture address")
        .to_string();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 1024];
            let Ok(read) = stream.read(&mut buffer) else {
                continue;
            };
            let request = String::from_utf8_lossy(&buffer[..read]);
            let response = if request.contains("Authorization: Bearer right\r\n") {
                &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
            } else {
                &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
            };
            let _ = stream.write_all(response);
        }
    });
    assert_eq!(
        probe_bearer(&address, "/v1/models", "right"),
        KeyProbe::Accepted
    );
    assert_eq!(
        probe_bearer(&address, "/v1/models", "wrong"),
        KeyProbe::Rejected
    );
    assert_eq!(
        probe_bearer("127.0.0.1:1", "/v1/models", "right"),
        KeyProbe::Unreachable
    );
}

#[test]
fn cancellation_at_health_probe_start_prevents_the_probe() {
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let probes = Arc::new(AtomicUsize::new(0));
    let worker_probes = Arc::clone(&probes);
    let (entered, blocked) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        wait_for_health_cancellable_with_start(
            "http://127.0.0.1:1",
            Duration::from_secs(30),
            &worker_cancellation,
            CANCELLABLE_ATTEMPT_TIMEOUT,
            || {
                entered.send(()).expect("announce health probe boundary");
                released.recv().expect("release health probe boundary");
            },
            |_, _| {
                worker_probes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
    });
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("health wait pauses immediately before probe admission");

    cancellation.cancel();
    release.send(()).expect("release health probe boundary");
    let result = worker.join().expect("health worker joins");

    assert!(matches!(result, Err(HealthError::Cancelled)));
    assert_eq!(
        probes.load(Ordering::SeqCst),
        0,
        "no health probe starts after cancellation returns"
    );
}

#[test]
fn cancellation_joins_a_blocked_health_wait_without_another_probe() {
    let cancellation = crate::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let probes = Arc::new(AtomicUsize::new(0));
    let worker_probes = Arc::clone(&probes);
    let (entered, blocked) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        wait_for_health_cancellable_with(
            "http://127.0.0.1:1",
            Duration::from_secs(30),
            &worker_cancellation,
            CANCELLABLE_ATTEMPT_TIMEOUT,
            |_, _| {
                worker_probes.fetch_add(1, Ordering::SeqCst);
                entered.send(()).expect("announce blocked health probe");
                let _ = worker_cancellation.wait_timeout(Duration::from_secs(30));
                Err(ProbeError::UnexpectedStatus {
                    status_line: "blocked".to_owned(),
                })
            },
        )
    });
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the health phase blocks deterministically");

    let started = Instant::now();
    cancellation.cancel();
    let result = worker.join().expect("the health worker joins");

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "cancellation bounds the blocked health wait"
    );
    assert!(matches!(result, Err(HealthError::Cancelled)));
    assert_eq!(
        probes.load(Ordering::SeqCst),
        1,
        "no probe starts after cancellation"
    );
}
