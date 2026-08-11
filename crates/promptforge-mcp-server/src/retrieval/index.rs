//! The ranking engine behind `need_prompt`, and the descriptors it ranks.
//!
//! The catalog becomes a tool catalog the engine understands: one descriptor per
//! runnable prompt, all on one server, each carrying the prompt's name, its
//! description, and the one-string `args` schema `run_prompt` takes. That is
//! the whole mapping - the engine never learns anything about a prompt a client
//! could not read for itself.
//!
//! The configuration departs from the engine's defaults in exactly one place,
//! the similarity floor, and [`crate::retrieval`] says why.

use promptforge_tool_picker::{Catalog as Descriptors, Config, ToolDescriptor, ToolId, ToolPicker};
use serde_json::json;

use crate::catalog::Catalog;
use crate::retrieval::{Candidate, Shortlist};

/// The server every descriptor names, since one server publishes them all.
///
/// The engine treats `(server, name)` as a tool's identity and uses the server
/// to tell one publisher's duplicate pair from two publishers' overlap. There is
/// one publisher here, so the value is a constant and that distinction never
/// arises.
const SERVER: &str = "promptforge";

/// The similarity a candidate must reach to be offered: the lowest the engine's
/// validated policy admits, which is zero. The engine's configuration domain
/// forbids a negative floor, so a candidate whose similarity to the capability
/// is negative is the one thing this floor still withholds; [`crate::retrieval`]
/// states that in the module contract.
const SIMILARITY_FLOOR: f32 = 0.0;

/// One built index: the engine over one catalog's descriptors.
///
/// Immutable. A catalog that changed calls for a new index over the same model,
/// which is what [`Index::rebuild`] makes.
#[derive(Debug)]
pub(crate) struct Index {
    /// The engine, holding the descriptors and their vectors.
    picker: ToolPicker,
}

impl Index {
    /// Loads the model and indexes `catalog`.
    ///
    /// `None` means the index could not be built, either because the model would
    /// not load or because the catalog could not be indexed; the engine's
    /// `BuildError` does not say which, so neither does the log. The reason is
    /// reported here because this is the only place that knows it; the caller's
    /// answer is to serve without retrieval.
    pub(super) fn build(catalog: &Catalog) -> Option<Index> {
        let config = compiled_config()?;
        match ToolPicker::build(descriptors(catalog), config) {
            Ok(picker) => Some(Index { picker }),
            Err(error) => {
                tracing::error!("need_prompt cannot answer: retrieval index build failed: {error}");
                None
            }
        }
    }

    /// Indexes `catalog` with a model somebody else already loaded.
    #[cfg(test)]
    pub(super) fn build_with(
        model: &promptforge_tool_picker::Model,
        catalog: &Catalog,
    ) -> Option<Index> {
        let config = compiled_config()?;
        match ToolPicker::build_with_model(model, descriptors(catalog), config) {
            Ok(picker) => Some(Index { picker }),
            Err(error) => {
                tracing::error!("the shared retrieval model could not index the catalog: {error}");
                None
            }
        }
    }

    /// A new index over `catalog`, sharing this one's loaded model.
    ///
    /// `None` means the rebuild failed and this index is still the best there
    /// is; the reason is reported here.
    pub(super) fn rebuild(&self, catalog: &Catalog) -> Option<Index> {
        match self.picker.rebuild(descriptors(catalog)) {
            Ok(picker) => Some(Index { picker }),
            Err(error) => {
                tracing::warn!("need_prompt keeps its previous index: {error}");
                None
            }
        }
    }

    /// How many prompts are indexed.
    pub(super) fn len(&self) -> usize {
        self.picker.len()
    }

    /// The best `k` prompts for `capability`, best first.
    pub(super) fn shortlist(&self, capability: &str, k: usize) -> Shortlist {
        match self.picker.shortlist(capability, k) {
            Ok(tools) => Shortlist::Candidates(tools.iter().map(candidate).collect()),
            Err(error) => Shortlist::Failed(error.to_string()),
        }
    }
}

/// A ranked descriptor as the caller reads it.
fn candidate(tool: &ToolDescriptor) -> Candidate {
    Candidate {
        name: tool.name().to_owned(),
        description: tool.description().to_owned(),
    }
}

/// The catalog as the engine reads it: one descriptor per runnable prompt.
///
/// A broken entry is left out. It cannot run, so recommending it would spend the
/// caller's next call on a certain failure, and it carries no description, so it
/// would be ranked on its name alone. `list_prompts` is where a broken prompt is
/// read, with the problem that stops it.
///
/// These three fields are the whole of what is embedded: the engine derives the
/// text from them itself, in [`ToolDescriptor::enriched_text`], and nothing here
/// spells that text a second time for the two spellings to drift apart. What
/// discriminates one prompt from another is therefore the name and the
/// description a caller already reads in `list_prompts`, since the schema is the
/// same for every prompt.
pub(crate) fn descriptors(catalog: &Catalog) -> Descriptors {
    catalog
        .entries()
        .iter()
        .filter(|entry| entry.problem().is_none())
        .map(|entry| {
            ToolDescriptor::new(
                ToolId::new(SERVER, entry.name()),
                entry.description(),
                args_schema(),
            )
        })
        .collect()
}

/// The one-string `args` schema a run takes.
///
/// The engine reads the top-level property names to enrich the text it embeds,
/// so this contributes the same word to every prompt and separates none of them
/// from another. It is here because the descriptor is meant to say what a call
/// really takes, and what a prompt takes is one raw string.
fn args_schema() -> serde_json::Value {
    json!({"type": "object", "properties": {"args": {"type": "string"}}})
}

/// The engine's configuration for this use, or `None` after logging why the
/// compiled-in policy is invalid.
///
/// The shortlist length is not set here: the query passes its own `k` (the
/// `CANDIDATES` bound in `crate::retrieval`) to `Index::shortlist`, which is the
/// one authoritative bound, so `top_k` in this configuration would never be
/// consulted. The only value this configuration departs from the engine's
/// defaults on is the similarity floor.
///
/// A failure is a violated compiled-in invariant rather than an expected error,
/// so it is logged with its real cause rather than silently discarded: a future
/// edit that pushes [`SIMILARITY_FLOOR`] out of the supported domain must not
/// disable retrieval without a trace.
fn compiled_config() -> Option<Config> {
    match Config::default().with_similarity_floor(SIMILARITY_FLOOR) {
        Ok(config) => Some(config),
        Err(error) => {
            tracing::error!(
                "need_prompt cannot answer: the compiled-in retrieval policy is invalid: {error}"
            );
            None
        }
    }
}
