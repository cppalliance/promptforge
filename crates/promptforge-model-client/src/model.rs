//! Prompt-local model bindings: catalog, bind/use declarations, and invocation.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a pinned
//! offline entry). H1 `models.bind` resolves a description against that catalog
//! under hard constraints, freezes invocation parameters, and stores the result
//! in the host's run-scoped model bindings. H2 `models.use` selects at most
//! one binding per
//! section; H1 `models.default` supplies the prompt-wide default for sections
//! that omit `models.use`. Model-facing sections with neither binding fail with
//! a model-binding failure surfaced through the host's run error.

use std::num::NonZeroU32;

use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId};
use serde_json::Value;

use crate::Result;

mod error;
mod ids;
mod options;
mod resolver;
mod transport;

pub use error::{CompletionError, CompletionErrorKind};
pub use ids::{ModelCatalogError, ModelId, ModelIdError};
pub use options::{
    CompletionOptions, ModelBindOpts, ModelBinding, ModelDescriptor, ModelInvocation, ModelSet,
    ModelView, Temperature, TemperatureError, ThinkingMode,
};
pub use resolver::PickerModelResolver;
pub use transport::{fetch_model_catalog, subscribe_progress};

/// Complete live model set for one bind pass.
///
/// `#[non_exhaustive]` so the collision-free catalog invariant is only ever
/// established through [`ModelCatalog::new`]/[`ModelCatalog::empty`].
// No `Eq`: bindings carry `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors in host order.
    ///
    /// # Errors
    /// Returns [`ModelCatalogError::DuplicateId`] when two descriptors share one
    /// stable [`ModelId`], so an ambiguous catalog is unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use promptforge_model_client::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    ///
    /// let ctx = NonZeroU32::new(8_192).ok_or("context is non-zero")?;
    /// let id = ModelId::gateway("small")?;
    /// let catalog = ModelCatalog::new([ModelDescriptor::new(
    ///     id.clone(),
    ///     "A tiny model",
    ///     ctx,
    ///     ThinkingMode::Never,
    /// )])?;
    /// assert!(catalog.contains(&id));
    /// assert_eq!(catalog.models().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        models: impl IntoIterator<Item = ModelDescriptor>,
    ) -> std::result::Result<ModelCatalog, ModelCatalogError> {
        let models: Vec<ModelDescriptor> = models.into_iter().collect();
        for (index, model) in models.iter().enumerate() {
            if models[..index].iter().any(|prior| prior.id() == model.id()) {
                return Err(ModelCatalogError::DuplicateId {
                    server: model.id().server().to_owned(),
                    name: model.id().name().to_owned(),
                });
            }
        }
        Ok(Self { models })
    }

    /// Builds a catalog from descriptors already known to be collision-free.
    ///
    /// Used by internal callers whose inputs are already validated, where
    /// duplicate checking is redundant.
    pub(crate) fn from_validated(models: Vec<ModelDescriptor>) -> ModelCatalog {
        Self { models }
    }

    /// An empty catalog; every `models.bind` resolves as absent.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_validated(Vec::new())
    }

    /// Returns every descriptor.
    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    /// Returns whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Looks up a descriptor by stable identity.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.iter().find(|model| model.id() == id)
    }

    /// Returns whether the catalog contains a descriptor with `id`.
    #[must_use]
    pub fn contains(&self, id: &ModelId) -> bool {
        self.get(id).is_some()
    }

    /// Returns the descriptors satisfying `opts` as borrowed references.
    ///
    /// This clones nothing (MODEL-017): the semantic resolver builds its picker
    /// directly from these borrowed matches and selects the resolved descriptor
    /// back out of the same borrowed slice.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the resolver and its test
    /// doubles in `promptforge-core`, not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn filtered(&self, opts: &ModelBindOpts) -> Vec<&ModelDescriptor> {
        self.models
            .iter()
            .filter(|model| satisfies_constraints(model, opts))
            .collect()
    }
}

/// Builds a tool-picker [`Catalog`] from borrowed model descriptors.
///
/// The picker's `enriched_text` prefixes the tool name, so vendor model ids
/// must not ride in that name or they drown the capability description.
/// Identity is encoded in the picker id's server field; every entry uses a
/// single neutral, crate-private label. Accepting borrowed descriptors lets a
/// filtered view build a picker without first cloning matches into an owned
/// catalog (MODEL-017).
pub(crate) fn picker_catalog_from<'a>(
    models: impl IntoIterator<Item = &'a ModelDescriptor>,
) -> Catalog {
    Catalog::new(
        models
            .into_iter()
            .map(|model| {
                ToolDescriptor::new(
                    model_to_picker_id(model.id()),
                    model.description().to_owned(),
                    Value::Object(serde_json::Map::new()),
                )
            })
            .collect(),
    )
}

/// Neutral picker name so `enriched_text` does not inject vendor model ids.
const PICKER_MODEL_LABEL: &str = "model";

/// Separates server and model name inside the picker's server field.
const PICKER_ID_SEPARATOR: char = '\u{1e}';

fn model_to_picker_id(id: &ModelId) -> PickerToolId {
    PickerToolId::new(
        format!("{}{}{}", id.server(), PICKER_ID_SEPARATOR, id.name()),
        PICKER_MODEL_LABEL,
    )
}

pub(crate) fn model_from_picker_id(id: &PickerToolId) -> ModelId {
    match id.server().split_once(PICKER_ID_SEPARATOR) {
        Some((server, name)) if !server.is_empty() && !name.is_empty() => {
            ModelId::from_validated(server, name)
        }
        _ => ModelId::from_validated(id.server(), id.name()),
    }
}

/// Resolves one `models.bind` description under optional hard constraints.
pub trait ModelResolver: Send + Sync {
    /// Resolves `description` with `opts` to a binding identity and invocation.
    ///
    /// # Errors
    /// Returns the crate's binding error when the capability cannot be
    /// resolved uniquely or no catalog entry satisfies the constraints.
    fn resolve(&self, description: &str, opts: &ModelBindOpts) -> Result<ResolvedModel>;
}

impl<F> ModelResolver for F
where
    F: Fn(&str, &ModelBindOpts) -> Result<ResolvedModel> + Send + Sync,
{
    fn resolve(&self, description: &str, opts: &ModelBindOpts) -> Result<ResolvedModel> {
        self(description, opts)
    }
}

/// The identity and invocation produced by a successful model resolve.
// No `Eq`: the invocation carries an `f64` temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModel {
    /// The selected catalog identity.
    pub id: ModelId,
    /// Frozen per-request fields from the bind's opts.
    pub invocation: ModelInvocation,
    /// The catalog context window size in tokens (always non-zero).
    pub context: NonZeroU32,
}

fn satisfies_constraints(model: &ModelDescriptor, opts: &ModelBindOpts) -> bool {
    if let Some(min_context) = opts.context
        && model.context() < min_context
    {
        return false;
    }
    match opts.thinking {
        Some(true) => matches!(
            model.thinking(),
            ThinkingMode::Switchable | ThinkingMode::Always
        ),
        Some(false) => matches!(
            model.thinking(),
            ThinkingMode::Switchable | ThinkingMode::Never
        ),
        None => true,
    }
}

#[cfg(test)]
mod tests;
