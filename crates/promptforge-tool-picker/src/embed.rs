//! The private sentence encoder, loaded from the compiled-in model.
//!
//! [`Encoder`] is the crate's one path from a string to a vector. Loading the
//! model is the expensive part, so it happens once and every later
//! [`Encoder::embed`] borrows the loaded model immutably.
//!
//! Every text takes the identical path: a capability need and a tool's enriched
//! text are encoded by the same code with no prefix and no per-role branch.
//! Pooling is CLS, the model's own convention. The weights are embedded as fp16
//! and loaded as f32.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::{Tokenizer, TruncationParams};

use crate::assets;
use crate::error::{ModelLoadError, QueryError};

/// The length of every vector [`Encoder::embed`] returns.
pub(crate) const EMBEDDING_DIMENSIONS: usize = 384;

/// The dtype the model runs in: f32, not the f16 the weights are stored as.
const COMPUTE_DTYPE: DType = DType::F32;

/// The compiled-in sentence encoder, loaded and ready to embed.
///
/// Construction parses and upcasts the whole checkpoint; [`Encoder::embed`]
/// takes `&self` and allocates only per call. The type is `Send + Sync`.
pub(crate) struct Encoder {
    /// Configured once, at construction, to truncate rather than to pad.
    tokenizer: Tokenizer,
    /// The BERT stack, held immutably and shared across calls.
    model: BertModel,
}

impl Encoder {
    /// Loads the compiled-in model.
    ///
    /// # Errors
    /// Returns [`ModelLoadError`] when the embedded configuration, tokenizer,
    /// or weights cannot be read, or the checkpoint's hidden size is not
    /// [`EMBEDDING_DIMENSIONS`].
    pub(crate) fn load() -> Result<Self, ModelLoadError> {
        verify_provenance()?;

        let config: BertConfig =
            serde_json::from_slice(assets::CONFIG_JSON).map_err(ModelLoadError::config)?;

        if config.hidden_size != EMBEDDING_DIMENSIONS {
            return Err(ModelLoadError::dimensions(
                config.hidden_size,
                EMBEDDING_DIMENSIONS,
            ));
        }

        let mut tokenizer =
            Tokenizer::from_bytes(assets::TOKENIZER_JSON).map_err(ModelLoadError::tokenizer)?;
        tokenizer.with_padding(None);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.max_position_embeddings,
                ..TruncationParams::default()
            }))
            .map_err(ModelLoadError::truncation)?;

        // `include_bytes!` data is 1-byte aligned, so the buffered loader, which
        // copies into an owned buffer, is the correct one.
        let weights = VarBuilder::from_buffered_safetensors(
            assets::WEIGHTS_SAFETENSORS.to_vec(),
            COMPUTE_DTYPE,
            &Device::Cpu,
        )
        .map_err(ModelLoadError::weights)?;

        let model = BertModel::load(weights, &config).map_err(ModelLoadError::model)?;

        Ok(Self { tokenizer, model })
    }

    /// Embeds one text as a unit-length vector of [`EMBEDDING_DIMENSIONS`] floats.
    ///
    /// # Errors
    /// Returns [`QueryError`] when the text cannot be tokenized, the forward
    /// pass fails, or the produced vector cannot be normalized.
    pub(crate) fn embed(&self, text: &str) -> Result<Vec<f32>, QueryError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(QueryError::tokenization)?;

        let token_ids = encoding.get_ids();
        if token_ids.is_empty() {
            return Err(QueryError::invalid_embedding("the text produced no tokens"));
        }

        let hidden = self.forward(token_ids)?;
        let mut vector = hidden.to_vec1::<f32>().map_err(QueryError::inference)?;

        if vector.len() != EMBEDDING_DIMENSIONS {
            return Err(QueryError::invalid_embedding(format!(
                "the model produced {} components, expected {EMBEDDING_DIMENSIONS}",
                vector.len()
            )));
        }

        l2_normalize(&mut vector)?;
        Ok(vector)
    }

    /// Runs the encoder over one token sequence and returns the CLS row.
    fn forward(&self, token_ids: &[u32]) -> Result<Tensor, QueryError> {
        let device = &self.model.device;
        Tensor::new(token_ids, device)
            .and_then(|ids| ids.unsqueeze(0))
            .and_then(|ids| {
                let token_types = ids.zeros_like()?;
                self.model.forward(&ids, &token_types, None)
            })
            .and_then(|hidden| hidden.get(0)?.get(0))
            .map_err(QueryError::inference)
    }
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("model", &"bge-small-en-v1.5")
            .field("dimensions", &EMBEDDING_DIMENSIONS)
            .finish_non_exhaustive()
    }
}

/// Checks that the embedded weights carry the pinned repository and revision.
///
/// The build script writes `{repo}@{revision}` into the fp16 safetensors
/// `__metadata__.source` field, and the same pin is embedded as
/// [`assets::SOURCE_REPO`] and [`assets::SOURCE_REVISION`]. Binding the two here
/// makes a mismatched, mixed, or substituted checkpoint a build defect that
/// fails loudly at load rather than silently altering every ranking.
fn verify_provenance() -> Result<(), ModelLoadError> {
    let blob = assets::WEIGHTS_SAFETENSORS;
    let length_bytes: [u8; 8] = blob
        .get(..8)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| {
            ModelLoadError::provenance("weights blob is too short for a safetensors header")
        })?;
    let header_len = usize::try_from(u64::from_le_bytes(length_bytes)).map_err(|_| {
        ModelLoadError::provenance("safetensors header length does not fit in usize")
    })?;
    let end = header_len
        .checked_add(8)
        .ok_or_else(|| ModelLoadError::provenance("safetensors header length overflows"))?;
    let header_bytes = blob
        .get(8..end)
        .ok_or_else(|| ModelLoadError::provenance("safetensors header is truncated"))?;
    let header: serde_json::Value = serde_json::from_slice(header_bytes).map_err(|error| {
        ModelLoadError::provenance(format!("safetensors header is not JSON: {error}"))
    })?;

    let source = header
        .get("__metadata__")
        .and_then(|metadata| metadata.get("source"))
        .and_then(serde_json::Value::as_str);
    let expected = format!("{}@{}", assets::SOURCE_REPO, assets::SOURCE_REVISION);
    if source == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(ModelLoadError::provenance(format!(
            "embedded weights declare source {source:?}, expected {expected:?}"
        )))
    }
}

/// Scales a vector to unit length in place.
fn l2_normalize(vector: &mut [f32]) -> Result<(), QueryError> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(QueryError::invalid_embedding(format!(
            "the pooled vector has length {norm}, which cannot be normalized"
        )));
    }
    for value in vector.iter_mut() {
        *value /= norm;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EMBEDDING_DIMENSIONS, Encoder};

    fn encoder() -> &'static Encoder {
        static ENCODER: std::sync::OnceLock<Encoder> = std::sync::OnceLock::new();
        ENCODER.get_or_init(|| Encoder::load().expect("the compiled-in model loads"))
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn an_embedding_has_the_models_dimensions_and_is_unit_length() {
        let vector = encoder().embed("read a file from disk").expect("embed");
        assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected unit norm, got {norm}");
    }

    #[test]
    fn the_same_text_embeds_to_the_same_vector() {
        let text = "create a pull request on a repository";
        assert_eq!(
            encoder().embed(text).expect("embed"),
            encoder().embed(text).expect("embed")
        );
    }

    #[test]
    fn similarity_tracks_meaning() {
        let near_a = encoder().embed("delete a file from disk").expect("embed");
        let near_b = encoder()
            .embed("remove a file from the disk")
            .expect("embed");
        let far = encoder()
            .embed("convert an amount between two currencies")
            .expect("embed");
        let near = cosine(&near_a, &near_b);
        let unrelated = cosine(&near_a, &far);
        assert!(near > 0.9, "two phrasings scored {near}");
        assert!(unrelated < near - 0.1, "unrelated scored {unrelated}");
    }

    #[test]
    fn empty_and_overlong_text_embed_rather_than_failing() {
        assert_eq!(
            encoder().embed("").expect("embed").len(),
            EMBEDDING_DIMENSIONS
        );
        let long = "fetch a web page and convert it to markdown ".repeat(160);
        assert_eq!(
            encoder().embed(&long).expect("embed").len(),
            EMBEDDING_DIMENSIONS
        );
    }

    #[test]
    fn the_golden_vector_has_not_moved() {
        const GOLDEN_TEXT: &str = "read the contents of a file from the local filesystem";
        const GOLDEN_PREFIX: [f32; 8] = [
            -0.040_741_92,
            0.021_011_36,
            0.012_813_352,
            -0.047_173_303,
            0.062_421_806,
            -0.102_996_57,
            0.018_370_535,
            -0.037_201_513,
        ];
        let vector = encoder().embed(GOLDEN_TEXT).expect("embed");
        for (index, expected) in GOLDEN_PREFIX.iter().enumerate() {
            assert!(
                (vector[index] - expected).abs() < 1e-4,
                "component {index} is {}, expected about {expected}",
                vector[index]
            );
        }
    }
}
