//! Operation-scoped progress reporting for PromptForge processes.
//!
//! The owner of one operation creates a [`ProgressTree`] on the process-wide
//! [`ProgressHub`], registers every leaf up front with a weight proportional
//! to its share of the operation's expected duration, and reports through the
//! [`ProgressHandle`] it gets back. Renderers subscribe to the hub's event
//! stream or pull snapshots; producers never format output. The crate never
//! spawns tasks and never blocks: hosts own their forwarding and renderer
//! tasks.

mod event;
mod handle;
mod hub;
mod remote;
mod render;
mod tree;

pub use crate::event::{EventState, OperationId, ProgressEvent};
pub use crate::handle::ProgressHandle;
pub use crate::hub::ProgressHub;
pub use crate::remote::RemoteOperation;
pub use crate::render::{NodeSnapshot, OperationSnapshot, ProgressMeter};
pub use crate::tree::ProgressTree;

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProgressHandle>();
    assert_send_sync::<ProgressTree>();
};
