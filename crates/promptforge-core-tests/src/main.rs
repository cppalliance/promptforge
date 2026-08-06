//! Explicit model-test entry point plus offline parse, declaration, and execution fixtures.
//!
//! The binary remains intentionally inert until the model runner is added. Run
//! `cargo test -p promptforge-core-tests` for the offline fixture harness.

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "step 6 provisions artifacts for the step 7 model runner"
    )
)]
mod artifacts;

#[cfg(test)]
mod suite;

fn main() {}
