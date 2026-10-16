//! Fanout mechanics and failure semantics on the scheduler. Each submodule
//! holds one topic and reaches the shared helpers through `use super::*`.

use super::*;

mod arms;
mod failure_semantics;
mod scheduling;
