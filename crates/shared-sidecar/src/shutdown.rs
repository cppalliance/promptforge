//! The `POST /shutdown` request: how a reader asks the gateway to exit.
//!
//! The workshop shell's quit-everything menu item is the caller: it posts
//! the current validated connection's bearer key to the gateway's shutdown
//! route, which answers `202 Accepted` and drains. Raw HTTP/1.0 over
//! `TcpStream` like the health probe, matching the crate's dependency diet -
//! no HTTP client.

use crate::health::{self, ProbeError};
use crate::{GatewayDiscoveryFile, ValidatedConnection};
use std::time::Instant;

/// The shutdown route's path on the gateway.
const SHUTDOWN_PATH: &str = "/shutdown";

/// A failure of [`request_shutdown`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShutdownError {
    /// The request could not be delivered or the answer read.
    #[error("{operation}")]
    Io {
        /// The failed operation.
        operation: &'static str,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The gateway answered with a non-2xx status: the key was rejected
    /// or the route is gone.
    #[error("unexpected shutdown response: {status_line}")]
    Rejected {
        /// The response's first line.
        status_line: String,
    },
}

impl From<ProbeError> for ShutdownError {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::Io { operation, source } => Self::Io { operation, source },
            ProbeError::UnexpectedStatus { status_line } => Self::Rejected { status_line },
        }
    }
}

/// Posts `POST /shutdown` to the validated local Gateway, presenting its
/// bearer key. A 2xx answer means the Gateway accepted and is draining;
/// the caller exits without waiting for the process to die.
///
/// # Errors
/// Returns [`ShutdownError::Io`] when the gateway cannot be connected or
/// its answer cannot be read, and [`ShutdownError::Rejected`] when the
/// answer's status is not 2xx.
///
/// # Examples
/// ```no_run
/// # let file = shared_sidecar::GatewayDiscoveryFile {
/// #     port: 8081,
/// #     api_key: "secret".into(),
/// #     pid: 42,
/// #     epoch: 1,
/// #     version: "0.2.0".into(),
/// #     started_at: "2026-09-07T00:00:00Z".into(),
/// # };
/// let connection = shared_sidecar::ValidatedConnection::validate(file)?;
/// shared_sidecar::request_shutdown(&connection)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn request_shutdown(connection: &ValidatedConnection) -> Result<(), ShutdownError> {
    request_shutdown_file(connection.gateway_discovery_file())
}

/// Posts authenticated shutdown without allowing network I/O to outlive the
/// caller's absolute deadline.
///
/// # Errors
/// Returns the same delivery and rejection errors as [`request_shutdown`].
pub fn request_shutdown_before(
    connection: &ValidatedConnection,
    deadline: Instant,
) -> Result<(), ShutdownError> {
    request_shutdown_file_before(connection.gateway_discovery_file(), deadline)
}

fn request_shutdown_file(file: &GatewayDiscoveryFile) -> Result<(), ShutdownError> {
    let address = format!("127.0.0.1:{}", file.port);
    let head = health::request_head(&address, "POST", SHUTDOWN_PATH, Some(&file.api_key))?;
    accepted(&head)
}

fn request_shutdown_file_before(
    file: &GatewayDiscoveryFile,
    deadline: Instant,
) -> Result<(), ShutdownError> {
    let address = format!("127.0.0.1:{}", file.port);
    let head = health::request_head_until(
        &address,
        "POST",
        SHUTDOWN_PATH,
        Some(&file.api_key),
        deadline,
    )?;
    accepted(&head)
}

fn accepted(head: &str) -> Result<(), ShutdownError> {
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok());
    if status.is_some_and(|code| (200..300).contains(&code)) {
        return Ok(());
    }
    Err(ShutdownError::Rejected {
        status_line: status.map_or_else(|| "<malformed>".to_owned(), |code| code.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A gateway discovery file naming the fixture gateway's port.
    fn file(port: u16) -> GatewayDiscoveryFile {
        GatewayDiscoveryFile {
            port,
            api_key: "key".to_owned(),
            pid: 4242,
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    /// A fixture gateway recording each request and answering with the
    /// canned response.
    fn fixture_gateway(response: &'static [u8]) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        let (requests, received) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                if let Ok(read) = stream.read(&mut buffer) {
                    let _ = requests.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
                }
                let _ = stream.write_all(response);
            }
        });
        (port, received)
    }

    #[test]
    fn an_accepted_shutdown_posts_the_route_with_the_files_key() {
        let (port, received) =
            fixture_gateway(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n");
        request_shutdown_file(&file(port)).expect("the gateway accepted");
        let request = received
            .recv_timeout(Duration::from_secs(5))
            .expect("the request arrived");
        assert!(
            request.starts_with("POST /shutdown HTTP/1.0\r\n"),
            "the request is a POST to the shutdown route: {request:?}"
        );
        assert!(
            request.contains("Authorization: Bearer key\r\n"),
            "the file's key is the bearer token: {request:?}"
        );
        assert!(
            request.contains(&format!("Host: 127.0.0.1:{port}\r\n")),
            "the Host header is the bound address, not localhost: {request:?}"
        );
    }

    #[test]
    fn a_refused_shutdown_is_rejected() {
        let (port, _received) =
            fixture_gateway(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        let error = request_shutdown_file(&file(port)).expect_err("a 401 is a refusal");
        assert!(
            matches!(error, ShutdownError::Rejected { .. }),
            "a non-2xx answer is a rejection: {error}"
        );
    }

    #[test]
    fn a_reflected_bearer_is_absent_from_shutdown_errors() {
        let secret = "capability-secret";
        let response = format!("HTTP/1.1 401 rejected-{secret}\r\nContent-Length: 0\r\n\r\n");
        let response: &'static [u8] = Box::leak(response.into_bytes().into_boxed_slice());
        let (port, _received) = fixture_gateway(response);
        let mut connection = file(port);
        connection.api_key = secret.to_owned();

        let error = request_shutdown_file(&connection).expect_err("a 401 is a refusal");
        assert!(
            !format!("{error:?} {error}").contains(secret),
            "bearer values never enter error diagnostics"
        );
    }

    #[test]
    fn a_dead_gateway_is_an_io_error() {
        // Port 1 is never listening, so the connect fails fast.
        let error = request_shutdown_file(&file(1)).expect_err("a dead port cannot answer");
        assert!(
            matches!(error, ShutdownError::Io { .. }),
            "an undelivered request is an I/O error: {error}"
        );
    }

    #[test]
    fn an_absolute_deadline_bounds_an_unresponsive_shutdown_target() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow shutdown fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept shutdown");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            std::thread::sleep(Duration::from_secs(1));
        });
        let started = std::time::Instant::now();

        let error = request_shutdown_file_before(&file(port), started + Duration::from_millis(100))
            .expect_err("an unresponsive child exceeds the cleanup deadline");

        assert!(matches!(error, ShutdownError::Io { .. }));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "late-child shutdown uses one absolute network deadline"
        );
    }
}
