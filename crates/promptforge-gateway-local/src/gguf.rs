//! GGUF header inspection: architecture, layer count, and parameter count
//! read from a model file's metadata without touching tensor data.
//!
//! The parser walks the GGUF header (magic, version, tensor count, metadata
//! key-value pairs) and, when `general.parameter_count` is absent, the
//! tensor-info table, seeking over every payload it does not need. Reads are
//! bounded: materialized strings are capped, entry counts are sanity-limited,
//! and every skip is checked against the file length, so a malformed or
//! hostile file fails with a typed error instead of an unbounded read.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::artifacts::{safe_relative_path, validate_cache_path};
use crate::error::LocalError;

/// Magic bytes opening every GGUF file.
const GGUF_MAGIC: [u8; 4] = *b"GGUF";

/// Ceiling on metadata key-value entries; real models carry a few dozen.
const MAX_METADATA_ENTRIES: u64 = 65_536;

/// Ceiling on tensor-info entries; the largest real models stay in the
/// low tens of thousands.
const MAX_TENSORS: u64 = 65_536;

/// Ceiling on a metadata key or a captured string value. The keys and the
/// architecture name this module reads are short dotted identifiers; a
/// longer one means the file is not worth trusting.
const MAX_STRING_BYTES: u64 = 4_096;

/// Ceiling on array-in-array nesting while skipping metadata values.
const MAX_ARRAY_DEPTH: u32 = 8;

/// Ceiling on tensor dimensions; the GGUF spec currently uses at most 4.
const MAX_TENSOR_DIMS: u32 = 8;

/// GGUF metadata value-type tags, per the GGUF specification.
const TYPE_UINT8: u32 = 0;
const TYPE_INT8: u32 = 1;
const TYPE_UINT16: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_FLOAT32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;
const TYPE_FLOAT64: u32 = 12;

/// The byte width of a fixed-size metadata value type, or `None` for
/// strings, arrays, and unknown tags.
fn fixed_size(value_type: u32) -> Option<u64> {
    match value_type {
        TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => Some(1),
        TYPE_UINT16 | TYPE_INT16 => Some(2),
        TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => Some(4),
        TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => Some(8),
        _ => None,
    }
}

/// Model facts read from a GGUF header.
///
/// Every field is optional: a well-formed header that simply lacks a key
/// reports `None` for it rather than failing, so the caller can fall back
/// per field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ModelInfo {
    /// The `general.architecture` metadata value (e.g. `llama`).
    pub architecture: Option<String>,
    /// The `<architecture>.block_count` metadata value: the transformer
    /// layer count the UI's `gpu_layers` readout divides against.
    pub layer_count: Option<u64>,
    /// The `general.parameter_count` metadata value when present, otherwise
    /// the sum of tensor-shape element counts from the tensor-info table.
    pub parameter_count: Option<u64>,
}

/// Reads [`ModelInfo`] from the GGUF file at `relative` under `root`.
///
/// `relative` is caller input and is parsed at this boundary: it must be a
/// plain relative path (no absolute prefix, no `..`) and must resolve under
/// `root` without crossing a symlink or reparse point. Only the header and
/// tensor-info table are read; tensor data is never touched.
///
/// # Errors
/// Returns [`LocalError::UnsafeCachePath`] when `relative` escapes `root`,
/// [`LocalError::MalformedGguf`] when the file is not a well-formed GGUF
/// header, and [`LocalError::Io`] when the file cannot be opened or read.
pub fn read_model_info(root: &Path, relative: &Path) -> Result<ModelInfo, LocalError> {
    if !safe_relative_path(relative) {
        return Err(LocalError::UnsafeCachePath {
            path: relative.to_owned(),
        });
    }
    let path = root.join(relative);
    validate_cache_path(root, &path)?;
    let file = File::open(&path).map_err(|source| LocalError::Io {
        operation: "open GGUF file",
        path: path.clone(),
        source,
    })?;
    let len = file
        .metadata()
        .map_err(|source| LocalError::Io {
            operation: "inspect GGUF file",
            path: path.clone(),
            source,
        })?
        .len();
    let mut reader = HeaderReader {
        inner: BufReader::new(file),
        path,
        pos: 0,
        len,
    };
    reader.parse()
}

/// A bounded, position-tracking reader over one GGUF file.
///
/// Every skip is validated against the file length before seeking, so a
/// bogus length field fails as malformed instead of walking off the file.
struct HeaderReader {
    inner: BufReader<File>,
    path: PathBuf,
    pos: u64,
    len: u64,
}

impl HeaderReader {
    /// A malformed-header error naming this file and `reason`.
    fn malformed(&self, reason: impl Into<String>) -> LocalError {
        LocalError::MalformedGguf {
            path: self.path.clone(),
            reason: reason.into(),
        }
    }

    /// Reads exactly `N` bytes, treating end-of-file as a malformed header.
    fn read_bytes<const N: usize>(&mut self) -> Result<[u8; N], LocalError> {
        let mut buffer = [0u8; N];
        self.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    /// Fills `buffer`, treating end-of-file as a malformed header.
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), LocalError> {
        self.inner.read_exact(buffer).map_err(|source| {
            if source.kind() == io::ErrorKind::UnexpectedEof {
                self.malformed("truncated file")
            } else {
                LocalError::Io {
                    operation: "read GGUF header",
                    path: self.path.clone(),
                    source,
                }
            }
        })?;
        self.pos += buffer.len() as u64;
        Ok(())
    }

    fn read_u32(&mut self) -> Result<u32, LocalError> {
        Ok(u32::from_le_bytes(self.read_bytes::<4>()?))
    }

    fn read_u64(&mut self) -> Result<u64, LocalError> {
        Ok(u64::from_le_bytes(self.read_bytes::<8>()?))
    }

    /// Seeks forward `count` bytes, refusing to pass the end of the file.
    fn skip(&mut self, count: u64) -> Result<(), LocalError> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| self.malformed("value overruns the file"))?;
        let offset = i64::try_from(count).map_err(|_| self.malformed("value overruns the file"))?;
        self.inner
            .seek_relative(offset)
            .map_err(|source| LocalError::Io {
                operation: "seek GGUF header",
                path: self.path.clone(),
                source,
            })?;
        self.pos = end;
        Ok(())
    }

    /// Reads a GGUF string (u64 length + UTF-8 bytes), capped at
    /// [`MAX_STRING_BYTES`]; `what` names the field in the error.
    fn read_string(&mut self, what: &str) -> Result<String, LocalError> {
        let length = self.read_u64()?;
        if length > MAX_STRING_BYTES {
            return Err(self.malformed(format!("{what} length {length} exceeds the cap")));
        }
        let capacity = usize::try_from(length)
            .map_err(|_| self.malformed(format!("{what} length {length} exceeds the cap")))?;
        let mut bytes = vec![0u8; capacity];
        self.read_exact(&mut bytes)?;
        String::from_utf8(bytes).map_err(|_| self.malformed(format!("{what} is not UTF-8")))
    }

    /// Skips a GGUF string without materializing it.
    fn skip_string(&mut self) -> Result<(), LocalError> {
        let length = self.read_u64()?;
        self.skip(length)
    }

    /// Skips one metadata value of `value_type`, recursing into arrays.
    fn skip_value(&mut self, value_type: u32, depth: u32) -> Result<(), LocalError> {
        if let Some(size) = fixed_size(value_type) {
            return self.skip(size);
        }
        match value_type {
            TYPE_STRING => self.skip_string(),
            TYPE_ARRAY => {
                if depth >= MAX_ARRAY_DEPTH {
                    return Err(self.malformed("array nesting exceeds the cap"));
                }
                let element_type = self.read_u32()?;
                let count = self.read_u64()?;
                if let Some(size) = fixed_size(element_type) {
                    let total = count
                        .checked_mul(size)
                        .ok_or_else(|| self.malformed("array size overflows"))?;
                    self.skip(total)
                } else {
                    // Strings and nested arrays have no fixed width, so each
                    // element is walked; every iteration consumes at least
                    // one length field, so the file length bounds the loop.
                    for _ in 0..count {
                        self.skip_value(element_type, depth + 1)?;
                    }
                    Ok(())
                }
            }
            other => Err(self.malformed(format!("unknown metadata value type {other}"))),
        }
    }

    /// Consumes one metadata value, returning it as `u64` when it is a
    /// non-negative integer type and `None` (value skipped) otherwise.
    fn read_int_value(&mut self, value_type: u32) -> Result<Option<u64>, LocalError> {
        Ok(match value_type {
            TYPE_UINT8 => Some(u64::from(self.read_bytes::<1>()?[0])),
            TYPE_INT8 => u64::try_from(i8::from_le_bytes(self.read_bytes::<1>()?)).ok(),
            TYPE_UINT16 => Some(u64::from(u16::from_le_bytes(self.read_bytes::<2>()?))),
            TYPE_INT16 => u64::try_from(i16::from_le_bytes(self.read_bytes::<2>()?)).ok(),
            TYPE_UINT32 => Some(u64::from(self.read_u32()?)),
            TYPE_INT32 => u64::try_from(i32::from_le_bytes(self.read_bytes::<4>()?)).ok(),
            TYPE_UINT64 => Some(self.read_u64()?),
            TYPE_INT64 => u64::try_from(i64::from_le_bytes(self.read_bytes::<8>()?)).ok(),
            other => {
                self.skip_value(other, 0)?;
                None
            }
        })
    }

    /// Parses the header: magic, version, counts, metadata, and - when the
    /// parameter count is not declared - the tensor-info table.
    fn parse(&mut self) -> Result<ModelInfo, LocalError> {
        if self.read_bytes::<4>()? != GGUF_MAGIC {
            return Err(self.malformed("not a GGUF file (bad magic)"));
        }
        let version = self.read_u32()?;
        if !(2..=3).contains(&version) {
            // A byte-swapped version field is the fingerprint of the
            // big-endian GGUF variant, worth naming in the error.
            let reason = if (2..=3).contains(&version.swap_bytes()) {
                "big-endian GGUF is not supported".to_owned()
            } else {
                format!("unsupported GGUF version {version}")
            };
            return Err(self.malformed(reason));
        }
        let tensor_count = self.read_u64()?;
        if tensor_count > MAX_TENSORS {
            return Err(self.malformed(format!("tensor count {tensor_count} exceeds the cap")));
        }
        let metadata_count = self.read_u64()?;
        if metadata_count > MAX_METADATA_ENTRIES {
            return Err(self.malformed(format!("metadata count {metadata_count} exceeds the cap")));
        }

        let mut architecture: Option<String> = None;
        let mut parameter_count: Option<u64> = None;
        // Key order is not guaranteed, so every `*.block_count` is kept and
        // joined against the architecture after the walk.
        let mut block_counts: HashMap<String, u64> = HashMap::new();
        for _ in 0..metadata_count {
            let key = self.read_string("metadata key")?;
            let value_type = self.read_u32()?;
            if key == "general.architecture" && value_type == TYPE_STRING {
                architecture = Some(self.read_string("architecture value")?);
            } else if key == "general.parameter_count" {
                if let Some(value) = self.read_int_value(value_type)? {
                    parameter_count = Some(value);
                }
            } else if key.ends_with(".block_count") {
                if let Some(value) = self.read_int_value(value_type)? {
                    block_counts.insert(key, value);
                }
            } else {
                self.skip_value(value_type, 0)?;
            }
        }

        if parameter_count.is_none() && tensor_count > 0 {
            parameter_count = Some(self.sum_tensor_elements(tensor_count)?);
        }

        let layer_count = architecture
            .as_deref()
            .and_then(|arch| block_counts.get(&format!("{arch}.block_count")).copied());
        Ok(ModelInfo {
            architecture,
            layer_count,
            parameter_count,
        })
    }

    /// Walks the tensor-info table and sums each tensor's element count,
    /// the parameter-count fallback when `general.parameter_count` is absent.
    fn sum_tensor_elements(&mut self, tensor_count: u64) -> Result<u64, LocalError> {
        let mut total: u128 = 0;
        for _ in 0..tensor_count {
            self.skip_string()?;
            let dimension_count = self.read_u32()?;
            if dimension_count > MAX_TENSOR_DIMS {
                return Err(self.malformed(format!(
                    "tensor dimension count {dimension_count} exceeds the cap"
                )));
            }
            let mut elements: u128 = 1;
            for _ in 0..dimension_count {
                elements = elements
                    .checked_mul(u128::from(self.read_u64()?))
                    .ok_or_else(|| self.malformed("tensor shape overflows"))?;
            }
            let _ggml_type = self.read_u32()?;
            let _data_offset = self.read_u64()?;
            total = total
                .checked_add(elements)
                .ok_or_else(|| self.malformed("parameter count overflows"))?;
        }
        u64::try_from(total).map_err(|_| self.malformed("parameter count overflows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Appends a GGUF string (u64 LE length + bytes) to `out`.
    fn push_string(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }

    /// The fixed GGUF prelude: magic, version 3, and the two counts.
    fn push_prelude(out: &mut Vec<u8>, tensor_count: u64, metadata_count: u64) {
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&tensor_count.to_le_bytes());
        out.extend_from_slice(&metadata_count.to_le_bytes());
    }

    /// A minimal well-formed GGUF header: architecture, block count,
    /// declared parameter count, plus one skipped string array and one
    /// skipped float to exercise the skip paths.
    fn synthetic_gguf() -> Vec<u8> {
        let mut out = Vec::new();
        push_prelude(&mut out, 0, 5);
        push_string(&mut out, "general.architecture");
        out.extend_from_slice(&TYPE_STRING.to_le_bytes());
        push_string(&mut out, "llama");
        push_string(&mut out, "tokenizer.ggml.tokens");
        out.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
        out.extend_from_slice(&TYPE_STRING.to_le_bytes());
        out.extend_from_slice(&2u64.to_le_bytes());
        push_string(&mut out, "<s>");
        push_string(&mut out, "</s>");
        push_string(&mut out, "llama.block_count");
        out.extend_from_slice(&TYPE_UINT32.to_le_bytes());
        out.extend_from_slice(&32u32.to_le_bytes());
        push_string(&mut out, "general.parameter_count");
        out.extend_from_slice(&TYPE_UINT64.to_le_bytes());
        out.extend_from_slice(&8_030_000_000u64.to_le_bytes());
        push_string(&mut out, "llama.rope.freq_base");
        out.extend_from_slice(&TYPE_FLOAT32.to_le_bytes());
        out.extend_from_slice(&10_000.0f32.to_le_bytes());
        out
    }

    /// Writes `bytes` as `models/<name>` under a fresh cache root and
    /// returns the root.
    fn cache_with(name: &str, bytes: &[u8]) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("models");
        std::fs::create_dir_all(&models).expect("mkdir models");
        std::fs::write(models.join(name), bytes).expect("write fixture");
        temp
    }

    #[test]
    fn reads_architecture_layer_and_parameter_counts() {
        let root = cache_with("tiny.gguf", &synthetic_gguf());
        let info = read_model_info(root.path(), Path::new("models/tiny.gguf"))
            .expect("the synthetic header parses");
        assert_eq!(
            info,
            ModelInfo {
                architecture: Some("llama".to_owned()),
                layer_count: Some(32),
                parameter_count: Some(8_030_000_000),
            }
        );
    }

    #[test]
    fn parameter_count_falls_back_to_tensor_shapes() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 2, 2);
        push_string(&mut bytes, "general.architecture");
        bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
        push_string(&mut bytes, "llama");
        push_string(&mut bytes, "llama.block_count");
        bytes.extend_from_slice(&TYPE_UINT32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        // Tensor infos: name, n_dims, dims, ggml type, offset.
        push_string(&mut bytes, "blk.0.attn_q.weight");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&4_096u64.to_le_bytes());
        bytes.extend_from_slice(&32u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        push_string(&mut bytes, "output.bias");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&10u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());

        let root = cache_with("shapes.gguf", &bytes);
        let info = read_model_info(root.path(), Path::new("models/shapes.gguf"))
            .expect("the tensor-shape header parses");
        assert_eq!(info.parameter_count, Some(4_096 * 32 + 10));
        assert_eq!(info.layer_count, Some(2));
    }

    #[test]
    fn wrong_magic_is_malformed_not_a_panic() {
        let root = cache_with("junk.gguf", b"not a gguf file at all");
        let error = read_model_info(root.path(), Path::new("models/junk.gguf"))
            .expect_err("junk bytes are refused");
        assert!(matches!(error, LocalError::MalformedGguf { .. }));
        assert!(error.to_string().contains("bad magic"));
    }

    #[test]
    fn truncated_file_is_malformed_not_a_panic() {
        let full = synthetic_gguf();
        let root = cache_with("cut.gguf", &full[..full.len() / 2]);
        let error = read_model_info(root.path(), Path::new("models/cut.gguf"))
            .expect_err("a truncated header is refused");
        assert!(matches!(error, LocalError::MalformedGguf { .. }));
    }

    #[test]
    fn unsupported_version_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        let root = cache_with("v1.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/v1.gguf"))
            .expect_err("version 1 is refused");
        assert!(error.to_string().contains("unsupported GGUF version 1"));
    }

    #[test]
    fn bogus_string_length_is_malformed_not_an_unbounded_read() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 0, 1);
        // A key whose declared length dwarfs the file.
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        let root = cache_with("bogus.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/bogus.gguf"))
            .expect_err("a bogus length is refused");
        assert!(matches!(error, LocalError::MalformedGguf { .. }));
    }

    #[test]
    fn entry_count_caps_are_enforced() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 0, MAX_METADATA_ENTRIES + 1);
        let root = cache_with("meta.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/meta.gguf"))
            .expect_err("an oversized metadata count is refused");
        assert!(error.to_string().contains("metadata count"));

        let mut bytes = Vec::new();
        push_prelude(&mut bytes, MAX_TENSORS + 1, 0);
        let root = cache_with("tensors.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/tensors.gguf"))
            .expect_err("an oversized tensor count is refused");
        assert!(error.to_string().contains("tensor count"));
    }

    #[test]
    fn deep_array_nesting_is_refused_not_a_stack_overflow() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 0, 1);
        push_string(&mut bytes, "some.key");
        bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
        // Each nested level is an element type plus a count of one.
        for _ in 0..MAX_ARRAY_DEPTH {
            bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
            bytes.extend_from_slice(&1u64.to_le_bytes());
        }
        let root = cache_with("deep.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/deep.gguf"))
            .expect_err("nesting past the cap is refused");
        assert!(error.to_string().contains("array nesting exceeds the cap"));
    }

    #[test]
    fn array_size_overflow_is_refused() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 0, 1);
        push_string(&mut bytes, "some.key");
        bytes.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
        bytes.extend_from_slice(&TYPE_UINT64.to_le_bytes());
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        let root = cache_with("overflow.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/overflow.gguf"))
            .expect_err("an overflowing array byte size is refused");
        assert!(error.to_string().contains("array size overflows"));
    }

    #[test]
    fn skipped_value_overrunning_the_file_is_refused() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 0, 1);
        push_string(&mut bytes, "some.key");
        bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
        // A skipped string may exceed the materialization cap, but never
        // the file itself.
        bytes.extend_from_slice(&1_000_000u64.to_le_bytes());
        let root = cache_with("overrun.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/overrun.gguf"))
            .expect_err("a skip past end-of-file is refused");
        assert!(error.to_string().contains("value overruns the file"));
    }

    /// One tensor-info record: name, the given dims, ggml type, offset.
    fn push_tensor(out: &mut Vec<u8>, dims: &[u64]) {
        push_string(out, "t");
        let dimension_count = u32::try_from(dims.len()).expect("test dims fit u32");
        out.extend_from_slice(&dimension_count.to_le_bytes());
        for dim in dims {
            out.extend_from_slice(&dim.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
    }

    #[test]
    fn tensor_dimension_cap_is_enforced() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 1, 0);
        push_string(&mut bytes, "t");
        bytes.extend_from_slice(&(MAX_TENSOR_DIMS + 1).to_le_bytes());
        let root = cache_with("dims.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/dims.gguf"))
            .expect_err("an oversized dimension count is refused");
        assert!(error.to_string().contains("tensor dimension count"));
    }

    #[test]
    fn tensor_shape_overflow_is_refused() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 1, 0);
        // Three maximal dims overflow even the u128 accumulator.
        push_tensor(&mut bytes, &[u64::MAX, u64::MAX, u64::MAX]);
        let root = cache_with("shape.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/shape.gguf"))
            .expect_err("an overflowing tensor shape is refused");
        assert!(error.to_string().contains("tensor shape overflows"));
    }

    #[test]
    fn parameter_count_past_u64_is_refused() {
        let mut bytes = Vec::new();
        push_prelude(&mut bytes, 1, 0);
        // The element count fits u128 but not the reported u64.
        push_tensor(&mut bytes, &[u64::MAX, 2]);
        let root = cache_with("params.gguf", &bytes);
        let error = read_model_info(root.path(), Path::new("models/params.gguf"))
            .expect_err("a parameter count past u64 is refused");
        assert!(error.to_string().contains("parameter count overflows"));
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        let root = cache_with("tiny.gguf", &synthetic_gguf());
        for escape in [
            Path::new("../outside.gguf"),
            Path::new("models/../../outside.gguf"),
        ] {
            assert!(matches!(
                read_model_info(root.path(), escape),
                Err(LocalError::UnsafeCachePath { .. })
            ));
        }
        let absolute = root.path().join("models").join("tiny.gguf");
        assert!(
            matches!(
                read_model_info(root.path(), &absolute),
                Err(LocalError::UnsafeCachePath { .. })
            ),
            "an absolute path is refused even when it points inside the cache"
        );
    }
}
