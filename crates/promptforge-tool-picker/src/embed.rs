//! Turning text into a comparable vector with the compiled-in model.
//!
//! [`Embedder`] is the crate's one path from a string to a vector. Loading the
//! model is by far the expensive part - tens of megabytes of weights parsed and
//! upcast - so it happens once, in [`Embedder::new`], and every later
//! [`Embedder::embed`] borrows the loaded model immutably. A caller therefore
//! builds one embedder and keeps it for the life of the process.
//!
//! # Every text takes the identical path
//!
//! A capability need and a tool's enriched text are encoded by the same code
//! with no prefix, no marker, and no per-role branch. bge publishes a
//! "Represent this sentence for searching relevant passages:" prefix for
//! *asymmetric* retrieval, where a short query is matched against long
//! documents. Tool resolution is symmetric - a one-line need against a one-line
//! tool description - and the study this engine is drawn from measured its
//! thresholds with no prefix. Adding one would shift every similarity and
//! invalidate those thresholds, so there is no prefix and no option to add one.
//!
//! # Pooling is CLS, not mean
//!
//! Pooling is a property of the model, not a preference. bge-small-en-v1.5 was
//! trained with the first token's hidden state as the sentence vector, so this
//! module takes that token and nothing else. Mean pooling over the same weights
//! produces vectors that still look plausible - unit length, sensible sign, a
//! believable spread of similarities - while ranking measurably worse. It is a
//! silent failure, which is why the choice is stated here rather than left to
//! read out of the code.
//!
//! # Storage dtype and compute dtype are different decisions
//!
//! The weights are embedded as fp16 to halve what each binary carries (see
//! [`crate::assets`]). They are loaded as f32: Candle's CPU coverage for f16 is
//! uneven, and the upcast happens once at load rather than per call, so it buys
//! speed and breadth at no per-embedding cost.

use std::sync::Arc;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::{Tokenizer, TruncationParams};

use crate::assets;
use crate::error::{ModelLoadError, ModelLoadErrorRepr};

/// The length of every vector [`Embedder::embed`] returns.
///
/// The hidden size of bge-small-en-v1.5. It is a constant rather than a
/// configurable because only one model is compiled in; [`Embedder::new`]
/// rejects a checkpoint whose hidden size disagrees with it.
pub(crate) const EMBEDDING_DIMENSIONS: usize = 384;

/// The dtype the model runs in.
///
/// f32, not the f16 the weights are stored as. See the module documentation for
/// why storage and compute are separate decisions.
const COMPUTE_DTYPE: DType = DType::F32;

/// The compiled-in sentence encoder, loaded and ready to embed.
///
/// Construct one with [`Embedder::new`] and share it: construction parses and
/// upcasts the whole checkpoint, while [`Embedder::embed`] takes `&self` and
/// allocates only per call. The type is `Send + Sync`, so one embedder behind
/// an `Arc` serves every thread.
///
/// The same text always yields the same vector: the model runs on the CPU in
/// f32 with no sampling, no dropout at inference, and no dependence on how many
/// texts are embedded together.
struct Backend {
    /// Configured once, at construction, to truncate rather than to pad.
    tokenizer: Tokenizer,
    /// The BERT stack, held immutably and shared across calls.
    model: BertModel,
}

/// A reusable handle to the compiled sentence model.
#[derive(Clone)]
#[non_exhaustive]
pub struct Model {
    backend: Arc<Backend>,
}

impl Model {
    /// Loads the compiled-in model.
    ///
    /// Reads no file, opens no socket, and consults no configuration: the
    /// weights, the tokenizer, and the architecture description are all bytes
    /// in this library. The cost is dominated by parsing the checkpoint and
    /// upcasting it to f32, which is why this is a separate step from
    /// embedding and why a caller should do it once.
    ///
    /// The tokenizer is set to truncate at the model's maximum sequence length
    /// and never to pad. Truncation means a pathologically long description
    /// yields a vector for its opening instead of an error. No padding means
    /// each text is run on its own exact token count, so a vector cannot depend
    /// on what else was embedded alongside it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelLoad`] if the embedded configuration, tokenizer,
    /// or weights cannot be read, or if the checkpoint's hidden size is not
    /// [`EMBEDDING_DIMENSIONS`]. Every one of these is a build defect rather
    /// than anything a caller can correct at run time.
    #[must_use = "loading a model has no effect unless the returned handle is used"]
    pub fn load() -> Result<Self, ModelLoadError> {
        let config: BertConfig = serde_json::from_slice(assets::CONFIG_JSON)
            .map_err(|error| ModelLoadError(ModelLoadErrorRepr::Config(Box::new(error))))?;

        if config.hidden_size != EMBEDDING_DIMENSIONS {
            return Err(ModelLoadError(ModelLoadErrorRepr::Dimensions));
        }

        let mut tokenizer = Tokenizer::from_bytes(assets::TOKENIZER_JSON)
            .map_err(|error| ModelLoadError(ModelLoadErrorRepr::Tokenizer(error)))?;
        tokenizer.with_padding(None);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.max_position_embeddings,
                ..TruncationParams::default()
            }))
            .map_err(|error| ModelLoadError(ModelLoadErrorRepr::Tokenizer(error)))?;

        // `include_bytes!` data is 1-byte aligned, so the buffered loader - which
        // copies into an owned buffer - is the correct one. The memory-mapped
        // loader needs a file, and the borrowed-slice loader is no better here.
        let weights = VarBuilder::from_slice_safetensors(
            assets::WEIGHTS_SAFETENSORS,
            COMPUTE_DTYPE,
            &Device::Cpu,
        )
        .map_err(|error| ModelLoadError(ModelLoadErrorRepr::Weights(Box::new(error))))?;

        let model = BertModel::load(weights, &config)
            .map_err(|error| ModelLoadError(ModelLoadErrorRepr::Architecture(Box::new(error))))?;

        Ok(Self {
            backend: Arc::new(Backend { tokenizer, model }),
        })
    }

    /// Embeds one text as a unit-length vector of [`EMBEDDING_DIMENSIONS`] floats.
    ///
    /// The vector is L2-normalized, so a dot product between two of them *is*
    /// their cosine similarity and ranking never has to divide by a magnitude.
    ///
    /// Text longer than the model's maximum sequence length is truncated, not
    /// rejected. Whitespace-only and empty text still embed: the tokenizer's
    /// special tokens carry them, and the result is the model's vector for an
    /// empty sentence rather than an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tokenize`] if the text cannot be tokenized, and
    /// [`Error::Embed`] if the forward pass fails or produces a vector that
    /// cannot be normalized because its length is zero or not finite.
    pub(crate) fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedFailure> {
        let encoding = self
            .backend
            .tokenizer
            .encode_fast(text, true)
            .map_err(EmbedFailure::Tokenization)?;

        let token_ids = encoding.get_ids();
        if token_ids.is_empty() {
            return Err(EmbedFailure::InvalidEmbedding);
        }

        let hidden = self.forward(token_ids)?;
        let mut vector = hidden.to_vec1::<f32>().map_err(EmbedFailure::Inference)?;

        if vector.len() != EMBEDDING_DIMENSIONS {
            return Err(EmbedFailure::InvalidEmbedding);
        }

        l2_normalize(&mut vector)?;
        Ok(vector)
    }

    /// Runs the encoder over one token sequence and returns the CLS row.
    ///
    /// No attention mask is passed because nothing is padded: every token in
    /// the sequence is real, which is exactly the all-ones mask the model
    /// assumes in its absence.
    fn forward(&self, token_ids: &[u32]) -> Result<Tensor, EmbedFailure> {
        let device = &self.backend.model.device;
        let pooled = Tensor::new(token_ids, device)
            .and_then(|ids| ids.unsqueeze(0))
            .and_then(|ids| {
                let token_types = ids.zeros_like()?;
                self.backend.model.forward(&ids, &token_types, None)
            })
            // The batch of one, then the first token of the sequence: CLS.
            .and_then(|hidden| hidden.get(0)?.get(0));

        pooled.map_err(EmbedFailure::Inference)
    }
}

impl std::fmt::Debug for Model {
    /// Names the model and its dimensions; the weights themselves are not
    /// printable in any useful way, and `BertModel` is not `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model").finish_non_exhaustive()
    }
}

/// Scales a vector to unit length in place.
///
/// # Errors
///
/// Returns [`Error::Embed`] if the vector's length is zero or not finite, which
/// cannot be scaled to one and would poison every later cosine comparison.
fn l2_normalize(vector: &mut [f32]) -> Result<(), EmbedFailure> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(EmbedFailure::InvalidEmbedding);
    }

    for value in vector.iter_mut() {
        *value /= norm;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EmbedFailure {
    #[error("tokenization")]
    Tokenization(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("inference")]
    Inference(#[source] candle_core::Error),
    #[error("invalid embedding")]
    InvalidEmbedding,
}

#[cfg(all(test, not(test)))]
mod tests {
    use super::{EMBEDDING_DIMENSIONS, Embedder};

    /// One embedder for the whole module's tests.
    ///
    /// Loading the checkpoint costs far more than any single test, and every
    /// test here wants the same immutable encoder.
    fn embedder() -> &'static Embedder {
        static EMBEDDER: std::sync::OnceLock<Embedder> = std::sync::OnceLock::new();
        EMBEDDER.get_or_init(|| Embedder::new().expect("the compiled-in model loads"))
    }

    /// The dot product of two vectors the embedder has already normalized.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn an_embedding_has_the_models_dimensions() {
        let vector = embedder().embed("read a file from disk").unwrap();
        assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
    }

    #[test]
    fn an_embedding_is_unit_length() {
        let vector = embedder().embed("send an email to a recipient").unwrap();
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit norm, got {norm} - a dot product is only a cosine when it is 1"
        );
    }

    #[test]
    fn the_same_text_embeds_to_the_same_vector() {
        let text = "create a pull request on a repository";
        let first = embedder().embed(text).unwrap();
        let second = embedder().embed(text).unwrap();
        assert_eq!(
            first, second,
            "resolution is deterministic only if embedding is"
        );
    }

    #[test]
    fn the_golden_vector_has_not_moved() {
        // Generated by running this code, not by hand: these are the first
        // eight components the embedder actually produced. They pin the whole
        // pipeline - tokenizer, weights, CLS pooling, normalization - so a
        // change in any of them fails here instead of silently re-ranking every
        // catalog. The tolerance is loose enough for a different CPU's
        // floating-point rounding and far too tight for a different vector.
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

        let vector = embedder().embed(GOLDEN_TEXT).unwrap();
        for (index, expected) in GOLDEN_PREFIX.iter().enumerate() {
            let actual = vector[index];
            assert!(
                (actual - expected).abs() < 1e-4,
                "component {index} is {actual}, expected about {expected}"
            );
        }
    }

    #[test]
    fn similarity_tracks_meaning() {
        let near_a = embedder().embed("delete a file from disk").unwrap();
        let near_b = embedder().embed("remove a file from the disk").unwrap();
        let far = embedder()
            .embed("convert an amount between two currencies")
            .unwrap();

        let near = cosine(&near_a, &near_b);
        let unrelated = cosine(&near_a, &far);

        assert!(
            near > 0.9,
            "two phrasings of one capability scored {near}, so the model is not reading them"
        );
        assert!(
            unrelated < near - 0.1,
            "unrelated text scored {unrelated} against a near-duplicate's {near}, \
             which is what a constant-output model would look like"
        );
        assert!(
            unrelated < 0.99,
            "every text scoring alike ({unrelated}) means the encoder is emitting a constant"
        );
    }

    #[test]
    fn text_far_over_the_sequence_limit_is_truncated_rather_than_rejected() {
        // Roughly 1,600 tokens against a 512-token limit. This is the slowest
        // test in the crate by an order of magnitude - a full-width forward
        // pass in an unoptimized build - and the cost is the 512 tokens the
        // model actually sees, not the ones thrown away.
        let long = "fetch a web page and convert it to markdown ".repeat(160);
        let vector = embedder().embed(&long).unwrap();
        assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn empty_text_embeds_rather_than_failing() {
        let vector = embedder().embed("").unwrap();
        assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
    }

    #[test]
    fn a_batch_matches_the_texts_embedded_one_at_a_time() {
        let texts = ["list the files in a directory", "run a shell command"];
        let batch = embedder().embed_all(texts).unwrap();
        assert_eq!(batch.len(), texts.len());
        for (vector, text) in batch.iter().zip(texts) {
            assert_eq!(*vector, embedder().embed(text).unwrap());
        }
    }

    #[test]
    fn an_embedder_can_be_shared_across_threads() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Embedder>();
    }
}
