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
mod options;
mod resolver;
mod transport;

pub use error::{CompletionError, CompletionErrorKind};
pub use options::{
    CompletionOptions, ModelBindOpts, ModelBinding, ModelInvocation, ModelSet, ModelView,
    Temperature, TemperatureError,
};
// The model identity/catalog vocabulary is canonical in
// `shared-promptforge-api` and re-exported here so existing
// `promptforge_model_client::model::` paths keep resolving.
pub use resolver::PickerModelResolver;
pub use shared_promptforge_api::models::{
    ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ModelIdError, ThinkingMode,
};
pub use transport::fetch_model_catalog;

/// Returns the descriptors satisfying `opts` as borrowed references.
///
/// This clones nothing (MODEL-017): the semantic resolver builds its picker
/// directly from these borrowed matches and selects the resolved descriptor
/// back out of the same borrowed slice.
///
/// `#[doc(hidden)]`: a cross-crate seam for the resolver and its test
/// doubles in `promptforge-api`, not host API. An extension trait because
/// [`ModelCatalog`] is canonical in `shared-promptforge-api` while
/// [`ModelBindOpts`] binding machinery stays here.
#[doc(hidden)]
pub trait ModelCatalogFiltered {
    /// Returns the descriptors satisfying `opts` as borrowed references.
    #[must_use]
    fn filtered(&self, opts: &ModelBindOpts) -> Vec<&ModelDescriptor>;
}

impl ModelCatalogFiltered for ModelCatalog {
    fn filtered(&self, opts: &ModelBindOpts) -> Vec<&ModelDescriptor> {
        self.models()
            .iter()
            .filter(|model| satisfies_constraints(model, opts))
            .collect()
    }
}

/// Builds a tool-picker [`Catalog`] from borrowed model descriptors.
///
/// The picker's `enriched_text` prefixes the tool name, so vendor model ids
/// must not ride in that name or they drown the capability description.
/// Identity is escaped into the picker id's first two segments; every entry
/// uses a single neutral, crate-private label as its name. Accepting borrowed
/// descriptors lets a filtered view build a picker without first cloning
/// matches into an owned catalog (MODEL-017).
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

/// Encodes a model identity as a picker id in the global naming grammar.
///
/// A picker id is a 3-segment global name (`namespace/pack/name`) over a
/// lowercase ASCII charset, while a model id component is nearly arbitrary
/// text, so each component rides in one of the first two segments escaped
/// byte-wise: charset bytes pass through and every other byte - including the
/// escape introducer `-` itself - is emitted as `-` plus two lowercase hex
/// digits. The name segment is the neutral label.
fn model_to_picker_id(id: &ModelId) -> PickerToolId {
    PickerToolId::from_validated(&format!(
        "{}/{}/{}",
        escape_segment(id.server()),
        escape_segment(id.name()),
        PICKER_MODEL_LABEL
    ))
}

/// Recovers the model identity encoded by [`model_to_picker_id`].
///
/// The escape is total, so a self-produced id always decodes; a foreign id
/// that does not decode falls back to its raw first two segments, mirroring
/// the pre-migration defensive path.
pub(crate) fn model_from_picker_id(id: &PickerToolId) -> ModelId {
    let capability = id.capability();
    match (
        unescape_segment(capability.namespace()),
        unescape_segment(capability.pack()),
    ) {
        (Some(server), Some(name)) => ModelId::from_validated(server, name),
        _ => ModelId::from_validated(capability.namespace(), capability.pack()),
    }
}

/// Encodes one model id component into a global-name segment; see
/// [`model_to_picker_id`] for the scheme.
fn escape_segment(component: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(component.len());
    for byte in component.bytes() {
        if matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.') {
            out.push(char::from(byte));
        } else {
            out.push('-');
            out.push(char::from(HEX[(byte >> 4) as usize]));
            out.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    out
}

/// Decodes one segment produced by [`escape_segment`]. Returns `None` for a
/// segment that is not escape output (a dangling `-` or non-UTF-8 bytes).
fn unescape_segment(segment: &str) -> Option<String> {
    fn hex_digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'-' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
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
