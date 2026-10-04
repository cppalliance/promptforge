//! Activation integration suite: resolving a prompt's declared
//! Plugins against the registry, the run's services reaching
//! `create`, activation failure semantics, co-activation conflicts,
//! catalog assembly with prefix containment, declared service needs, the
//! activated Plugins' preludes, and the Harness's
//! activate-prepare-run path refusing an unsatisfiable prompt with the
//! Engine's model-readable notice.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod activation;
mod assembly;
mod needs;
mod preludes;
mod support;
