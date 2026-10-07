//! Tests for the Host service vocabulary: ids by literal, the typed keys,
//! and the map's provide, get, and provides rules.

use std::collections::HashSet;
use std::sync::Arc;

use super::{HostServices, ServiceError, ServiceId, ServiceKey};

/// A text service, built in a `const` as every key is.
const GREETING: ServiceKey<str> = ServiceKey::new("acme/greeting");

/// A second key with the greeting's literal and another type.
const GREETING_AS_NUMBER: ServiceKey<u32> = ServiceKey::new("acme/greeting");

/// A static slice of ids, the shape `Package::needs` holds.
const NEEDS: &[ServiceId] = &[GREETING.id()];

#[test]
fn a_provided_service_is_found_under_its_key() {
    let mut services = HostServices::new();
    assert!(services.get(&GREETING).is_none());
    assert!(!services.provides(&GREETING.id()));

    services
        .provide(&GREETING, Arc::from("hello"))
        .expect("a valid, new id is accepted");
    assert_eq!(services.get(&GREETING).as_deref(), Some("hello"));
    assert!(services.provides(&GREETING.id()));
    assert!(services.provides(&NEEDS[0]));
}

#[test]
fn a_second_provider_under_a_provided_id_is_refused_and_the_first_stays() {
    let mut services = HostServices::new();
    services
        .provide(&GREETING, Arc::from("first"))
        .expect("the first provider is accepted");

    let error = services
        .provide(&GREETING, Arc::from("second"))
        .expect_err("a duplicate id is refused");
    assert!(
        matches!(
            error,
            ServiceError::DuplicateId {
                id: "acme/greeting"
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "a service with id acme/greeting is already provided"
    );

    let other_type = services
        .provide(&GREETING_AS_NUMBER, Arc::new(7))
        .expect_err("a duplicate literal is refused whatever its type");
    assert!(
        matches!(
            other_type,
            ServiceError::DuplicateId {
                id: "acme/greeting"
            }
        ),
        "{other_type:?}"
    );
    assert_eq!(services.get(&GREETING).as_deref(), Some("first"));
}

#[test]
fn an_id_outside_the_namespace_name_grammar_is_refused() {
    for literal in [
        "greeting",
        "acme/greeting/extra",
        "acme/",
        "Acme/Greeting",
        "acme/greet ing",
    ] {
        let key: ServiceKey<str> = ServiceKey::new(literal);
        let mut services = HostServices::new();
        let error = services
            .provide(&key, Arc::from("hello"))
            .expect_err("an unparseable id is refused");
        let ServiceError::InvalidId { id } = &error else {
            panic!("{literal}: the refusal names an invalid id: {error:?}");
        };
        assert_eq!(*id, literal);
        assert_eq!(
            error.to_string(),
            format!("service id {literal} is not a namespace/name id")
        );
        assert!(
            std::error::Error::source(&error).is_none(),
            "{literal}: the refusal carries no cause"
        );
        assert!(
            !services.provides(&key.id()),
            "{literal}: nothing is stored"
        );
    }
}

#[test]
fn a_service_id_is_a_global_name_with_exactly_one_slash() {
    for literal in ["acme/greeting/extra", "acme/greet ing", "greeting"] {
        let key: ServiceKey<str> = ServiceKey::new(literal);
        let mut services = HostServices::new();
        let error = services
            .provide(&key, Arc::from("hello"))
            .expect_err("a literal outside the service id grammar is refused");
        assert!(
            matches!(&error, ServiceError::InvalidId { id, .. } if *id == literal),
            "{literal}: {error:?}"
        );
        assert!(
            std::error::Error::source(&error).is_none(),
            "{literal}: the refusal carries no Plugin id parse failure: {error:?}"
        );
        assert!(
            !services.provides(&key.id()),
            "{literal}: nothing is stored"
        );
    }
    let mut services = HostServices::new();
    services
        .provide(
            &ServiceKey::<str>::new("acme.corp/greeting-v2"),
            Arc::from("hello"),
        )
        .expect("a two-segment literal is accepted");
}

#[test]
fn a_provider_under_the_right_id_with_the_wrong_type_is_not_found() {
    let mut services = HostServices::new();
    services
        .provide(&GREETING_AS_NUMBER, Arc::new(7))
        .expect("the number provider is accepted");

    assert!(
        services.get(&GREETING).is_none(),
        "get finds nothing when the stored type differs"
    );
    assert!(
        !services.provides(&GREETING.id()),
        "provides is false when the stored type differs"
    );
    assert_eq!(services.get(&GREETING_AS_NUMBER).as_deref(), Some(&7));
    assert!(services.provides(&GREETING_AS_NUMBER.id()));
}

#[test]
fn ids_compare_hash_and_display_by_their_literal() {
    assert_eq!(GREETING.id(), GREETING_AS_NUMBER.id());
    assert_ne!(GREETING.id(), ServiceKey::<str>::new("acme/farewell").id());
    let ids: HashSet<ServiceId> = [GREETING.id(), GREETING_AS_NUMBER.id()].into();
    assert_eq!(ids.len(), 1, "one literal hashes once");
    assert_eq!(GREETING.id().to_string(), "acme/greeting");
    assert_eq!(
        format!("{:?}", GREETING.id()),
        "ServiceId(\"acme/greeting\")"
    );
}

#[test]
fn a_clone_shares_the_same_providers() {
    let mut services = HostServices::new();
    let provider: Arc<str> = Arc::from("hello");
    services
        .provide(&GREETING, Arc::clone(&provider))
        .expect("the provider is accepted");
    let clone = services.clone();
    let found = clone.get(&GREETING).expect("the clone holds the provider");
    assert!(Arc::ptr_eq(&found, &provider));
}

#[test]
fn debug_lists_the_provided_ids_without_the_providers() {
    let mut services = HostServices::new();
    services
        .provide(&GREETING, Arc::from("secret greeting"))
        .expect("the provider is accepted");
    let shown = format!("{services:?}");
    assert!(shown.contains("acme/greeting"), "{shown}");
    assert!(!shown.contains("secret greeting"), "{shown}");
}
