//! The one-connection health and bearer proof.

use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::{
    ATTEMPT_TIMEOUT, CANCELLABLE_ATTEMPT_TIMEOUT, ConnectionProbe, ProbeError, RESPONSE_BODY_LIMIT,
    RESPONSE_HEAD_LIMIT, RETRY_INTERVAL, configure_stream, connect_until,
};
use crate::CancellationToken;

/// Proves that one address is healthy and accepts the presented bearer.
/// Both requests use one TCP connection, so authority cannot come from a
/// listener that replaced the endpoint after the health response.
#[cfg(test)]
pub(crate) fn probe_connection(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    health_budget: Duration,
) -> ConnectionProbe {
    probe_connection_until(address, bearer_path, bearer, Instant::now() + health_budget)
}

/// Proves one connection within the caller's absolute deadline.
pub(crate) fn probe_connection_until(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    deadline: Instant,
) -> ConnectionProbe {
    probe_connection_with(
        address,
        bearer_path,
        bearer,
        deadline,
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
        Instant::now() + health_budget,
        cancellation,
        CANCELLABLE_ATTEMPT_TIMEOUT,
    )
}

fn probe_connection_with(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    deadline: Instant,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
) -> ConnectionProbe {
    probe_connection_with_probe(
        address,
        bearer_path,
        bearer,
        deadline,
        cancellation,
        attempt_timeout,
        probe_connection_once,
    )
}

pub(super) fn probe_connection_with_probe(
    address: &str,
    bearer_path: &str,
    bearer: &str,
    deadline: Instant,
    cancellation: &CancellationToken,
    attempt_timeout: Duration,
    mut probe: impl FnMut(&str, &str, &str, Instant) -> ConnectionAttempt,
) -> ConnectionProbe {
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
pub(super) enum ConnectionAttempt {
    Accepted,
    HealthFailed,
    KeyRejected,
    ProofInterrupted,
}

/// Checks health and bearer acceptance over one socket. Once health has
/// succeeded, any socket loss fails the proof instead of reconnecting to a
/// potentially different endpoint.
pub(super) fn probe_connection_once(
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
