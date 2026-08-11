//! Pure helpers for the `llama-server` guard: output capture, port selection,
//! per-attempt identity, readiness probing, and argument rendering.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use super::{API_KEY_REDACTION, AttemptIdentity, CAPTURE_LIMIT, LOOPBACK, LaunchOptions, Result};
use crate::local::error::LocalError;

#[derive(Debug)]
pub(super) struct BoundedCapture {
    bytes: VecDeque<u8>,
    dropped: usize,
    limit: usize,
}

impl BoundedCapture {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit),
            dropped: 0,
            limit,
        }
    }

    pub(super) fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        while self.bytes.len() > self.limit {
            self.bytes.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub(super) fn render(&self) -> String {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bytes);
        if self.dropped == 0 {
            text.into_owned()
        } else {
            format!("[{} earlier bytes omitted]\n{text}", self.dropped)
        }
    }
}

pub(super) type SharedCapture = Arc<Mutex<BoundedCapture>>;

pub(super) fn new_capture() -> SharedCapture {
    Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)))
}

pub(super) fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((LOOPBACK, 0)).map_err(|source| LocalError::Port {
        operation: "select free llama-server port",
        source,
    })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|source| LocalError::Port {
            operation: "read selected llama-server port",
            source,
        })
}

pub(super) fn random_identity() -> AttemptIdentity {
    // The loopback bearer token guards the local llama-server; use the OS-seeded
    // cryptographic RNG (`rand::rng`, a ChaCha-based CSPRNG) rather than a fast
    // non-cryptographic generator.
    use rand::Rng;
    let mut rng = rand::rng();
    let model_nonce = format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>());
    let key_nonce = format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>());
    AttemptIdentity {
        model_alias: format!("promptforge-local-{model_nonce}"),
        api_key: format!("promptforge-local-{key_nonce}"),
    }
}

pub(super) fn listener_is_present(port: u16, timeout: Duration) -> bool {
    let Ok(address) = format!("{LOOPBACK}:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&address, timeout).is_ok()
}

pub(super) fn readiness_belongs_to(
    client: &reqwest::blocking::Client,
    port: u16,
    api_key: &str,
    model_alias: &str,
) -> bool {
    let base = format!("http://{LOOPBACK}:{port}");
    let Ok(health) = client
        .get(format!("{base}/health"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !health.status().is_success() {
        return false;
    }
    let Ok(models) = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !models.status().is_success() {
        return false;
    }
    let Ok(body) = models.json::<Value>() else {
        return false;
    };
    body.get("data")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("id").and_then(Value::as_str) == Some(model_alias))
        })
}

/// Builds the full `llama-server` argument vector for one launch attempt.
pub(super) fn server_args(
    model: &Path,
    port: u16,
    model_alias: &str,
    api_key: &str,
    options: &LaunchOptions,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--model"),
        model.as_os_str().to_owned(),
        OsString::from("--alias"),
        OsString::from(model_alias),
        OsString::from("--api-key"),
        OsString::from(api_key),
        OsString::from("--host"),
        OsString::from(LOOPBACK),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--ctx-size"),
        OsString::from(options.ctx_size.to_string()),
        OsString::from("--n-predict"),
        OsString::from(options.n_predict.to_string()),
        OsString::from("--parallel"),
        OsString::from(options.parallel.to_string()),
        OsString::from("--cache-type-k"),
        OsString::from(&options.cache_type_k),
        OsString::from("--cache-type-v"),
        OsString::from(&options.cache_type_v),
        OsString::from("-ngl"),
        OsString::from(options.gpu_layers.to_string()),
        OsString::from("--jinja"),
    ];
    if let Some(template) = &options.chat_template_file {
        args.extend([
            OsString::from("--chat-template-file"),
            template.as_os_str().to_owned(),
        ]);
    }
    if options.flash_attention {
        args.extend([OsString::from("--flash-attn"), OsString::from("on")]);
    }
    if !options.think {
        args.extend([OsString::from("--reasoning"), OsString::from("off")]);
    }
    args.extend([OsString::from("--reasoning-format"), OsString::from("auto")]);
    let (temp, top_p) = if options.think {
        ("1.0", "0.95")
    } else {
        ("0.7", "0.8")
    };
    args.extend([
        OsString::from("--temp"),
        OsString::from(temp),
        OsString::from("--top-p"),
        OsString::from(top_p),
        OsString::from("--top-k"),
        OsString::from("20"),
        OsString::from("--presence-penalty"),
        OsString::from("1.5"),
    ]);
    args
}

pub(super) fn display_invocation(executable: &Path, args: &[OsString]) -> String {
    let mut pieces = Vec::with_capacity(args.len() + 1);
    pieces.push(executable.display().to_string());
    let mut redact_next = false;
    for argument in args {
        if redact_next {
            pieces.push(API_KEY_REDACTION.to_owned());
            redact_next = false;
        } else {
            let rendered = argument.to_string_lossy().into_owned();
            redact_next = rendered == "--api-key";
            pieces.push(rendered);
        }
    }
    pieces.join(" ")
}

pub(super) fn capture_reader<R>(
    name: &'static str,
    mut reader: R,
    capture: SharedCapture,
) -> Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => capture
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append(&buffer[..count]),
                }
            }
        })
        .map_err(|source| LocalError::CaptureThread {
            stream: name,
            source,
        })
}
