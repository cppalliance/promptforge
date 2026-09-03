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
    encoder: Arc<Encoder>,
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
            encoder: Arc::new(Encoder::load_with_progress(progress)?),
        })
    }

    /// Embeds one text with this model, for crate-internal indexing.
    pub(crate) fn embed(&self, text: &str) -> Result<Vec<f32>, QueryError> {
        self.encoder.embed(text)
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
        Arc::ptr_eq(&self.encoder, &other.encoder)
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
}
