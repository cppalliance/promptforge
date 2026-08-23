//! Acquire the sentence-embedding model this crate compiles into itself.
//!
//! The script downloads `BAAI/bge-small-en-v1.5` from the Hugging Face Hub,
//! pinned to one immutable commit, checks every file against a hardcoded
//! SHA-256 digest, downcasts the fp32 weights to fp16, and writes the result
//! into `OUT_DIR` via a temporary file renamed into place. `src/assets.rs` then
//! `include_bytes!`-embeds what lands there. The pinned revision and source
//! repository are written into `OUT_DIR` too, so `src/assets.rs` embeds one
//! generated provenance record rather than repeating it.
//!
//! Nothing is written inside the crate source tree: `OUT_DIR` lives under
//! `target/`, which is gitignored.
//!
//! The first build needs network access. Later builds reuse the Hugging Face
//! cache. A stamp file recording the pinned revision, the conversion version,
//! and the SHA-256 of every generated output skips the work entirely while all
//! three outputs still hash to what the stamp records.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use half::f16;
use hf_hub::HFClientSync;
use safetensors::tensor::{Dtype, SafeTensors, TensorView, View, serialize};
use sha2::{Digest, Sha256};

/// Owner of the Hugging Face model repository.
const REPO_OWNER: &str = "BAAI";

/// Name of the Hugging Face model repository.
const REPO_NAME: &str = "bge-small-en-v1.5";

/// The pinned revision: an immutable commit, not a branch.
const REVISION: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";

/// Upstream filename of the fp32 weights.
const WEIGHTS_SRC: &str = "model.safetensors";

/// Upstream filename of the tokenizer.
const TOKENIZER_SRC: &str = "tokenizer.json";

/// Upstream filename of the model configuration.
const CONFIG_SRC: &str = "config.json";

/// SHA-256 of `model.safetensors` at [`REVISION`].
const WEIGHTS_SHA256: &str = "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad";

/// SHA-256 of `tokenizer.json` at [`REVISION`].
const TOKENIZER_SHA256: &str = "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";

/// SHA-256 of `config.json` at [`REVISION`].
const CONFIG_SHA256: &str = "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750";

/// Name of the converted fp16 weights written into `OUT_DIR`.
const WEIGHTS_OUT: &str = "model-fp16.safetensors";

/// Name of the file in `OUT_DIR` holding [`REVISION`].
const REVISION_OUT: &str = "revision.txt";

/// Name of the file in `OUT_DIR` holding the source repository identity.
const REPO_OUT: &str = "repo.txt";

/// Name of the stamp recording which outputs `OUT_DIR` already holds.
const STAMP_OUT: &str = "assets.stamp";

/// Bumped whenever the conversion changes, so an existing `OUT_DIR` produced by
/// an older script is rebuilt rather than trusted.
const CONVERSION_VERSION: &str = "2";

/// The three generated outputs whose digests the stamp records.
const OUTPUTS: [&str; 3] = [WEIGHTS_OUT, TOKENIZER_SRC, CONFIG_SRC];

fn main() {
    if let Err(cause) = run() {
        // Debug formatting prints the whole context chain; each Hub failure
        // still carries its own multi-line remediation text.
        eprintln!("\npromptforge-tool-picker build script failed.\n\n{cause:?}\n");
        std::process::exit(1);
    }
}

/// Stage the model assets in `OUT_DIR`, doing nothing if they are already there.
fn run() -> Result<()> {
    println!("cargo::rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").context("OUT_DIR is unset")?);

    // Provenance is generated on every run, before the up-to-date shortcut, so
    // it stays the single source of truth for what `src/assets.rs` exposes.
    write_atomic(&out_dir, REVISION_OUT, REVISION.as_bytes())?;
    write_atomic(
        &out_dir,
        REPO_OUT,
        format!("{REPO_OWNER}/{REPO_NAME}").as_bytes(),
    )?;

    if is_up_to_date(&out_dir) {
        return Ok(());
    }

    let weights = fetch(WEIGHTS_SRC, WEIGHTS_SHA256)?;
    let tokenizer = fetch(TOKENIZER_SRC, TOKENIZER_SHA256)?;
    let config = fetch(CONFIG_SRC, CONFIG_SHA256)?;

    let weights_fp16 = to_fp16(&weights).context("downcast the weights to fp16")?;
    write_atomic(&out_dir, WEIGHTS_OUT, &weights_fp16)?;
    write_atomic(&out_dir, TOKENIZER_SRC, &tokenizer)?;
    write_atomic(&out_dir, CONFIG_SRC, &config)?;

    // The stamp is written last, after the outputs are in place, and records a
    // digest of each so a truncated or replaced output is a cache miss.
    write_atomic(&out_dir, STAMP_OUT, stamp(&out_dir)?.as_bytes())?;
    Ok(())
}

/// The stamp text: revision, conversion version, and each output's digest.
fn stamp(out_dir: &Path) -> Result<String> {
    let mut stamp = format!("{REVISION} fp16 v{CONVERSION_VERSION}\n");
    for name in OUTPUTS {
        let path = out_dir.join(name);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read generated output {}", path.display()))?;
        let _ = writeln!(stamp, "{name} {}", hex(Sha256::digest(&bytes).as_slice()));
    }
    Ok(stamp)
}

/// Whether `OUT_DIR` already holds converted outputs matching the stamp.
///
/// Recomputes each output's digest and compares it to the stamp, so a
/// truncated, corrupted, or locally replaced output is treated as a cache miss.
fn is_up_to_date(out_dir: &Path) -> bool {
    let Ok(recorded) = std::fs::read_to_string(out_dir.join(STAMP_OUT)) else {
        return false;
    };
    match stamp(out_dir) {
        Ok(current) => current == recorded,
        Err(_) => false,
    }
}

/// Write `bytes` to `dir/name` via a temporary file renamed into place.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let final_path = dir.join(name);
    let temp_path = dir.join(format!("{name}.tmp"));
    std::fs::write(&temp_path, bytes)
        .with_context(|| format!("write staged output {}", temp_path.display()))?;
    std::fs::rename(&temp_path, &final_path)
        .with_context(|| format!("commit output {}", final_path.display()))?;
    Ok(())
}

/// Download one file from the pinned revision and check it against `expected`.
fn fetch(filename: &str, expected: &str) -> Result<Vec<u8>> {
    let client = HFClientSync::new().map_err(|e| unreachable_hub(filename, &e))?;
    let path = client
        .model(REPO_OWNER, REPO_NAME)
        .download_file()
        .filename(filename)
        .revision(REVISION)
        .send()
        .map_err(|e| unreachable_hub(filename, &e))?;
    let bytes =
        std::fs::read(&path).with_context(|| format!("read cached download {}", path.display()))?;

    let actual = hex(Sha256::digest(&bytes).as_slice());
    if actual != expected {
        bail!(
            "checksum mismatch for {REPO_OWNER}/{REPO_NAME}@{REVISION}/{filename}\n  \
             expected sha256 {expected}\n  \
             actual   sha256 {actual}\n  \
             cached at {}\n\
             The pinned revision is immutable, so this file is corrupt or tampered with. \
             Delete the cached copy and rebuild.",
            path.display()
        );
    }
    Ok(bytes)
}

/// Turn a Hub failure into a message that says what to do about it.
fn unreachable_hub(filename: &str, cause: &dyn std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "could not obtain {REPO_OWNER}/{REPO_NAME}@{REVISION}/{filename}: {cause}\n\
         This crate compiles the embedding model into the library, so the first build \
         needs network access to the Hugging Face Hub (about 130MB). Later builds reuse \
         the Hugging Face cache; set HF_HUB_CACHE or HF_HOME to point at a warm one, or \
         set HF_ENDPOINT to a reachable mirror."
    )
}

/// Lowercase hex encoding of a digest.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// An owned tensor, since the converted data outlives the source view.
struct OwnedTensor {
    /// Element type of the tensor.
    dtype: Dtype,
    /// Dimensions, row-major.
    shape: Vec<usize>,
    /// Raw little-endian element bytes.
    data: Vec<u8>,
}

impl View for &OwnedTensor {
    fn dtype(&self) -> Dtype {
        self.dtype
    }
    fn shape(&self) -> &[usize] {
        &self.shape
    }
    fn data(&self) -> std::borrow::Cow<'_, [u8]> {
        self.data.as_slice().into()
    }
    fn data_len(&self) -> usize {
        self.data.len()
    }
}

/// Rewrite a safetensors blob with every fp32 tensor downcast to fp16.
fn to_fp16(bytes: &[u8]) -> Result<Vec<u8>> {
    let source = SafeTensors::deserialize(bytes).context("parse the upstream safetensors blob")?;
    let converted: Vec<(String, OwnedTensor)> = source
        .tensors()
        .into_iter()
        .map(|(name, view)| convert(&name, &view).map(|tensor| (name, tensor)))
        .collect::<Result<_, _>>()?;

    let metadata = HashMap::from([
        ("format".to_owned(), "pt".to_owned()),
        (
            "source".to_owned(),
            format!("{REPO_OWNER}/{REPO_NAME}@{REVISION}"),
        ),
        ("precision".to_owned(), "fp16 downcast from fp32".to_owned()),
    ]);
    let named = converted
        .iter()
        .map(|(name, tensor)| (name.as_str(), tensor));
    serialize(named, Some(metadata)).context("serialize the converted safetensors blob")
}

/// Downcast one tensor to fp16, or copy it through if it is not fp32.
fn convert(name: &str, view: &TensorView<'_>) -> Result<OwnedTensor> {
    let shape = view.shape().to_vec();
    if view.dtype() != Dtype::F32 {
        return Ok(OwnedTensor {
            dtype: view.dtype(),
            shape,
            data: view.data().to_vec(),
        });
    }

    let source = view.data();
    if source.len() % 4 != 0 {
        bail!(
            "tensor {name} is F32 but its {} bytes are not a whole number of f32 values",
            source.len()
        );
    }
    let mut data = Vec::with_capacity(source.len() / 2);
    for chunk in source.as_chunks::<4>().0 {
        let bits = u32::from_le_bytes(*chunk);
        data.extend_from_slice(&f16::from_f32(f32::from_bits(bits)).to_le_bytes());
    }
    Ok(OwnedTensor {
        dtype: Dtype::F16,
        shape,
        data,
    })
}
