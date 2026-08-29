//! Small shared host-support primitives for the PromptForge runtime.
//!
//! [`untrusted`] wraps untrusted external data in a nonce-guarded envelope,
//! [`cancel`] is the cooperative cancellation handle and task-local scope a
//! run observes, and [`observe`] is the report-only vocabulary a run reports
//! its progress through. This crate depends on no other promptforge crate, so
//! every promptforge crate may depend on it.

pub mod cancel;
pub mod observe;
pub mod untrusted;
