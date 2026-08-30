//! One embedding model for the whole test binary.
//!
//! Loading the model is seconds of CPU and many test modules need one - every
//! `PreparedTools::new` indexes the live tool catalog over it - so it is loaded
//! once behind a [`LazyLock`] and shared. That is sound rather than merely
//! cheap: the engine's own contract is that two indexes over one encoder embed
//! identical text to identical vectors, so sharing cannot change an answer. A
//! test that wants its own model would be measuring the loader, which the
//! engine's own suite already does.

use std::sync::LazyLock;

use promptforge_tool_picker::Model;

/// The test binary's one loaded model.
static MODEL: LazyLock<Model> =
    LazyLock::new(|| Model::load().expect("the compiled-in retrieval model loads"));

/// The shared model, for a test that builds a picker or an index itself.
pub(crate) fn model() -> &'static Model {
    &MODEL
}
