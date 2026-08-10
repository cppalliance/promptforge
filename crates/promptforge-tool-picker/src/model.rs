//! The opaque, reusable model handle.
//!
//! [`Model`] is the only public model-lifecycle concept. Loading the model is
//! the expensive step, so a caller serving several catalogs loads one [`Model`]
//! and hands it to [`ToolPicker::build_with_model`](crate::ToolPicker::build_with_model)
//! or reuses a picker's model through
//! [`ToolPicker::rebuild`](crate::ToolPicker::rebuild). The concrete tokenizer,
//! tensor backend, and vector dimensions remain private.

use std::sync::Arc;

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
        Ok(Self {
            encoder: Arc::new(Encoder::load()?),
        })
    }

    /// Embeds one text with this model, for crate-internal indexing.
    pub(crate) fn embed(&self, text: &str) -> Result<Vec<f32>, QueryError> {
        self.encoder.embed(text)
    }

    /// Whether two handles share the same loaded encoder allocation.
    #[cfg(test)]
    pub(crate) fn shares_encoder(&self, other: &Model) -> bool {
        Arc::ptr_eq(&self.encoder, &other.encoder)
    }
}

#[cfg(test)]
mod tests {
    use super::Model;

    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn a_model_is_send_sync_static_and_clone_shares_the_encoder() {
        assert_send_sync_static::<Model>();
        let model = Model::load().expect("the compiled-in model loads");
        let clone = model.clone();
        assert!(model.shares_encoder(&clone), "cloning must not reload");
    }
}
