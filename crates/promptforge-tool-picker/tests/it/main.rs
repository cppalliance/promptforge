//! The crate's integration tests, as one binary.
//!
//! Every area is a module here rather than its own file directly under
//! `tests/`, because each such file is a separate test binary and each binary
//! relinks the whole library - which carries the embedding model, tens of
//! megabytes of it. One binary links once and every area shares the result.
//!
//! - [`behavior`] drives the four outcomes end to end over a committed fixture.
//! - [`public_api`] exercises only what a dependent crate can reach.

mod api;
mod behavior;
mod public_api;
