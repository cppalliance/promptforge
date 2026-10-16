//! Engine misuse no prompt author can cause: a poisoned store-conflict
//! slot still hands over the conflict it holds. Lifecycle-order breaks -
//! injecting twice, reading `var` before injection - do not compile.

use super::*;

#[test]
fn a_poisoned_store_conflict_slot_still_hands_over_its_conflict() {
    let vm = TestVm::new().build();
    let slot = vm.store_conflicts_handle();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut held = slot.lock().expect("the first lock is not poisoned");
        *held = Some("a.txt claimed by two identities".to_owned());
        panic!("poison the store-conflict slot");
    }));
    assert!(poisoned.is_err(), "the panic must poison the slot");
    assert_eq!(
        vm.take_store_conflict().as_deref(),
        Some("a.txt claimed by two identities"),
        "a poisoned slot must not drop the conflict it holds"
    );
    assert_eq!(vm.take_store_conflict(), None, "taking clears the slot");
    vm.teardown();
}
