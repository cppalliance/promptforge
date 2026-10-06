//! Tests for the preflight report's missing services.

use promptforge_types::plugins::PluginId;

use super::{MissingService, PluginConflict, RequirementCheck, Requirements, UnmetRequirement};

fn id(text: &str) -> PluginId {
    PluginId::parse(text).expect("a static valid id")
}

fn missing_input(plugin: &str) -> MissingService {
    MissingService::new(id(plugin), "an input broker")
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
fn merge_drops_an_incoming_missing_plugin_already_named_as_lacking_a_service() {
    let mut requirements = Requirements::default();
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    let mut other = Requirements::default();
    other.missing_required.push(id("promptforge/user-input"));
    other.missing_required.push(id("acme/other"));
    requirements.merge(other);
    assert_eq!(requirements.missing_required, [id("acme/other")]);
    assert_eq!(
        requirements.missing_services,
        [missing_input("promptforge/user-input")]
    );
}

#[test]
fn merge_drops_a_missing_plugin_the_incoming_report_names_as_lacking_a_service() {
    let mut requirements = Requirements::default();
    requirements
        .missing_required
        .push(id("promptforge/user-input"));
    requirements.missing_required.push(id("acme/other"));
    let mut activation = Requirements::default();
    activation
        .missing_services
        .push(missing_input("promptforge/user-input"));
    requirements.merge(activation);
    assert_eq!(requirements.missing_required, [id("acme/other")]);
    assert_eq!(
        requirements.missing_services,
        [missing_input("promptforge/user-input")]
    );
}

#[test]
fn the_notice_names_the_plugin_and_the_service_it_lacks() {
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
fn the_notice_lists_missing_services_after_missing_plugins() {
    let mut requirements = Requirements::default();
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    requirements.missing_required.push(id("promptforge/web"));
    assert_eq!(
        requirements.notice(),
        "the environment cannot satisfy this prompt:\n\
         - missing required Plugin: promptforge/web\n\
         - promptforge/user-input needs an input broker, and this host provides none"
    );
}

#[test]
fn the_notice_lists_a_conflict_after_missing_services_and_before_unmet_requirements() {
    let mut requirements = Requirements::default();
    requirements.unmet_requirements.push(UnmetRequirement {
        role: "writer".to_owned(),
        check: RequirementCheck::ContextMinimum,
        required: "200000".to_owned(),
        actual: "32000".to_owned(),
    });
    requirements.conflicts.push(PluginConflict::new(
        id("promptforge/bashkit"),
        id("promptforge/terminal"),
    ));
    requirements
        .missing_services
        .push(missing_input("promptforge/user-input"));
    requirements.missing_required.push(id("promptforge/web"));
    assert_eq!(
        requirements.notice(),
        "the environment cannot satisfy this prompt:\n\
         - missing required Plugin: promptforge/web\n\
         - promptforge/user-input needs an input broker, and this host provides none\n\
         - conflicting Plugins: promptforge/bashkit and promptforge/terminal cannot be \
         activated together; declare one or the other\n\
         - role 'writer': requires a context of at least 200000 tokens; \
         the current model provides 32000"
    );
}
