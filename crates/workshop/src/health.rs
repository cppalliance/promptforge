//! Readiness probe: poll the server's `/health` endpoint until it answers.
//!
//! The probe is a raw loopback HTTP/1.0 GET over `std::net::TcpStream` -
//! enough to read a status line, with no HTTP client dependency in the
//! shell.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::Context as _;

/// Delay between probes while the server comes up.
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Per-attempt connect and read timeout, so one hung attempt cannot eat
/// the whole budget.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Polls `GET {base_url}/health` until it answers 200 or `timeout` elapses.
///
/// # Errors
/// Returns an error when the endpoint does not answer 200 within `timeout`,
/// or when `base_url` is not an `http://host:port` URL.
pub(crate) fn wait_for_health(base_url: &str, timeout: Duration) -> anyhow::Result<()> {
    let address = base_url
        .strip_prefix("http://")
        .with_context(|| format!("health probe needs an http:// URL, got {base_url}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match probe(address) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(error).context(format!(
                        "the workshop server did not answer {base_url}/health within {timeout:?}"
                    ));
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}

/// Issues one `GET /health` and requires a 200 status line.
fn probe(address: &str) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(address).context("connect")?;
    stream.set_read_timeout(Some(ATTEMPT_TIMEOUT))?;
    stream.set_write_timeout(Some(ATTEMPT_TIMEOUT))?;
    stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .context("write the health request")?;
    let mut buffer = [0u8; 256];
    let read = stream.read(&mut buffer).context("read the status line")?;
    let head = String::from_utf8_lossy(&buffer[..read]);
    let status_ok = head
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200");
    anyhow::ensure!(
        status_ok,
        "unexpected health response: {}",
        head.lines().next().unwrap_or("<empty>")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::TcpListener;

    /// Answers every connection with a canned `200 OK` until the test
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
        let message = format!("{error:?}");
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
        let message = format!("{error:?}");
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
            format!("{error:?}").contains("http://"),
            "the error names the scheme requirement: {error:?}"
        );
    }
}
