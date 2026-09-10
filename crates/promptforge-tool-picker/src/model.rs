//! The opaque, reusable model handle.
//!
//! [`Model`] is the only public model-lifecycle concept. Loading the model is
//! the expensive step, so a caller serving several catalogs loads one [`Model`]
//! and hands it to [`ToolPicker::build_with_model`](crate::ToolPicker::build_with_model)
//! or reuses a picker's model through
//! [`ToolPicker::rebuild`](crate::ToolPicker::rebuild). The concrete tokenizer,
//! tensor backend, and vector dimensions remain private.

use std::sync::Arc;

use shared_progress::ProgressHandle;

use crate::embed::Encoder;
use crate::error::{ModelLoadError, QueryError};

/// A loaded, reusable sentence-embedding model.
///
/// Cheap to clone: cloning shares the loaded weights rather than reloading
/// them. The internal backend is replaceable behind this opaque handle.
///
/// # Examples
///
/// ```no_run
/// use promptforge_tool_picker::Model;
///
/// let model = Model::load()?;
/// let reused = model.clone();
/// # let _ = reused;
/// # Ok::<(), promptforge_tool_picker::ModelLoadError>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Model {
    /// The loaded encoder, shared cheaply across clones and pickers.
    encoder: Option<Arc<Encoder>>,
}

impl Model {
    /// Loads the compiled-in model once.
    ///
    /// # Errors
    /// Returns [`ModelLoadError`] when the compiled-in configuration,
    /// tokenizer, or weights cannot be turned into a usable encoder.
    #[must_use = "loading the model is the expensive step; keep the handle to reuse it"]
    pub fn load() -> Result<Self, ModelLoadError> {
        Self::load_with_progress(None)
    }

    /// Loads the compiled-in model once, reporting progress through `progress`.
    ///
    /// The byte-measurable stage is the copy of the compiled-in weights into
    /// an aligned buffer: the leaf advances per copied chunk and completes
    /// when the copy finishes. A `None` handle loads without reporting,
    /// exactly as [`Model::load`] does.
    ///
    /// # Errors
    /// Returns [`ModelLoadError`] when the compiled-in configuration,
    /// tokenizer, or weights cannot be turned into a usable encoder.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::Model;
    ///
    /// let model = Model::load_with_progress(None)?;
    /// # Ok::<(), promptforge_tool_picker::ModelLoadError>(())
    /// ```
    #[must_use = "loading the model is the expensive step; keep the handle to reuse it"]
    pub fn load_with_progress(progress: Option<&ProgressHandle>) -> Result<Self, ModelLoadError> {
        Ok(Self {
            encoder: Some(Arc::new(Encoder::load_with_progress(progress)?)),
        })
    }

    /// A handle with no loaded encoder, for pickers that never embed.
    ///
    /// Embedding through it falls back to the deterministic hasher, which a
    /// picker over an empty catalog never needs: with no indexed rows every
    /// query abstains before the vector matters.
    pub(crate) fn unloaded() -> Self {
        Self { encoder: None }
    }

    /// Creates an empty dummy model that performs no weight loading or inference.
    ///
    /// Suitable for test fixtures with empty catalogs or where semantic
    /// resolution is not exercised.
    #[cfg(feature = "test-fixtures")]
    #[must_use]
    pub fn dummy() -> Self {
        Self::unloaded()
    }

    /// Embeds one text with this model, for crate-internal indexing.
    pub(crate) fn embed(&self, text: &str) -> Result<Vec<f32>, QueryError> {
        if let Some(encoder) = &self.encoder {
            encoder.embed(text)
        } else {
            let mut vector = vec![0.0f32; crate::embed::EMBEDDING_DIMENSIONS];
            let trimmed = text.trim();
            if trimmed.is_empty() {
                vector[0] = 1.0;
                return Ok(vector);
            }
            for word in trimmed.split_whitespace() {
                let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
                for byte in word.as_bytes() {
                    hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "embedding dimensions fit in usize"
                )]
                let idx = (hash % (crate::embed::EMBEDDING_DIMENSIONS as u64)) as usize;
                let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };
                vector[idx] += sign;
            }
            let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm <= 0.0 || !norm.is_finite() {
                vector[0] = 1.0;
            } else {
                for v in &mut vector {
                    *v /= norm;
                }
            }
            Ok(vector)
        }
    }

    /// Whether two handles share the same loaded encoder allocation.
    ///
    /// A test seam, public only under the `test-fixtures` feature so a
    /// consumer's test binary can assert that a shared model was not
    /// reloaded.
    #[cfg(feature = "test-fixtures")]
    #[doc(hidden)]
    #[must_use]
    pub fn shares_encoder(&self, other: &Model) -> bool {
        match (&self.encoder, &other.encoder) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use shared_progress::{EventState, ProgressHub};

    use super::Model;

    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn a_model_is_send_sync_static_and_clone_shares_the_encoder() {
        assert_send_sync_static::<Model>();
        let model = Model::load().expect("the compiled-in model loads");
        let clone = model.clone();
        assert!(model.shares_encoder(&clone), "cloning must not reload");
    }

    #[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
    #[test]
    fn load_with_progress_reports_the_weights_copy_in_byte_steps() {
        let hub = Arc::new(ProgressHub::new());
        let mut events = hub.subscribe();
        let tree = hub.operation();
        let leaf = tree.register("load-model", 1.0);
        assert!(matches!(
            events.try_recv().expect("register emits Begun").state,
            EventState::Begun { .. }
        ));

        let model = Model::load_with_progress(Some(&leaf)).expect("the compiled-in model loads");
        drop(model);

        let first = events.try_recv().expect("the first chunk reports");
        assert!(
            matches!(first.state, EventState::Updated { fraction } if fraction > 0.0 && fraction < 1.0),
            "the first chunk is a partial byte fraction, got {:?}",
            first.state
        );
        assert_eq!(leaf.fraction(), 1.0, "the copy completes the leaf");
        let mut saw_finished = false;
        while let Ok(event) = events.try_recv() {
            saw_finished |= matches!(event.state, EventState::Finished { ok: true });
        }
        assert!(saw_finished, "completion emits Finished");
    }

    #[test]
    fn dummy_model_shares_encoder_and_embeds_deterministically() {
        let dummy1 = Model::dummy();
        let dummy2 = Model::dummy();
        assert!(dummy1.shares_encoder(&dummy2));
        let vec1 = dummy1.embed("some text").expect("embed succeeds");
        let vec2 = dummy2.embed("some text").expect("embed succeeds");
        assert_eq!(vec1, vec2);
        assert_eq!(vec1.len(), crate::embed::EMBEDDING_DIMENSIONS);
        let norm = vec1.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
