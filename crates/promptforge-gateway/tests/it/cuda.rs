//! Live CUDA proof: an opt-in end-to-end run of the embedded CUDA
//! `llama-server` bundle with an MTP drafter and a multimodal projector on
//! real hardware.
//!
//! The test is `#[ignore]`d and additionally opt-in: even when forced with
//! `--ignored`, it prints a skip notice and returns `Ok` unless
//! `PROMPTFORGE_LIVE_CUDA=1` is set. Run it with:
//!
//! ```text
//! cargo test -p promptforge-gateway --features llama-cuda -- --ignored live_cuda
//! ```

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::support::TestServer;

/// Opt-in gate: the run downloads three multi-gigabyte GGUF artifacts and
/// loads them onto a real GPU.
const LIVE_ENV: &str = "PROMPTFORGE_LIVE_CUDA";

const MAIN_URL: &str = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-UD-Q4_K_XL.gguf";
const MAIN_SHA256: &str = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32";
const DRAFT_URL: &str =
    "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mtp-gemma-4-E2B-it.gguf";
const DRAFT_SHA256: &str = "9eba819938efccfd6044f8af84e3bbfddc639a2bcf32ebc36420e6a649191919";
const PROJECTOR_URL: &str =
    "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf";
const PROJECTOR_SHA256: &str = "140be8d7849741f88c50757d529b84373ee8e27052cc2236855b537f4a8215fa";

/// First provisioning downloads the three pinned artifacts.
const PROVISION_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// A marker-hit relaunch skips downloads and re-hashing; only spawn and
/// weight load remain.
const RELAUNCH_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// One completion against a warm server.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Bound on waiting for the capture readers to drain the child's startup
/// log: readiness is an HTTP probe, so it can beat the final piped bytes.
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(30);

/// Serializes live CUDA runs: two concurrent runs would both load
/// multi-gigabyte weights onto one GPU.
static LIVE_CUDA: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One pinned artifact of the live catalog entry.
struct PinnedArtifact {
    url: &'static str,
    sha256: &'static str,
    filename: &'static str,
}

const ARTIFACTS: [PinnedArtifact; 3] = [
    PinnedArtifact {
        url: MAIN_URL,
        sha256: MAIN_SHA256,
        filename: "gemma-4-E2B-it-UD-Q4_K_XL.gguf",
    },
    PinnedArtifact {
        url: DRAFT_URL,
        sha256: DRAFT_SHA256,
        filename: "mtp-gemma-4-E2B-it.gguf",
    },
    PinnedArtifact {
        url: PROJECTOR_URL,
        sha256: PROJECTOR_SHA256,
        filename: "mmproj-F16.gguf",
    },
];

/// The live catalog: the rollout entry with its MTP drafter and projector,
/// served from a test-scoped cache directory.
fn live_config_toml(cache: &Path) -> String {
    format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = "{cache}"

[[local_model]]
name = "gemma-4"
description = "Gemma 4 E2B instruct with MTP drafting and vision, live CUDA proof"
source = "{MAIN_URL}"
sha256 = "{MAIN_SHA256}"
context = 131072
parallel = 1
flash_attention = true
thinking = "never"

[local_model.speculative]
type = "draft-mtp"
source = "{DRAFT_URL}"
sha256 = "{DRAFT_SHA256}"
draft_max = 2

[local_model.multimodal_projector]
source = "{PROJECTOR_URL}"
sha256 = "{PROJECTOR_SHA256}"
"#,
        cache = cache.display().to_string().replace('\\', "/"),
    )
}

/// Renders an error with its full `source` chain: the gateway's public error
/// types are opaque wrappers whose `Display` shows only the outer message, so
/// a phase failure must walk the chain to name the root cause.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}

/// Runs the real provisioning path (`ensure_model` for the main model and
/// both companions, plus embedded-bundle staging) and returns the assembled
/// gateway and the wall-clock cost.
///
/// A timeout panics but cannot cancel the blocking task; when it eventually
/// finishes, its returned gateway drops and kills the child.
async fn provision(toml: &str, timeout: Duration, phase: &str) -> (Gateway, Duration) {
    let toml = toml.to_owned();
    let started = Instant::now();
    let gateway = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            Gateway::from_config(
                &Config::from_toml_str(&toml).expect("live config parses"),
                ProfilesContext::default(),
            )
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("{phase} exceeded its timeout"))
    .expect("provisioning task panicked")
    .unwrap_or_else(|error| panic!("{phase} failed: {}", error_chain(&error)));
    (gateway, started.elapsed())
}

/// Polls the children's captured output until `predicate` holds, returning
/// the combined text.
async fn diagnostics_until(gateway: &Gateway, predicate: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + DIAGNOSTICS_TIMEOUT;
    loop {
        let text = gateway
            .local_diagnostics()
            .await
            .into_iter()
            .map(|(model, tail)| format!("== {model} ==\n{tail}"))
            .collect::<Vec<_>>()
            .join("\n");
        if predicate(&text) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for child log evidence; captured tail:\n{text}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The staged `llama-server.exe`, asserting it came from the embedded CUDA
/// bundle: the bundle staging path installs under a `cuda-*` directory,
/// while the archive path would have fetched into `downloads/` and installed
/// under a release-platform directory.
fn staged_cuda_executable(cache: &Path) -> PathBuf {
    let installs: Vec<PathBuf> = std::fs::read_dir(cache.join("llama.cpp"))
        .expect("llama.cpp cache dir exists")
        .map(|entry| entry.expect("read install entry").path())
        .collect();
    assert_eq!(
        installs.len(),
        1,
        "exactly one llama.cpp install expected: {installs:?}"
    );
    let install = &installs[0];
    let name = install.file_name().expect("install dir name");
    assert!(
        name.to_string_lossy().starts_with("cuda-"),
        "the staged server must come from the embedded CUDA bundle, got {}",
        install.display()
    );
    assert!(
        !cache.join("downloads").exists(),
        "a downloaded server archive must not exist in a CUDA build"
    );
    let executable = install.join("llama-server.exe");
    assert!(
        executable.is_file(),
        "staged llama-server.exe missing at {}",
        executable.display()
    );
    executable
}

/// The provisioning path's cache-slot key for a source: the first 16 hex
/// characters of the source's SHA-256.
fn source_cache_key(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The verified-digest marker path for a cached blob: `<blob>.verified`.
fn marker_path(blob: &Path) -> PathBuf {
    let mut name = blob.as_os_str().to_owned();
    name.push(".verified");
    PathBuf::from(name)
}

/// The cache-resident path of one pinned artifact.
fn blob_path(cache: &Path, artifact: &PinnedArtifact) -> PathBuf {
    cache
        .join("models")
        .join(source_cache_key(artifact.url))
        .join(artifact.filename)
}

/// Phase 5: every pinned artifact sits in its own cache slot with a marker
/// recording its pin, so provisioning verified all three digests.
fn assert_digest_markers(cache: &Path) {
    for artifact in &ARTIFACTS {
        let blob = blob_path(cache, artifact);
        assert!(blob.is_file(), "pinned blob missing at {}", blob.display());
        let marker = marker_path(&blob);
        let recorded = std::fs::read_to_string(&marker)
            .unwrap_or_else(|e| panic!("marker for {} unreadable: {e}", blob.display()));
        assert_eq!(
            recorded.lines().next(),
            Some(artifact.sha256),
            "marker for {} must record the pin",
            blob.display()
        );
    }
}

/// Size plus mtime of every pinned blob, in artifact order. A re-download
/// replaces the file and changes the fingerprint; a marker hit leaves it
/// untouched.
fn blob_fingerprints(cache: &Path) -> Vec<(u64, SystemTime)> {
    ARTIFACTS
        .iter()
        .map(|artifact| {
            let metadata = std::fs::metadata(blob_path(cache, artifact)).expect("blob metadata");
            (metadata.len(), metadata.modified().expect("blob mtime"))
        })
        .collect()
}

/// One non-streaming chat completion through the gateway, bounded by the
/// completion phase timeout.
async fn chat_completion(client: &reqwest::Client, addr: SocketAddr, request: &Value) -> Value {
    let response = tokio::time::timeout(
        COMPLETION_TIMEOUT,
        client
            .post(format!("http://{addr}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(request)
            .send(),
    )
    .await
    .expect("chat completion exceeded the phase timeout")
    .expect("chat completion send failed");
    let status = response.status();
    let body = tokio::time::timeout(COMPLETION_TIMEOUT, response.text())
        .await
        .expect("chat completion body exceeded the phase timeout")
        .expect("chat completion body read failed");
    assert_eq!(status.as_u16(), 200, "chat completion failed: {body}");
    serde_json::from_str(&body).expect("chat completion body is JSON")
}

/// Phase 6: an MTP completion under deterministic sampling must show the
/// drafter both proposed and landed tokens in the response's `timings`.
async fn prove_mtp(client: &reqwest::Client, addr: SocketAddr) {
    let completion = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": "Write the integers from 1 through 100, separated by one space, and output nothing else."
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 512
        }),
    )
    .await;
    assert_mtp_timings(&completion);
}

/// Phase 8: a tool call through the chat completions path must parse.
async fn prove_tool_call(client: &reqwest::Client, addr: SocketAddr) {
    let body = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": "What is the weather in Paris right now? Use the get_weather function."
            }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get the current weather in a named city",
                    "parameters": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } },
                        "required": ["city"]
                    }
                }
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 128
        }),
    )
    .await;
    assert_tool_call(&body);
}

/// Phase 9: a real image-content completion through the projector must
/// describe the generated test image.
async fn prove_image_completion(client: &reqwest::Client, addr: SocketAddr) {
    let body = chat_completion(
        client,
        addr,
        &serde_json::json!({
            "model": "gemma-4",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "The image is split vertically into two solid-color halves. Name the color of the left half and the color of the right half."
                    },
                    { "type": "image_url", "image_url": { "url": test_image_data_url() } }
                ]
            }],
            "temperature": 0,
            "seed": 42,
            "presence_penalty": 0,
            "max_tokens": 128
        }),
    )
    .await;
    let reply = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no text content in image completion: {body}"))
        .to_lowercase();
    assert!(
        reply.contains("red") && reply.contains("blue"),
        "image completion must name both colors, got: {reply}"
    );
}

/// Phases 2 through 5: embedded-bundle staging, CUDA device report, GPU
/// offload of both models, and verified digest markers.
async fn prove_staging_offload_and_pins(gateway: &Gateway, cache: &Path) {
    let staged = staged_cuda_executable(cache);
    eprintln!("staged embedded-bundle server at {}", staged.display());

    // The pinned server (third_party/llama.cpp @ fb0e6b6) never emits the
    // legacy `ggml_cuda_init` banner through llama-server's log path; the
    // device report is the per-model `llama_prepare_model_devices` line and
    // the offload evidence is one `offloaded n/n` line per model, so two
    // matches prove the target and the draft both offloaded.
    let diagnostics = diagnostics_until(gateway, |text| {
        text.contains("using device CUDA0") && text.matches("offloaded ").count() >= 2
    })
    .await;
    assert!(
        diagnostics.contains("CUDA0"),
        "no CUDA device report in child output:\n{diagnostics}"
    );
    assert!(
        !diagnostics.contains("offloaded 0/"),
        "a model offloaded no layers:\n{diagnostics}"
    );

    assert_digest_markers(cache);
}

/// Phase 6 helper: the response's `timings` extension must show the MTP
/// drafter both proposed and landed tokens.
fn assert_mtp_timings(body: &Value) {
    let timings = body
        .get("timings")
        .unwrap_or_else(|| panic!("no timings in response: {body}"));
    let drafted = timings
        .get("draft_n")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no draft_n in timings: {timings}"));
    let accepted = timings
        .get("draft_n_accepted")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no draft_n_accepted in timings: {timings}"));
    eprintln!("mtp timings: {timings}");
    assert!(drafted > 0, "the drafter proposed no tokens: {timings}");
    assert!(accepted > 0, "no drafted tokens were accepted: {timings}");
}

/// Phase 8: the model's reply must carry a tool call whose function
/// arguments parse as JSON.
fn assert_tool_call(body: &Value) {
    let tool_calls = body
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no tool_calls in response: {body}"));
    assert!(!tool_calls.is_empty(), "empty tool_calls: {body}");
    let function = tool_calls[0].get("function").expect("tool call function");
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .expect("tool call function name");
    assert!(!name.is_empty(), "empty tool call name: {body}");
    let arguments = function.get("arguments").expect("tool call arguments");
    let parsed: Value = match arguments {
        Value::String(text) => {
            serde_json::from_str(text).expect("tool call arguments string parses as JSON")
        }
        other => other.clone(),
    };
    assert!(
        parsed.is_object(),
        "tool call arguments not an object: {body}"
    );
}

/// Standard base64, so the test image needs no extra dependency.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = usize::from(*chunk.get(1).unwrap_or(&0));
        let b2 = usize::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(char::from(ALPHABET[(triple >> 18) & 63]));
        encoded.push(char::from(ALPHABET[(triple >> 12) & 63]));
        encoded.push(if chunk.len() > 1 {
            char::from(ALPHABET[(triple >> 6) & 63])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(ALPHABET[triple & 63])
        } else {
            '='
        });
    }
    encoded
}

/// A 64x64 PNG, left half pure red and right half pure blue, as a data URL.
fn test_image_data_url() -> String {
    let mut pixels = Vec::with_capacity(64 * 64 * 3);
    for _row in 0..64 {
        for column in 0..64 {
            if column < 32 {
                pixels.extend_from_slice(&[255, 0, 0]);
            } else {
                pixels.extend_from_slice(&[0, 0, 255]);
            }
        }
    }
    let mut png_bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_bytes, 64, 64);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // `write_header` consumes the encoder; dropping the writer writes IEND.
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&pixels).expect("png encode");
    drop(writer);
    format!("data:image/png;base64,{}", base64_encode(&png_bytes))
}

/// Live end-to-end proof on a CUDA host: provisioning, embedded-bundle
/// staging, CUDA device report, GPU offload of both models, digest markers,
/// MTP acceptance, cache reuse, a tool call, and a projector completion.
#[tokio::test]
#[ignore = "requires a Windows CUDA Toolkit, an NVIDIA GPU, and multi-gigabyte model downloads; set PROMPTFORGE_LIVE_CUDA=1 to opt in"]
async fn live_cuda_mtp_multimodal_end_to_end() {
    if std::env::var_os(LIVE_ENV).is_none() {
        eprintln!(
            "skipping: set {LIVE_ENV}=1 to run (needs a Windows CUDA Toolkit, an NVIDIA GPU, \
             and multi-gigabyte model downloads)"
        );
        return;
    }
    assert_eq!(
        base64_encode(b"Man"),
        "TWFu",
        "the test's base64 helper must be standard"
    );
    let _serial = LIVE_CUDA.lock().await;

    let cache = tempfile::tempdir().unwrap();
    let toml = live_config_toml(cache.path());

    // Phase 1: provision through the gateway's real machinery.
    let (gateway, first_provision) =
        provision(&toml, PROVISION_TIMEOUT, "initial provisioning").await;

    // Phases 2-5: embedded-bundle staging, CUDA device report, GPU offload
    // of the target and draft models, and verified digest markers.
    prove_staging_offload_and_pins(&gateway, cache.path()).await;

    let server = TestServer::start(gateway).await;
    let client = reqwest::Client::new();

    // Phase 6: an MTP completion under deterministic sampling.
    prove_mtp(&client, server.addr).await;

    // Phase 7: stop and relaunch against the same cache; the second
    // provision must reuse it (no re-download) and be no slower.
    server.shutdown().await;
    let before = blob_fingerprints(cache.path());
    let (gateway, second_provision) =
        provision(&toml, RELAUNCH_TIMEOUT, "cache-hit relaunch").await;
    assert_eq!(
        blob_fingerprints(cache.path()),
        before,
        "the relaunch re-downloaded artifacts"
    );
    assert!(
        second_provision <= first_provision,
        "cache-hit relaunch ({second_provision:?}) slower than the downloading first provision \
         ({first_provision:?})"
    );
    let server = TestServer::start(gateway).await;

    // Phases 8 and 9 run against the relaunched server, which also proves
    // the cache-hit child serves.
    prove_tool_call(&client, server.addr).await;
    prove_image_completion(&client, server.addr).await;

    server.shutdown().await;
}
