//! Readiness and key probes: raw loopback HTTP/1.x over
//! `std::net::TcpStream`, enough to read a status line, with no HTTP
//! client dependency.
//!
//! Moved from the workshop shell's `health.rs`; the only change in the
//! move is the `Host` header, which now carries the bound loopback
//! address instead of `localhost`, matching the gateway's loopback `Host`
//! allowlist.

use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::CancellationToken;

/// Delay between probes while the server comes up.
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Per-attempt connect and read timeout, so one hung attempt cannot eat
/// the whole budget.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-attempt I/O budget while a supervisor may be cancelled.
const CANCELLABLE_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(100);
/// Maximum accepted response head for the two-request validation proof.
const RESPONSE_HEAD_LIMIT: usize = 16 * 1024;
/// Maximum body drained between the health and bearer responses.
const RESPONSE_BODY_LIMIT: usize = 64 * 1024;

/// A failure of [`wait_for_health`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HealthError {
    /// The caller cancelled the health wait.
    #[error("the health wait was cancelled")]
    Cancelled,

    /// The URL is not an `http://` URL.
    #[error("health probe needs an http:// URL, got {url}")]
    NotHttp {
        /// The offending URL.
        url: String,
    },

    /// No 200 answer arrived within the budget.
    #[error("the server did not answer {url}/health within {timeout:?}")]
    Timeout {
        /// The probed base URL.
        url: String,
        /// The budget that elapsed.
        timeout: Duration,
        /// The last attempt's failure.
        #[source]
        source: ProbeError,
    },
}

/// One probe attempt failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// The connect, write, or read failed.
    #[error("{operation}")]
    Io {
        /// The failed operation.
        operation: &'static str,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The status line was not a 200.
    #[error("unexpected health response: {status_line}")]
    UnexpectedStatus {
        /// The response's first line.
        status_line: String,
    },
}

/// The outcome of one bearer-key probe.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyProbe {
    /// A 2xx answer: the key is accepted.
    Accepted,
    /// A non-2xx answer: the key is rejected (or the route is gone).
    Rejected,
    /// The server could not be queried at all.
    Unreachable,
}

/// The combined readiness and authority proof for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionProbe {
    /// Health answered and the bearer was accepted.
    Accepted,
    /// The caller cancelled validation.
    Cancelled,
    /// Health failed, including a bearer probe that became unreachable.
    HealthFailed,
    /// Health answered but the bearer was rejected.
    KeyRejected,
}

/// Proves that one address is healthy and accepts the presented bearer.
/// Both requests use one TCP connection, so authority cannot come from a
/// listener that replaced the endpoint after the health response.
pub(crate) fn probe_connection(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    health_budget: Duration,
) -> ConnectionProbe {
    probe_connection_with(
        address,
        bearer_path,
        bearer,
        health_budget,
        &CancellationToken::new(),
        ATTEMPT_TIMEOUT,
    )
}

/// Proves one connection while observing supervisor cancellation.
pub(crate) fn probe_connection_cancellable(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    health_budget: Duration,
    cancellation: &CancellationToken,
) -> ConnectionProbe {
    probe_connection_with(
        address,
        bearer_path,
        bearer,
        health_budget,
        cancellation,
        CANCELLABLE_ATTEMPT_TIMEOUT,
    )
}

fn probe_connection_with(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    health_budget: Duration,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
) -> ConnectionProbe {
    probe_connection_with_probe(
        address,
        bearer_path,
        bearer,
        health_budget,
        cancellation,
        attempt_timeout,
        probe_connection_once,
    )
}

fn probe_connection_with_probe(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    health_budget: Duration,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
    mut probe: impl FnMut(&str, &str, &str, Instant) -> ConnectionAttempt,
) -> ConnectionProbe {
    let deadline = Instant::now() + health_budget;
    loop {
        let attempt_deadline = (Instant::now() + attempt_timeout).min(deadline);
        let Some(attempt) =
            cancellation.run_if_active(|| probe(address, bearer_path, bearer, attempt_deadline))
        else {
            return ConnectionProbe::Cancelled;
        };
        match attempt {
            ConnectionAttempt::Accepted => return ConnectionProbe::Accepted,
            ConnectionAttempt::KeyRejected => return ConnectionProbe::KeyRejected,
            ConnectionAttempt::ProofInterrupted => return ConnectionProbe::HealthFailed,
            ConnectionAttempt::HealthFailed if Instant::now() >= deadline => {
                return ConnectionProbe::HealthFailed;
            }
            ConnectionAttempt::HealthFailed if cancellation.wait_timeout(RETRY_INTERVAL) => {
                return ConnectionProbe::Cancelled;
            }
            ConnectionAttempt::HealthFailed => {}
        }
    }
}

/// One coherent proof attempt over one TCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionAttempt {
    Accepted,
    HealthFailed,
    KeyRejected,
    ProofInterrupted,
}

/// Checks health and bearer acceptance over one socket. Once health has
/// succeeded, any socket loss fails the proof instead of reconnecting to a
/// potentially different endpoint.
fn probe_connection_once(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    deadline: Instant,
) -> ConnectionAttempt {
    let Ok(socket) = address.parse::<SocketAddr>() else {
        return ConnectionAttempt::HealthFailed;
    };
    let Ok(mut stream) = connect_until(&socket, deadline) else {
        return ConnectionAttempt::HealthFailed;
    };
    if write_request(&mut stream, address, "/health", None, false, deadline).is_err() {
        return ConnectionAttempt::HealthFailed;
    }
    let Ok(health_head) = read_framed_response_head(&mut stream, deadline) else {
        return ConnectionAttempt::HealthFailed;
    };
    if response_status(&health_head) != Some(200) {
        return ConnectionAttempt::HealthFailed;
    }
    if write_request(
        &mut stream,
        address,
        bearer_path,
        Some(bearer),
        true,
        deadline,
    )
    .is_err()
    {
        return ConnectionAttempt::ProofInterrupted;
    }
    let Ok(bearer_head) = read_framed_response_head(&mut stream, deadline) else {
        return ConnectionAttempt::ProofInterrupted;
    };
    if response_status(&bearer_head).is_some_and(|code| (200..300).contains(&code)) {
        ConnectionAttempt::Accepted
    } else {
        ConnectionAttempt::KeyRejected
    }
}

/// Connects within the one absolute attempt deadline.
fn connect_until(address: &SocketAddr, deadline: Instant) -> Result<TcpStream, ProbeError> {
    let timeout = remaining(deadline)?;
    TcpStream::connect_timeout(address, timeout).map_err(|source| ProbeError::Io {
        operation: "connect",
        source,
    })
}

/// Applies the remaining absolute attempt budget to reads and writes.
fn configure_stream(stream: &TcpStream, deadline: Instant) -> Result<(), ProbeError> {
    let timeout = remaining(deadline)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|source| ProbeError::Io {
            operation: "configure the read timeout",
            source,
        })?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|source| ProbeError::Io {
            operation: "configure the write timeout",
            source,
        })
}

fn remaining(deadline: Instant) -> Result<Duration, ProbeError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| ProbeError::Io {
            operation: "validation deadline elapsed",
            source: std::io::Error::from(std::io::ErrorKind::TimedOut),
        })
}

/// Writes one GET request, retaining or closing the connection as directed.
fn write_request(
    stream: &mut TcpStream,
    address: &str,
    path: &str,
    bearer: Option<&str>,
    close: bool,
    deadline: Instant,
) -> Result<(), ProbeError> {
    let connection = if close { "close" } else { "keep-alive" };
    let mut request =
        format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: {connection}\r\n");
    if let Some(key) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(key);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    configure_stream(stream, deadline)?;
    stream
        .write_all(request.as_bytes())
        .map_err(|source| ProbeError::Io {
            operation: "write the validation request",
            source,
        })
}

/// Reads one response head and drains its fixed-length body so the next
/// response starts at a framing boundary on the same socket.
fn read_framed_response_head(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<String, ProbeError> {
    let mut response = Vec::with_capacity(512);
    let header_end = loop {
        if let Some(end) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break end + 4;
        }
        if response.len() >= RESPONSE_HEAD_LIMIT {
            return Err(ProbeError::UnexpectedStatus {
                status_line: "<response head too large>".to_owned(),
            });
        }
        let mut buffer = [0_u8; 512];
        let available = RESPONSE_HEAD_LIMIT - response.len();
        let chunk = available.min(buffer.len());
        configure_stream(stream, deadline)?;
        let read = stream
            .read(&mut buffer[..chunk])
            .map_err(|source| ProbeError::Io {
                operation: "read the validation response",
                source,
            })?;
        if read == 0 {
            return Err(ProbeError::Io {
                operation: "read the validation response",
                source: std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
            });
        }
        response.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&response[..header_end]).into_owned();
    let content_length =
        response_content_length(&head).ok_or_else(|| ProbeError::UnexpectedStatus {
            status_line: "<missing or invalid content-length>".to_owned(),
        })?;
    if content_length > RESPONSE_BODY_LIMIT {
        return Err(ProbeError::UnexpectedStatus {
            status_line: "<response body too large>".to_owned(),
        });
    }
    let body_already_read = response.len() - header_end;
    if body_already_read > content_length {
        return Err(ProbeError::UnexpectedStatus {
            status_line: "<response body exceeds content-length>".to_owned(),
        });
    }
    if body_already_read < content_length {
        let mut remaining = content_length - body_already_read;
        let mut buffer = [0_u8; 512];
        while remaining > 0 {
            let chunk_len = remaining.min(buffer.len());
            configure_stream(stream, deadline)?;
            let read = stream
                .read(&mut buffer[..chunk_len])
                .map_err(|source| ProbeError::Io {
                    operation: "read the validation response body",
                    source,
                })?;
            if read == 0 {
                return Err(ProbeError::Io {
                    operation: "read the validation response body",
                    source: std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
                });
            }
            remaining -= read;
        }
    }
    Ok(head)
}

/// Parses a decimal Content-Length from a response head.
fn response_content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

/// Parses the three-digit response status.
fn response_status(head: &str) -> Option<u16> {
    head.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
}

/// Polls `GET {base_url}/health` until it answers 200 or `timeout`
/// elapses.
///
/// # Errors
/// Returns [`HealthError::NotHttp`] when `base_url` is not an
/// `http://host:port` URL, and [`HealthError::Timeout`] when the endpoint
/// does not answer 200 within `timeout`.
///
/// # Examples
/// ```no_run
/// # use std::time::Duration;
/// shared_sidecar::wait_for_health("http://127.0.0.1:8081", Duration::from_secs(5))?;
/// # Ok::<(), shared_sidecar::HealthError>(())
/// ```
pub fn wait_for_health(base_url: &str, timeout: Duration) -> Result<(), HealthError> {
    wait_for_health_cancellable_with(
        base_url,
        timeout,
        &CancellationToken::new(),
        ATTEMPT_TIMEOUT,
        probe_health_until,
    )
}

/// Polls `GET {base_url}/health` until it answers 200, the timeout elapses,
/// or the caller cancels the wait.
///
/// # Errors
/// Returns [`HealthError::Cancelled`] when `cancellation` is signalled, plus
/// the URL and timeout failures documented by [`wait_for_health`].
pub fn wait_for_health_cancellable(
    base_url: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<(), HealthError> {
    wait_for_health_cancellable_with(
        base_url,
        timeout,
        cancellation,
        CANCELLABLE_ATTEMPT_TIMEOUT,
        probe_health_until,
    )
}

fn wait_for_health_cancellable_with(
    base_url: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
    mut probe: impl FnMut(&str, Instant) -> Result<(), ProbeError>,
) -> Result<(), HealthError> {
    wait_for_health_cancellable_with_start(
        base_url,
        timeout,
        cancellation,
        attempt_timeout,
        || {},
        |address, deadline| probe(address, deadline),
    )
}

fn wait_for_health_cancellable_with_start(
    base_url: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
    mut before_probe: impl FnMut(),
    mut probe: impl FnMut(&str, Instant) -> Result<(), ProbeError>,
) -> Result<(), HealthError> {
    let address = base_url
        .strip_prefix("http://")
        .ok_or_else(|| HealthError::NotHttp {
            url: base_url.to_owned(),
        })?;
    let deadline = Instant::now() + timeout;
    loop {
        let attempt_deadline = (Instant::now() + attempt_timeout).min(deadline);
        before_probe();
        let Some(attempt) = cancellation.run_if_active(|| probe(address, attempt_deadline)) else {
            return Err(HealthError::Cancelled);
        };
        match attempt {
            Ok(()) => return Ok(()),
            Err(error) => {
                if cancellation.is_cancelled() {
                    return Err(HealthError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(HealthError::Timeout {
                        url: base_url.to_owned(),
                        timeout,
                        source: error,
                    });
                }
                if cancellation.wait_timeout(RETRY_INTERVAL) {
                    return Err(HealthError::Cancelled);
                }
            }
        }
    }
}

fn probe_health_until(address: &str, deadline: Instant) -> Result<(), ProbeError> {
    let head = request_head_until(address, "GET", "/health", None, deadline)?;
    let status_ok = head
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200");
    if status_ok {
        return Ok(());
    }
    Err(ProbeError::UnexpectedStatus {
        status_line: head.lines().next().unwrap_or("<empty>").to_owned(),
    })
}

/// Issues one `GET {path}` presenting `api_key` as the bearer token and
/// classifies the answer.
#[cfg(test)]
pub(crate) fn probe_bearer(address: &str, path: &str, api_key: &str) -> KeyProbe {
    match request_head(address, "GET", path, Some(api_key)) {
        Ok(head) => {
            let accepted = head
                .split_whitespace()
                .nth(1)
                .and_then(|code| code.parse::<u16>().ok())
                .is_some_and(|code| (200..300).contains(&code));
            if accepted {
                KeyProbe::Accepted
            } else {
                KeyProbe::Rejected
            }
        }
        Err(_) => KeyProbe::Unreachable,
    }
}

/// One HTTP/1.0 request returning the response head. The `Host` header
/// carries the bound address, never `localhost`: the gateway's loopback
/// `Host` allowlist refuses anything else.
pub(crate) fn request_head(
    address: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
) -> Result<String, ProbeError> {
    request_head_with_timeout(address, method, path, bearer, ATTEMPT_TIMEOUT)
}

fn request_head_with_timeout(
    address: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    timeout: Duration,
) -> Result<String, ProbeError> {
    request_head_until(address, method, path, bearer, Instant::now() + timeout)
}

fn request_head_until(
    address: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    deadline: Instant,
) -> Result<String, ProbeError> {
    let socket = address
        .parse::<SocketAddr>()
        .map_err(|source| ProbeError::Io {
            operation: "parse the loopback address",
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        })?;
    let mut stream = connect_until(&socket, deadline)?;
    let mut request = format!("{method} {path} HTTP/1.0\r\nHost: {address}\r\n");
    if let Some(key) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(key);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    configure_stream(&stream, deadline)?;
    stream
        .write_all(request.as_bytes())
        .map_err(|source| ProbeError::Io {
            operation: "write the request",
            source,
        })?;
    let mut buffer = [0u8; 256];
    configure_stream(&stream, deadline)?;
    let read = stream.read(&mut buffer).map_err(|source| ProbeError::Io {
        operation: "read the status line",
        source,
    })?;
    Ok(String::from_utf8_lossy(&buffer[..read]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

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
    fn connection_proof_cannot_mix_health_and_bearer_across_endpoints() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind replacement fixture");
        let address = listener
            .local_addr()
            .expect("replacement fixture address")
            .to_string();
        let (accepted, received) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut health, _) = listener.accept().expect("accept health probe");
            let mut buffer = [0_u8; 1024];
            let read = health.read(&mut buffer).expect("read health probe");
            assert!(
                String::from_utf8_lossy(&buffer[..read]).starts_with("GET /health "),
                "the first endpoint receives health"
            );
            health
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .expect("answer health");
            drop(health);

            listener
                .set_nonblocking(true)
                .expect("make replacement observation bounded");
            let deadline = Instant::now() + Duration::from_millis(500);
            let mut connection_count = 1;
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut bearer, _)) => {
                        connection_count += 1;
                        let read = bearer.read(&mut buffer).expect("read bearer probe");
                        assert!(
                            String::from_utf8_lossy(&buffer[..read])
                                .contains("Authorization: Bearer accepted\r\n"),
                            "the replacement endpoint accepts the bearer"
                        );
                        bearer
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                            .expect("answer bearer");
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("observe replacement connection: {error}"),
                }
            }
            accepted
                .send(connection_count)
                .expect("report accepted connections");
        });

        assert_eq!(
            probe_connection(
                &address,
                "/v1/models",
                "accepted",
                Duration::from_millis(250)
            ),
            ConnectionProbe::HealthFailed,
            "one capability cannot combine health from one socket with authority from another"
        );
        assert_eq!(
            received
                .recv_timeout(Duration::from_secs(2))
                .expect("fixture reports its connection count"),
            1,
            "validation never reconnects after health succeeds"
        );
    }

    #[test]
    fn a_real_connect_attempt_obeys_one_absolute_deadline() {
        let started = Instant::now();
        let result = probe_connection_once(
            "192.0.2.1:9",
            "/v1/models",
            "key",
            Instant::now() + Duration::from_millis(100),
        );

        assert_eq!(result, ConnectionAttempt::HealthFailed);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "connect_timeout bounds a stalled or unreachable route"
        );
    }

    #[test]
    fn slow_drip_response_head_cannot_refresh_the_attempt_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow-head fixture");
        let address = listener
            .local_addr()
            .expect("slow-head address")
            .to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept slow-head probe");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
                if stream.write_all(&[*byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let started = Instant::now();
        let result = probe_connection_once(
            &address,
            "/v1/models",
            "key",
            Instant::now() + CANCELLABLE_ATTEMPT_TIMEOUT,
        );

        assert_eq!(result, ConnectionAttempt::HealthFailed);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "one absolute deadline bounds a slow-drip response head"
        );
    }

    #[test]
    fn slow_drip_response_body_cannot_refresh_the_attempt_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow-body fixture");
        let address = listener
            .local_addr()
            .expect("slow-body address")
            .to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept slow-body probe");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            if stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n")
                .is_err()
            {
                return;
            }
            for byte in b"0123456789abcdef" {
                if stream.write_all(&[*byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let started = Instant::now();
        let result = probe_connection_once(
            &address,
            "/v1/models",
            "key",
            Instant::now() + CANCELLABLE_ATTEMPT_TIMEOUT,
        );

        assert_eq!(result, ConnectionAttempt::HealthFailed);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "one absolute deadline bounds a slow-drip response body"
        );
    }

    #[test]
    fn an_unbounded_declared_body_is_rejected_without_draining() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            RESPONSE_BODY_LIMIT + 1
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind oversized-body fixture");
        let address = listener
            .local_addr()
            .expect("oversized-body address")
            .to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept oversized-body probe");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response.as_bytes());
            std::thread::sleep(Duration::from_secs(1));
        });

        let started = Instant::now();
        let result = probe_connection_once(
            &address,
            "/v1/models",
            "key",
            Instant::now() + CANCELLABLE_ATTEMPT_TIMEOUT,
        );

        assert_eq!(result, ConnectionAttempt::HealthFailed);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "an oversized declared body is rejected before its bytes arrive"
        );
    }

    #[test]
    fn cancellation_joins_a_real_slow_drip_probe_without_retrying() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind cancelled-drip fixture");
        let address = listener
            .local_addr()
            .expect("cancelled-drip address")
            .to_string();
        let connections = Arc::new(AtomicUsize::new(0));
        let server_connections = Arc::clone(&connections);
        let (entered, blocked) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept cancelled-drip probe");
            server_connections.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            entered.send(()).expect("announce real slow-drip probe");
            for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
                if stream.write_all(&[*byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker = std::thread::spawn(move || {
            probe_connection_cancellable(
                &address,
                "/v1/models",
                "key",
                Duration::from_secs(30),
                &worker_cancellation,
            )
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the real probe begins its slow response");

        let started = Instant::now();
        cancellation.cancel();
        let result = worker.join().expect("the real probe worker joins");

        assert_eq!(result, ConnectionProbe::Cancelled);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the attempt deadline bounds joined cancellation during real I/O"
        );
        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "cancellation starts no later network probe"
        );
    }

    #[test]
    fn cancellation_at_validation_probe_start_prevents_the_probe() {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let probe_cancellation = worker_cancellation.clone();
        let probes = Arc::new(AtomicUsize::new(0));
        let worker_probes = Arc::clone(&probes);
        let (entered, blocked) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            probe_connection_with_probe(
                "127.0.0.1:1",
                "/v1/models",
                "key",
                Duration::from_secs(30),
                &worker_cancellation,
                CANCELLABLE_ATTEMPT_TIMEOUT,
                |_, _, _, _| {
                    entered
                        .send(())
                        .expect("announce validation probe boundary");
                    if probe_cancellation.wait_timeout(Duration::from_secs(30)) {
                        ConnectionAttempt::HealthFailed
                    } else {
                        worker_probes.fetch_add(1, Ordering::SeqCst);
                        ConnectionAttempt::Accepted
                    }
                },
            )
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("validation pauses immediately before probe admission");

        cancellation.cancel();
        let result = worker.join().expect("validation worker joins");

        assert_eq!(result, ConnectionProbe::Cancelled);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            0,
            "no validation probe starts after cancellation returns"
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
}
