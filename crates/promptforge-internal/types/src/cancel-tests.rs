//! Tests for `CancelHandle`: idempotence, parent-to-child propagation, and waker behavior.

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use super::CancelHandle;

/// How many wakers wait on `handle`'s own node.
fn registered(handle: &CancelHandle) -> usize {
    handle.inner.wakers.lock().unwrap().len()
}

/// A waker that counts its wakes, so a test can tell a cancel woke the
/// waiter from the waiter merely re-polling.
#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Counter {
    fn wakes(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Compile-time proof that a handle can cross thread boundaries and live for
/// the whole program: the Harness moves one into every performer task, and
/// the Engine keeps one in `RunContext` while `Run` itself is `Send`.
const fn _assert_auto_traits() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<CancelHandle>();
}

#[test]
fn a_fresh_handle_is_not_cancelled_and_its_clones_share_one_flag() {
    let a = CancelHandle::new();
    let b = CancelHandle::default();
    let c = a.clone();
    assert!(!a.is_cancelled() && !b.is_cancelled() && !c.is_cancelled());
    a.cancel();
    assert!(
        a.is_cancelled() && c.is_cancelled(),
        "clones share the flag"
    );
    assert!(!b.is_cancelled(), "an unrelated handle is untouched");
}

#[test]
fn cancel_is_idempotent_and_irreversible() {
    let handle = CancelHandle::new();
    handle.cancel();
    handle.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn a_child_observes_its_parents_cancel() {
    let parent = CancelHandle::new();
    let child = parent.child();
    assert!(!child.is_cancelled());
    parent.cancel();
    assert!(child.is_cancelled(), "parent cancel reaches the child");
}

#[test]
fn a_parent_does_not_observe_its_childs_cancel() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let sibling = parent.child();
    child.cancel();
    assert!(child.is_cancelled());
    assert!(!parent.is_cancelled(), "child cancel never reaches up");
    assert!(
        !sibling.is_cancelled(),
        "child cancel never reaches siblings"
    );
    // The sibling still tracks the parent afterwards.
    parent.cancel();
    assert!(sibling.is_cancelled());
}

#[test]
fn cloning_a_child_shares_the_childs_flag_not_the_parents() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let child_clone = child.clone();
    child.cancel();
    assert!(child_clone.is_cancelled());
    assert!(!parent.is_cancelled());
}

#[test]
fn cancel_propagates_down_a_grandchild_chain() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let grandchild = child.child();
    parent.cancel();
    assert!(child.is_cancelled() && grandchild.is_cancelled());
}

#[test]
fn a_middle_cancel_reaches_below_but_not_above() {
    let root = CancelHandle::new();
    let middle = root.child();
    let leaf = middle.child();
    middle.cancel();
    assert!(leaf.is_cancelled(), "a leaf observes an ancestor's cancel");
    assert!(
        !root.is_cancelled(),
        "the root never observes a descendant's cancel"
    );
}

#[test]
fn a_child_of_a_cancelled_parent_is_born_cancelled() {
    let parent = CancelHandle::new();
    parent.cancel();
    assert!(parent.child().is_cancelled());
    assert!(parent.child().child().is_cancelled());
}

#[test]
fn a_cancel_on_one_thread_is_observed_on_another() {
    // The Harness cancels from its supervisor while the Engine polls the
    // flag from whichever thread `step` happens to run on.
    let parent = CancelHandle::new();
    let child = parent.child();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let poller = thread::spawn(move || {
        ready_tx.send(()).expect("main thread is waiting");
        while !child.is_cancelled() {
            thread::yield_now();
        }
        done_tx.send(()).expect("main thread is waiting");
    });
    ready_rx.recv().expect("poller signals readiness");
    assert!(!parent.is_cancelled());
    parent.cancel();
    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the polling thread observes the cancel");
    poller.join().expect("poller exits cleanly");
}

#[test]
fn a_parents_cancel_wakes_a_waiter_on_its_child() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let counter = Arc::new(Counter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let mut waiting = pin!(child.cancelled());
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);
    // Re-polling registers the same waker once more, not twice.
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(counter.wakes(), 0, "nothing woke the waiter yet");
    parent.cancel();
    assert_eq!(
        counter.wakes(),
        1,
        "the cancel woke the waiter exactly once"
    );
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_childs_cancel_does_not_wake_a_waiter_on_its_parent() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let counter = Arc::new(Counter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let mut waiting = pin!(parent.cancelled());
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);
    child.cancel();
    assert_eq!(counter.wakes(), 0, "a child's cancel never reaches up");
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);
    parent.cancel();
    assert_eq!(counter.wakes(), 1);
    assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_waiter_dropped_before_cancel_leaves_no_waker_on_any_ancestor() {
    let root = CancelHandle::new();
    let middle = root.child();
    let leaf = middle.child();
    let counter = Arc::new(Counter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let mut waiting = leaf.cancelled();
    assert_eq!(Pin::new(&mut waiting).poll(&mut cx), Poll::Pending);
    assert_eq!(registered(&root), 1, "a poll registers up the chain");
    drop(waiting);
    for (node, handle) in [("leaf", &leaf), ("middle", &middle), ("root", &root)] {
        assert_eq!(
            registered(handle),
            0,
            "the {node} node still holds the dropped waiter's waker"
        );
    }
    root.cancel();
    assert_eq!(counter.wakes(), 0, "a dropped waiter is never woken");
}

#[test]
fn dropping_one_of_two_waiters_polled_by_one_task_leaves_the_other_woken() {
    let run = CancelHandle::new();
    let task = run.child();
    let counter = Arc::new(Counter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let mut first = task.cancelled();
    let mut second = task.cancelled();
    assert_eq!(Pin::new(&mut first).poll(&mut cx), Poll::Pending);
    assert_eq!(Pin::new(&mut second).poll(&mut cx), Poll::Pending);
    drop(first);
    run.cancel();
    assert_eq!(
        counter.wakes(),
        1,
        "the surviving waiter's registration outlives its sibling's drop"
    );
    assert_eq!(Pin::new(&mut second).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_waiter_re_polled_with_a_new_waker_is_woken_through_the_new_waker_only() {
    let run = CancelHandle::new();
    let task = run.child();
    let old = Arc::new(Counter::default());
    let new = Arc::new(Counter::default());
    let old_waker = Waker::from(Arc::clone(&old));
    let new_waker = Waker::from(Arc::clone(&new));
    let mut waiting = task.cancelled();
    assert_eq!(
        Pin::new(&mut waiting).poll(&mut Context::from_waker(&old_waker)),
        Poll::Pending
    );
    assert_eq!(
        Pin::new(&mut waiting).poll(&mut Context::from_waker(&new_waker)),
        Poll::Pending
    );
    run.cancel();
    assert_eq!(old.wakes(), 0, "the replaced waker is not woken");
    assert_eq!(new.wakes(), 1, "the latest waker is woken once");
}

#[test]
fn a_waiter_on_a_cancelled_handle_is_ready_at_its_first_poll() {
    let handle = CancelHandle::new();
    handle.cancel();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(pin!(handle.cancelled()).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn debug_output_names_the_flag_state() {
    let handle = CancelHandle::new();
    let before = format!("{handle:?}");
    assert!(before.contains("cancelled: false"), "{before}");
    handle.cancel();
    let after = format!("{handle:?}");
    assert!(after.contains("cancelled: true"), "{after}");
}
