//! Compile-time proof of the auto traits the API promises: a broken
//! promise fails the build of this suite, not a run of it.

use promptforge::cancel::{CancelHandle, Cancelled};
use promptforge::event::Event;
use promptforge::tools::{ToolCatalog, ToolError};
use promptforge::vfs::{Access, VfsRef};
use promptforge::{Environment, Run, RunContext, RunError, RunErrorKind, RunLimits, RunResult};

const fn assert_send<T: Send>() {}
const fn assert_send_sync<T: Send + Sync>() {}
const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
const fn assert_unpin<T: Unpin>() {}

#[test]
fn a_run_can_move_between_threads_between_calls() {
    assert_send::<Run>();
}

#[test]
fn the_run_configuration_and_outcome_types_are_send_sync_and_static() {
    assert_send_sync_static::<Environment>();
    assert_send_sync_static::<RunContext>();
    assert_send_sync_static::<RunLimits>();
    assert_send_sync_static::<RunResult>();
    assert_send_sync_static::<RunError>();
    assert_send_sync_static::<RunErrorKind>();
}

#[test]
fn a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin() {
    assert_send_sync_static::<CancelHandle>();
    assert_unpin::<Cancelled>();
}

#[test]
fn events_and_the_tool_catalog_and_errors_cross_threads() {
    assert_send_sync::<Event>();
    assert_send_sync::<ToolCatalog>();
    assert_send_sync_static::<ToolError>();
}

#[test]
fn vfs_handles_and_accesses_are_send_and_sync() {
    assert_send_sync::<VfsRef>();
    assert_send_sync::<Access>();
}
