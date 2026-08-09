//! Acquire the sentence-embedding model this crate compiles into itself.
//!
//! The script reads a locally provisioned `BAAI/bge-small-en-v1.5` snapshot,
//! pinned to one immutable commit, checks every file against a hardcoded
//! SHA-256 digest, downcasts fp32 weights to fp16, and writes the result into
//! `OUT_DIR`. `src/assets.rs` then `include_bytes!`-embeds what lands
//! there, so a linked binary carries the model with no external file and no
//! network at run time. The pinned revision is written into `OUT_DIR` too, so
//! `src/assets.rs` can embed it rather than repeat it.
//!
//! Nothing is written inside the crate source tree: `OUT_DIR` lives under
//! `target/`, which is gitignored, so the weights never become git-visible.
//!
//! `PROMPTFORGE_MODEL_DIR` names the immutable provisioned input. A stamp with
//! strong output digests skips conversion only while every generated file
//! remains intact.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use half::f16;
use safetensors::tensor::{Dtype, SafeTensors, TensorView, View, serialize};
use sha2::{Digest, Sha256};

/// Owner of the Hugging Face model repository.
const REPO_OWNER: &str = "BAAI";

/// Name of the Hugging Face model repository.
const REPO_NAME: &str = "bge-small-en-v1.5";

/// The pinned revision: an immutable commit, not a branch. A branch name would
/// let the upstream repository change what this crate embeds, which defeats
/// both the digest check and reproducible builds.
const REVISION: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";

/// Upstream filename of the fp32 weights.
const WEIGHTS_SRC: &str = "model.safetensors";

/// Upstream filename of the tokenizer.
const TOKENIZER_SRC: &str = "tokenizer.json";

/// Upstream filename of the model configuration.
const CONFIG_SRC: &str = "config.json";

/// SHA-256 of `model.safetensors` at [`REVISION`].
const WEIGHTS_SHA256: &str = "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad";
const WEIGHTS_FP16_SHA256: &str =
    "cff97238f782a46572442917b398a59a8400fb2bdc2c7744eb9ac294c359e957";

/// SHA-256 of `tokenizer.json` at [`REVISION`].
const TOKENIZER_SHA256: &str = "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";

/// SHA-256 of `config.json` at [`REVISION`].
const CONFIG_SHA256: &str = "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750";

/// Name of the converted fp16 weights written into `OUT_DIR`.
const WEIGHTS_OUT: &str = "model-fp16.safetensors";

/// Name of the file in `OUT_DIR` holding [`REVISION`], with no trailing
/// newline, so `src/assets.rs` can embed the pin instead of repeating it.
const REVISION_OUT: &str = "revision.txt";

/// Name of the stamp recording which revision `OUT_DIR` already holds.
const STAMP_OUT: &str = "assets.stamp";

/// Bumped whenever the conversion changes, so an existing `OUT_DIR` produced by
/// an older script is rebuilt rather than trusted.
const CONVERSION_VERSION: &str = "1";

fn main() {
    if let Err(cause) = run() {
        // Print rather than return the error. Every failure here already
        // carries its own multi-line remediation text, and `Display` is the
        // form that keeps those lines intact.
        eprintln!("\npromptforge-tool-picker build script failed.\n\n{cause:?}\n");
        std::process::exit(1);
    }
}

/// Stage the model assets in `OUT_DIR`, doing nothing if they are already there.
///
/// # Errors
///
/// Fails if the Hub is unreachable with a cold cache, if a downloaded file does
/// not match its pinned digest, or if the conversion or any write fails.
fn run() -> Result<()> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PROMPTFORGE_MODEL_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("read OUT_DIR")?);

    // Written on every run, before the up-to-date shortcut: it is the single
    // source of truth for the pin that `src/assets.rs` exposes, and it costs 40
    // bytes to keep it in step with this script unconditionally.
    let stamp_header = format!("{REVISION} fp16 v{CONVERSION_VERSION}");
    if is_up_to_date(&out_dir, &stamp_header) {
        return Ok(());
    }

    let weights = fetch(WEIGHTS_SRC, WEIGHTS_SHA256)?;
    let tokenizer = fetch(TOKENIZER_SRC, TOKENIZER_SHA256)?;
    let config = fetch(CONFIG_SRC, CONFIG_SHA256)?;

    write_atomic(&out_dir.join(WEIGHTS_OUT), &to_fp16(&weights)?)?;
    write_atomic(&out_dir.join(TOKENIZER_SRC), &tokenizer)?;
    write_atomic(&out_dir.join(CONFIG_SRC), &config)?;
    write_atomic(&out_dir.join(REVISION_OUT), REVISION.as_bytes())?;
    let mut stamp = format!("{stamp_header}\n");
    for name in [WEIGHTS_OUT, TOKENIZER_SRC, CONFIG_SRC, REVISION_OUT] {
        let bytes = std::fs::read(out_dir.join(name))
            .with_context(|| format!("read generated {}", out_dir.join(name).display()))?;
        writeln!(stamp, "{name} {}", hex(&Sha256::digest(bytes)))?;
    }
    write_atomic(&out_dir.join(STAMP_OUT), stamp.as_bytes())?;

    Ok(())
}

/// Whether `OUT_DIR` already holds converted assets for this exact revision.
fn is_up_to_date(out_dir: &Path, header: &str) -> bool {
    let recorded = std::fs::read_to_string(out_dir.join(STAMP_OUT)).unwrap_or_default();
    let mut lines = recorded.lines();
    if lines.next() != Some(header) {
        return false;
    }
    let recorded = lines.collect::<Vec<_>>();
    let names = [WEIGHTS_OUT, TOKENIZER_SRC, CONFIG_SRC, REVISION_OUT];
    recorded.len() == names.len()
        && recorded.iter().zip(names).all(|(line, required)| {
            let Some((name, expected)) = line.split_once(' ') else {
                return false;
            };
            name == required
                && std::fs::read(out_dir.join(name))
                    .is_ok_and(|bytes| hex(&Sha256::digest(bytes)) == expected)
        })
}

/// Reads one provisioned file and checks it against `expected`.
///
/// # Errors
///
/// Fails if the provisioned directory or file is missing, or if the bytes do
/// not hash to `expected`.
fn fetch(filename: &str, expected: &str) -> Result<Vec<u8>> {
    let root = PathBuf::from(std::env::var("PROMPTFORGE_MODEL_DIR").with_context(
        || "PROMPTFORGE_MODEL_DIR must name a provisioned, immutable model directory",
    )?);
    let source_name = if filename == WEIGHTS_SRC && !root.join(filename).is_file() {
        WEIGHTS_OUT
    } else {
        filename
    };
    let path = root.join(source_name);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read provisioned model input {}", path.display()))?;

    let actual = hex(Sha256::digest(&bytes).as_slice());
    let accepted = if source_name == WEIGHTS_OUT {
        WEIGHTS_FP16_SHA256
    } else {
        expected
    };
    if actual != accepted {
        bail!(
            "checksum mismatch for {REPO_OWNER}/{REPO_NAME}@{REVISION}/{filename}\n  \
             expected sha256 {accepted}\n  \
             actual   sha256 {actual}\n  \
             provisioned at {}\n\
             Replace the provisioned immutable input before rebuilding.",
            path.display()
        );
    }
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("old");
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("write staged asset {}", temporary.display()))?;
    if path.is_file() {
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(path, &backup)
            .with_context(|| format!("stage existing asset {}", path.display()))?;
    }
    std::fs::rename(&temporary, path)
        .with_context(|| format!("commit generated asset {}", path.display()))?;
    if backup.is_file() {
        std::fs::remove_file(&backup)
            .with_context(|| format!("remove superseded asset {}", backup.display()))?;
    }
    Ok(())
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
///
/// Halving the weights halves what every linked binary carries (about 130MB
/// down to 65MB). This is a storage decision only: fp16 is not asserted to be
/// the right *compute* dtype, and the loader is free to upcast to f32 in memory,
/// which is what Candle's uneven f16 CPU coverage will likely want.
///
/// Tensors of any other dtype are copied through unchanged.
///
/// # Errors
///
/// Fails if the input is not a valid safetensors file, if an fp32 tensor's byte
/// length is not a multiple of four, or if the result cannot be serialized.
fn to_fp16(bytes: &[u8]) -> Result<Vec<u8>> {
    let source = SafeTensors::deserialize(bytes)?;
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
    Ok(serialize(named, Some(metadata))?)
}

/// Downcast one tensor to fp16, or copy it through if it is not fp32.
///
/// # Errors
///
/// Fails if an fp32 tensor's byte length is not a multiple of four.
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
    for chunk in source.chunks_exact(4) {
        let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        data.extend_from_slice(&f16::from_f32(f32::from_bits(bits)).to_le_bytes());
    }
    Ok(OwnedTensor {
        dtype: Dtype::F16,
        shape,
        data,
    })
}
