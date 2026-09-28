//! Tests for the preflight report's missing host services.

use promptforge_types::capabilities::CapabilityId;

use super::{MissingService, Requirements};

fn id(text: &str) -> CapabilityId {
    CapabilityId::parse(text).expect("a static valid id")
}

fn missing_input(capability: &str) -> MissingService {
    MissingService::new(id(capability), "an input broker")
}

#[test]
fn a_missing_service_leaves_the_report_unsatisfied() {
    let mut requirements = Requirements::default();
    assert!(requirements.is_satisfied());
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    assert!(!requirements.is_satisfied());
    assert!(requirements.refusal().is_some());
}

#[test]
fn merge_folds_in_missing_services_without_repeating_one() {
    let mut requirements = Requirements::default();
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    let mut activation = Requirements::default();
    activation
        .missing_services
        .push(missing_input("promptforge/user-input"));
    activation
        .missing_services
        .push(missing_input("acme/asker"));
    requirements.merge(activation);
    assert_eq!(
        requirements.missing_services,
        [
            missing_input("promptforge/user-input"),
            missing_input("acme/asker"),
        ]
    );
}

#[test]
fn the_notice_names_the_capability_and_the_service_it_lacks() {
    let mut requirements = Requirements::default();
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    assert_eq!(
        requirements.notice(),
        "the environment cannot satisfy this prompt:\n\
         - promptforge/user-input needs an input broker, and this host provides none"
    );
}

#[test]
fn the_notice_lists_missing_services_after_missing_capabilities() {
    let mut requirements = Requirements::default();
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    requirements.missing_required.push(id("promptforge/web"));
    assert_eq!(
        requirements.notice(),
        "the environment cannot satisfy this prompt:\n\
         - missing required capability: promptforge/web\n\
         - promptforge/user-input needs an input broker, and this host provides none"
    );
}
