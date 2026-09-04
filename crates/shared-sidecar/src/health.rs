//! Readiness and key probes: raw loopback HTTP/1.0 over
//! `std::net::TcpStream`, enough to read a status line, with no HTTP
//! client dependency.
//!
//! Moved from the workshop shell's `health.rs`; the only change in the
//! move is the `Host` header, which now carries the bound loopback
//! address instead of `localhost`, matching the gateway's loopback `Host`
//! allowlist.

use std::io::{Read, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Delay between probes while the server comes up.
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Per-attempt connect and read timeout, so one hung attempt cannot eat
/// the whole budget.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

/// A failure of [`wait_for_health`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HealthError {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyProbe {
    /// A 2xx answer: the key is accepted.
    Accepted,
    /// A non-2xx answer: the key is rejected (or the route is gone).
    Rejected,
    /// The server could not be queried at all.
    Unreachable,
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
    let address = base_url
        .strip_prefix("http://")
        .ok_or_else(|| HealthError::NotHttp {
            url: base_url.to_owned(),
        })?;
    let deadline = Instant::now() + timeout;
    loop {
        match probe_health(address) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(HealthError::Timeout {
                        url: base_url.to_owned(),
                        timeout,
                        source: error,
                    });
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}

/// Issues one `GET /health` and requires a 200 status line.
pub(crate) fn probe_health(address: &str) -> Result<(), ProbeError> {
    let head = request_head(address, "GET", "/health", None)?;
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
    let mut stream = TcpStream::connect(address).map_err(|source| ProbeError::Io {
        operation: "connect",
        source,
    })?;
    stream
        .set_read_timeout(Some(ATTEMPT_TIMEOUT))
        .map_err(|source| ProbeError::Io {
            operation: "configure the read timeout",
            source,
        })?;
    stream
        .set_write_timeout(Some(ATTEMPT_TIMEOUT))
        .map_err(|source| ProbeError::Io {
            operation: "configure the write timeout",
            source,
        })?;
    let mut request = format!("{method} {path} HTTP/1.0\r\nHost: {address}\r\n");
    if let Some(key) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(key);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|source| ProbeError::Io {
            operation: "write the request",
            source,
        })?;
    let mut buffer = [0u8; 256];
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
    use std::sync::mpsc;

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
}
