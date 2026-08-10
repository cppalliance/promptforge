//! Authenticated `/v1/models` readiness probing with a hard body cap.
//!
//! A loopback listener is outside this harness's trust boundary until it proves
//! it owns our bearer token and exposes the expected model. The probe returns a
//! typed [`Readiness`] so the startup loop can keep waiting on transient
//! conditions but stop immediately on a terminal HTTP, decode, or size failure
//! instead of retrying it for the full readiness deadline.

use std::io::Read;
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde_json::Value;

use super::LOOPBACK;

/// Hard cap on the `/v1/models` body read during readiness. A wrong or hostile
/// listener cannot force unbounded memory growth: the request timeout bounds
/// elapsed time, not response size, so the byte cap does.
pub(crate) const MODELS_BODY_CAP: usize = 64 * 1024;

/// The outcome of one authenticated readiness probe.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    /// The listener owns our token and exposes the expected model.
    Ready,
    /// A transient condition (connection refused, model still loading); retry.
    Pending,
    /// A terminal failure that retrying the same key and profile cannot fix.
    Terminal(String),
}

/// Builds the short-timeout, no-proxy blocking client the readiness loop reuses.
pub(crate) fn build_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .context("build promptforge-gateway readiness client")
}

/// Probes `/health` then authenticated `/v1/models` on the loopback `port`.
pub(crate) fn probe(
    client: &reqwest::blocking::Client,
    port: u16,
    api_key: &str,
    model_name: &str,
) -> Readiness {
    let base = format!("http://{LOOPBACK}:{port}");
    let Ok(health) = client.get(format!("{base}/health")).send() else {
        return Readiness::Pending;
    };
    if !health.status().is_success() {
        return Readiness::Pending;
    }
    let Ok(models) = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
    else {
        return Readiness::Pending;
    };
    let success = models.status().is_success();
    let status = models.status();
    match read_capped(models, MODELS_BODY_CAP) {
        Ok(bytes) => classify_models(success, status.as_u16(), &bytes, model_name),
        Err(CapError::TooLarge(len)) => Readiness::Terminal(format!(
            "/v1/models body exceeded the {MODELS_BODY_CAP}-byte readiness cap (saw at least {len} bytes)"
        )),
        Err(CapError::Io(error)) => {
            Readiness::Terminal(format!("/v1/models body read failed: {error}"))
        }
    }
}

/// Failure reading a bounded body.
#[derive(Debug)]
enum CapError {
    /// The body was larger than the cap (carries the observed length).
    TooLarge(usize),
    /// The body could not be read to completion.
    Io(String),
}

/// Reads at most `cap` bytes from `reader`, rejecting an over-cap body.
///
/// Reads one byte past the cap so an exactly-`cap` body is accepted while any
/// larger body is deterministically rejected without buffering all of it.
fn read_capped<R: Read>(reader: R, cap: usize) -> Result<Vec<u8>, CapError> {
    let mut buffer = Vec::new();
    reader
        .take(cap as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|error| CapError::Io(error.to_string()))?;
    if buffer.len() > cap {
        return Err(CapError::TooLarge(buffer.len()));
    }
    Ok(buffer)
}

/// Classifies a bounded `/v1/models` response body.
fn classify_models(success: bool, status: u16, bytes: &[u8], model_name: &str) -> Readiness {
    if !success {
        let excerpt = String::from_utf8_lossy(bytes);
        return Readiness::Terminal(format!("/v1/models returned HTTP {status}: {excerpt}"));
    }
    let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
        return Readiness::Terminal(
            "/v1/models returned a 200 body that is not valid JSON".to_owned(),
        );
    };
    if models_list_contains(&body, model_name) {
        Readiness::Ready
    } else {
        // A well-formed list that does not yet name our model: the gateway may
        // still be loading it, so keep waiting rather than fail.
        Readiness::Pending
    }
}

/// Whether a `/v1/models` body names `model_name` in its `data` array.
fn models_list_contains(body: &Value, model_name: &str) -> bool {
    body.get("data")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("id").and_then(Value::as_str) == Some(model_name))
        })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Cursor, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread::{self, JoinHandle};

    use serde_json::json;

    use super::*;

    #[test]
    fn a_body_exactly_at_the_cap_is_accepted() {
        let body = vec![b'x'; 8];
        let read = read_capped(Cursor::new(body.clone()), 8).expect("exact-cap body must be read");
        assert_eq!(read, body);
    }

    #[test]
    fn a_body_one_byte_over_the_cap_is_rejected() {
        let body = vec![b'x'; 9];
        let error = read_capped(Cursor::new(body), 8).expect_err("over-cap body must be rejected");
        assert!(
            matches!(error, CapError::TooLarge(len) if len == 9),
            "{error:?}"
        );
    }

    #[test]
    fn a_present_model_is_ready() {
        let body = json!({"data": [{"id": "qwen3-0.6b"}]}).to_string();
        assert_eq!(
            classify_models(true, 200, body.as_bytes(), "qwen3-0.6b"),
            Readiness::Ready
        );
    }

    #[test]
    fn a_missing_model_in_a_valid_list_stays_pending() {
        let body = json!({"data": [{"id": "other"}]}).to_string();
        assert_eq!(
            classify_models(true, 200, body.as_bytes(), "qwen3-0.6b"),
            Readiness::Pending
        );
    }

    #[test]
    fn a_rejected_key_is_terminal_not_a_retry() {
        let Readiness::Terminal(message) =
            classify_models(false, 401, b"unauthorized", "qwen3-0.6b")
        else {
            panic!("a non-success models status must be terminal");
        };
        assert!(
            message.contains("401") && message.contains("unauthorized"),
            "{message}"
        );
    }

    #[test]
    fn a_malformed_success_body_is_terminal() {
        let Readiness::Terminal(message) = classify_models(true, 200, b"not json", "qwen3-0.6b")
        else {
            panic!("a malformed 200 body must be terminal");
        };
        assert!(message.contains("not valid JSON"), "{message}");
    }

    /// What a [`MockGateway`] does with each accepted connection.
    enum Behavior {
        /// Reply `200 ok` to `/health` and a fixed status + body to `/v1/models`.
        Respond { status: u16, body: Vec<u8> },
        /// Accept then immediately close each connection without responding, so
        /// the probe cannot obtain any reply and must report `Pending`. The
        /// listener owns the port for the whole test, so this is race-free where
        /// a released "free" port would collide with a concurrent test's bind.
        Reset,
        /// Reply `200 ok` to `/health`; return an empty model list on the first
        /// `/v1/models` request, then list `model`, exercising the
        /// pending-then-ready lifecycle across probe rounds.
        ModelLoads { model: String },
    }

    /// A minimal loopback HTTP server driven by [`Behavior`], so probe tests
    /// never launch a real gateway. It owns its ephemeral port for its lifetime.
    struct MockGateway {
        port: u16,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl MockGateway {
        fn start(behavior: Behavior) -> Self {
            // A blocking listener: no accept-poll sleep. It owns the ephemeral
            // port for the whole test, so no other bind can reclaim it.
            let listener = TcpListener::bind((LOOPBACK, 0)).expect("mock gateway must bind");
            let port = listener.local_addr().expect("mock addr").port();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = Arc::clone(&stop);
            let handle = thread::spawn(move || serve(&listener, &stop_thread, &behavior));
            Self {
                port,
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for MockGateway {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            // Wake the blocking accept with a throwaway connection so the serve
            // thread observes `stop` and exits; then join deterministically.
            let _ignored = TcpStream::connect((LOOPBACK, self.port));
            if let Some(handle) = self.handle.take() {
                let _ignored = handle.join();
            }
        }
    }

    fn serve(listener: &TcpListener, stop: &AtomicBool, behavior: &Behavior) {
        let models_requests = AtomicUsize::new(0);
        for stream in listener.incoming() {
            // Check `stop` before touching the stream so the Drop wake-up
            // connection (which sends nothing) is never read from.
            if stop.load(Ordering::Acquire) {
                break;
            }
            let Ok(mut stream) = stream else { break };
            if matches!(behavior, Behavior::Reset) {
                // Close the connection without reading or responding.
                drop(stream);
                continue;
            }
            // Read the full request line deterministically. The accepted stream
            // is blocking (the listener was never set non-blocking), so this
            // never returns a partial/empty line that could misroute the request
            // - the root cause of the earlier auth-test flakiness on Windows,
            // where an inherited non-blocking accept made `read` return
            // `WouldBlock` and the health request was answered as `/v1/models`.
            let request_line = {
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                let _ignored = reader.read_line(&mut line);
                line
            };
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (status, body): (u16, Vec<u8>) = if path.starts_with("/health") {
                (200, b"ok".to_vec())
            } else {
                match behavior {
                    Behavior::Respond { status, body } => (*status, body.clone()),
                    Behavior::ModelLoads { model } => {
                        // Scripted transition by request count, not wall-clock:
                        // the first models probe is pending, the next is ready.
                        if models_requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            (200, br#"{"data":[]}"#.to_vec())
                        } else {
                            (
                                200,
                                json!({"data": [{"id": model}]}).to_string().into_bytes(),
                            )
                        }
                    }
                    Behavior::Reset => continue,
                }
            };
            let header = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ignored = stream.write_all(header.as_bytes());
            let _ignored = stream.write_all(&body);
            let _ignored = stream.flush();
        }
    }

    #[test]
    fn probe_reports_ready_when_the_owned_model_is_listed() {
        let body = json!({"data": [{"id": "qwen3-0.6b"}]}).to_string();
        let server = MockGateway::start(Behavior::Respond {
            status: 200,
            body: body.into_bytes(),
        });
        let client = build_client(Duration::from_secs(2)).expect("client");
        assert_eq!(
            probe(&client, server.port, "secret", "qwen3-0.6b"),
            Readiness::Ready
        );
    }

    #[test]
    fn probe_reports_terminal_on_an_authentication_rejection() {
        let server = MockGateway::start(Behavior::Respond {
            status: 401,
            body: b"nope".to_vec(),
        });
        let client = build_client(Duration::from_secs(2)).expect("client");
        let Readiness::Terminal(message) = probe(&client, server.port, "secret", "qwen3-0.6b")
        else {
            panic!("a 401 on /v1/models must be terminal, not a retry");
        };
        assert!(
            message.contains("401") && message.contains("nope"),
            "the terminal diagnostic must carry the status and the bounded body excerpt: {message}"
        );
    }

    #[test]
    fn probe_stays_pending_against_an_unresponsive_listener() {
        // A listener that owns the port but never answers: race-free (no
        // released-port collision) and returns Pending without a wall-clock wait.
        let server = MockGateway::start(Behavior::Reset);
        let client = build_client(Duration::from_secs(2)).expect("client");
        assert_eq!(
            probe(&client, server.port, "secret", "qwen3-0.6b"),
            Readiness::Pending
        );
    }

    #[test]
    fn probe_transitions_from_pending_to_ready_as_the_model_loads() {
        let server = MockGateway::start(Behavior::ModelLoads {
            model: "qwen3-0.6b".to_owned(),
        });
        let client = build_client(Duration::from_secs(2)).expect("client");
        assert_eq!(
            probe(&client, server.port, "secret", "qwen3-0.6b"),
            Readiness::Pending,
            "an empty model list must keep the probe pending"
        );
        assert_eq!(
            probe(&client, server.port, "secret", "qwen3-0.6b"),
            Readiness::Ready,
            "the probe must report ready once the model appears"
        );
    }

    #[test]
    fn probe_rejects_an_oversized_models_body() {
        let mut body = String::from("{\"data\":[");
        body.push_str(&"\"padding\",".repeat(MODELS_BODY_CAP / 4));
        body.push_str("\"end\"]}");
        let server = MockGateway::start(Behavior::Respond {
            status: 200,
            body: body.into_bytes(),
        });
        let client = build_client(Duration::from_secs(2)).expect("client");
        let outcome = probe(&client, server.port, "secret", "qwen3-0.6b");
        assert!(matches!(outcome, Readiness::Terminal(message) if message.contains("cap")));
    }
}
