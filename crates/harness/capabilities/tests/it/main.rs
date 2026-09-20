//! Activation integration suite: resolving a prompt's declared
//! capabilities against the registry, the run's services reaching
//! `create`, activation failure semantics, co-activation conflicts,
//! catalog assembly with prefix containment, and the harness's
//! activate-prepare-run path refusing an unsatisfiable prompt with the
//! engine's model-readable notice.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod activation;
mod assembly;
mod support;
