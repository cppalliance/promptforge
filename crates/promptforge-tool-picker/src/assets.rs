//! The embedding model, compiled into this library.
//!
//! The build script fetches `BAAI/bge-small-en-v1.5` from the Hugging Face Hub
//! at one pinned commit, verifies each file against a hardcoded SHA-256 digest,
//! downcasts the weights to fp16, and stages the result in `OUT_DIR`. The
//! statics below embed those bytes, so a binary linking this crate carries
//! the model outright: no weights file to ship, no download at run time, and no
//! configuration naming a path.
//!
//! # Build requirements
//!
//! The *first* build needs network access to the Hugging Face Hub and pulls
//! roughly 130MB. Later builds read the Hugging Face cache
//! (`HF_HUB_CACHE`, or `HF_HOME`, default `~/.cache/huggingface`) and do not
//! touch the network. If the Hub is unreachable and the cache is cold, the
//! build fails loudly rather than producing a library with no model.
//!
//! # Precision
//!
//! The embedded weights are fp16 purely to halve what each binary carries. That
//! is a *storage* choice, not a compute choice: nothing here claims fp16 is the
//! right dtype to run inference in, and a loader is expected to upcast to f32 in
//! memory if that is faster or better covered on the target.

/// The fp16 model weights, as a safetensors blob.
///
/// Downcast from the upstream fp32 weights at build time; tensor names and
/// shapes are unchanged, and any non-fp32 tensor is carried through as-is.
///
/// A `static` rather than a `const`: a `const` of this size is materialized
/// afresh at every use site, which would undo the downcast's whole purpose.
pub static WEIGHTS_SAFETENSORS: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model-fp16.safetensors"));

/// The tokenizer, as a Hugging Face `tokenizer.json` document.
///
/// Byte-identical to the upstream file at [`SOURCE_REVISION`].
pub static TOKENIZER_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tokenizer.json"));

/// The model architecture configuration, as a `config.json` document.
///
/// Byte-identical to the upstream file at [`SOURCE_REVISION`].
pub static CONFIG_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/config.json"));

/// The Hugging Face repository the embedded assets came from.
pub const SOURCE_REPO: &str = "BAAI/bge-small-en-v1.5";

/// The immutable commit the embedded assets were taken from.
///
/// A commit, not a branch: a branch is mutable and would let the upstream
/// repository silently change what this crate embeds.
///
/// Embedded from `OUT_DIR` rather than written here, so the build script that
/// fetched the bytes is the only place the pin appears and the two cannot drift.
pub const SOURCE_REVISION: &str = include_str!(concat!(env!("OUT_DIR"), "/revision.txt"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_embedded_at_a_plausible_size() {
        // The upstream fp32 blob is ~133MB, so the fp16 downcast should be
        // ~67MB. A wide window still catches an empty or truncated include.
        let len = WEIGHTS_SAFETENSORS.len();
        assert!(
            (40_000_000..100_000_000).contains(&len),
            "embedded weights are {len} bytes, which is not a plausible fp16 bge-small blob"
        );
    }

    #[test]
    fn weights_are_a_safetensors_blob() {
        // 8-byte little-endian header length, then a JSON object.
        let header_len = u64::from_le_bytes(
            WEIGHTS_SAFETENSORS[..8]
                .try_into()
                .expect("slice of 8 bytes"),
        );
        let header_len = usize::try_from(header_len).expect("header fits in usize");
        let header: serde_json::Value =
            serde_json::from_slice(&WEIGHTS_SAFETENSORS[8..8 + header_len])
                .expect("safetensors header is JSON");
        assert!(
            header.get("embeddings.word_embeddings.weight").is_some(),
            "embedded weights do not look like a BERT checkpoint"
        );

        // The build script downcasts every fp32 tensor and carries every other
        // dtype through untouched. Both halves of that are load-bearing, so
        // check one tensor of each kind.
        assert_eq!(
            header["embeddings.word_embeddings.weight"]["dtype"], "F16",
            "a float tensor is not fp16, so the downcast did not happen"
        );
        assert_eq!(
            header["embeddings.position_ids"]["dtype"], "I64",
            "an integer tensor is not I64, so the non-float passthrough is broken"
        );
    }

    #[test]
    fn tokenizer_and_config_parse_as_json() {
        let tokenizer: serde_json::Value =
            serde_json::from_slice(TOKENIZER_JSON).expect("tokenizer.json parses");
        assert!(tokenizer.get("model").is_some());

        let config: serde_json::Value =
            serde_json::from_slice(CONFIG_JSON).expect("config.json parses");
        assert_eq!(config["model_type"], "bert");
        assert_eq!(config["hidden_size"], 384);
    }

    #[test]
    fn provenance_is_a_pinned_commit() {
        assert_eq!(SOURCE_REVISION.len(), 40);
        assert!(SOURCE_REVISION.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
