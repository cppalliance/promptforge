//! Live CUDA proof: an opt-in end-to-end run of a CUDA `llama-server` with
//! an MTP drafter and a multimodal projector on real hardware.
//!
//! The test is `#[ignore]`d and additionally opt-in: even when forced with
//! `--ignored`, it prints a skip notice and returns `Ok` unless
//! `PROMPTFORGE_LIVE_CUDA=1` is set. `PROMPTFORGE_LLAMA_SERVER` names the
//! CUDA `llama-server.exe` under test (for example one unpacked from the
//! `llama-cuda-blackwell` release); without it the gateway's managed
//! download decides the backend. Run it with:
//!
//! ```text
//! cargo test -p gateway -- --ignored live_cuda
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gateway::{Config, Gateway, ProfilesContext};
use sha2::{Digest as _, Sha256};

use crate::support::TestServer;

mod completions;

use completions::{base64_encode, prove_image_completion, prove_mtp, prove_tool_call};

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
const PROVISION_TIMEOUT: Duration = Duration::from_mins(45);
/// A marker-hit relaunch skips downloads and re-hashing; only spawn and
/// weight load remain.
const RELAUNCH_TIMEOUT: Duration = Duration::from_mins(15);
/// One completion against a warm server.
const COMPLETION_TIMEOUT: Duration = Duration::from_mins(5);
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
config-version = 0

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
/// both companions, plus server resolution) and returns the assembled
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

/// The `llama-server.exe` under test. With `PROMPTFORGE_LLAMA_SERVER` set
/// (the release workflow's smoke job points it at the freshly built binary),
/// the gateway runs exactly that executable and stages nothing; without it,
/// the managed download must have installed one into the cache.
fn staged_cuda_executable(cache: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("PROMPTFORGE_LLAMA_SERVER") {
        let executable = PathBuf::from(path);
        assert!(
            executable.is_file(),
            "PROMPTFORGE_LLAMA_SERVER names no file at {}",
            executable.display()
        );
        assert!(
            !cache.join("llama.cpp").exists(),
            "an external llama-server must stage nothing into the cache"
        );
        return executable;
    }
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
        name.to_string_lossy().contains("cuda"),
        "the staged server must be a CUDA build, got {}",
        install.display()
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

/// Phases 2 through 5: server staging, CUDA device report, GPU offload of
/// both models, and verified digest markers.
async fn prove_staging_offload_and_pins(gateway: &Gateway, cache: &Path) {
    let staged = staged_cuda_executable(cache);
    eprintln!("staged CUDA server at {}", staged.display());

    // The pinned server (llama.cpp b10082) never emits the
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

/// Live end-to-end proof on a CUDA machine: provisioning, server staging, CUDA
/// device report, GPU offload of both models, digest markers, MTP
/// acceptance, cache reuse, a tool call, and a projector completion.
#[tokio::test]
#[ignore = "requires an NVIDIA GPU and multi-gigabyte model downloads; set PROMPTFORGE_LIVE_CUDA=1 to opt in"]
async fn live_cuda_mtp_multimodal_end_to_end() {
    if std::env::var_os(LIVE_ENV).is_none() {
        eprintln!(
            "skipping: set {LIVE_ENV}=1 to run (needs an NVIDIA GPU and multi-gigabyte \
             model downloads)"
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
