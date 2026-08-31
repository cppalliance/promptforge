//! GGUF header inspection: architecture, layer count, parameter count, and
//! optional chat template read from metadata without touching tensor data.
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

/// Ceiling for a captured `tokenizer.chat_template`. Real templates can be
/// tens of kilobytes, while a one-mebibyte bound rejects hostile lengths.
const MAX_CHAT_TEMPLATE_BYTES: u64 = 1024 * 1024;

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
    /// The exact `tokenizer.chat_template` string when the metadata contains
    /// one with the expected string type.
    pub chat_template: Option<String>,
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
        self.read_string_capped(what, MAX_STRING_BYTES)
    }

    /// Reads a GGUF string with a field-specific byte ceiling.
    fn read_string_capped(&mut self, what: &str, cap: u64) -> Result<String, LocalError> {
        let length = self.read_u64()?;
        if length > cap {
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
        let mut chat_template: Option<String> = None;
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
            } else if key == "tokenizer.chat_template" && value_type == TYPE_STRING {
                chat_template =
                    Some(self.read_string_capped("chat template", MAX_CHAT_TEMPLATE_BYTES)?);
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
            chat_template,
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
mod tests;
