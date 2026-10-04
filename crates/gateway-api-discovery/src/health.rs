//! Readiness and key probes: raw loopback HTTP/1.x over
//! `std::net::TcpStream`, enough to read a status line, with no HTTP
//! client dependency.
//!
//! Moved from the workshop shell's `health.rs`; the only change in the
//! move is the `Host` header, which is now set to the bound loopback
//! address instead of `localhost`, matching the gateway's loopback `Host`
//! allowlist.

use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::CancellationToken;

mod connection;

#[cfg(test)]
use connection::probe_connection;
pub(crate) use connection::{probe_connection_cancellable, probe_connection_until};

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
enum KeyProbe {
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

/// Polls `GET {base_url}/health` until it answers 200 or `timeout`
/// elapses.
///
/// # Errors
/// Returns [`HealthError::NotHttp`] when `base_url` is not an
/// `http://host:port` URL, and [`HealthError::Timeout`] when the endpoint
/// does not answer 200 within `timeout`.
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
fn probe_bearer(address: &str, path: &str, api_key: &str) -> KeyProbe {
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
/// is set to the bound address, never `localhost`: the gateway's loopback
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

pub(crate) fn request_head_until(
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
mod tests;
