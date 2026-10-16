//! Engine misuse no prompt author can cause: lifecycle-order breaks fail as
//! internal errors, and a poisoned store-conflict slot still hands over
//! the conflict it holds.

use super::*;

#[test]
fn injecting_engine_values_twice_is_an_internal_error() {
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("the first injection succeeds");
    let error = vm
        .inject_values("", &json!({}), &fresh_access())
        .expect_err("a second injection is refused");
    assert!(
        matches!(&error, Error::Internal(message) if message.contains("already injected")),
        "a double injection is Engine misuse, not an authoring error: {error:?}"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn reading_var_before_injection_is_an_internal_error() {
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    let error = vm
        .var()
        .expect_err("var has no data table before injection");
    assert!(
        matches!(&error, Error::Internal(message) if message.contains("not been injected")),
        "a lifecycle-order break is Engine misuse, not an authoring error: {error:?}"
    );
}

#[test]
fn a_poisoned_store_conflict_slot_still_hands_over_its_conflict() {
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
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
}
