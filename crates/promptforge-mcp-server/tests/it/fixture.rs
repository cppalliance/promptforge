//! One embedding model for the whole integration-test binary.
//!
//! Loading the model is seconds of CPU and every prepared-tools boot needs
//! one, so it is loaded once behind a [`OnceLock`](std::sync::OnceLock) and
//! shared, exactly as the unit tests share theirs.

use std::sync::OnceLock;

use promptforge_tool_picker::Model;

/// The shared model, for a boot that prepares tools. The initialization lives
/// in this function rather than a static initializer because the workspace
/// clippy policy excuses `expect` only inside test function bodies.
// An `allow` rather than an `expect`: whether the lint fires here depends on
// how clippy classifies this fixture's closure under the build's cfg
// permutation, so an expectation would be unfulfilled in some builds and fail
// the -D warnings gate.
#[allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]
pub(crate) fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| Model::load().expect("the compiled-in retrieval model loads"))
}
