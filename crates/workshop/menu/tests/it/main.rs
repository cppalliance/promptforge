//! Integration tests for `workshop-menu`: the registration contract -
//! the catalog sink, the workbench sink, and the menu's state handles
//! served through the registry's contribution collections.

use workshop_menu::{CatalogBus, MenuBus, MenuHandles, register};
use workshop_registry::{CatalogSink, MenuSink, Registry};

/// A catalog of chat models with the given ids.
fn models(ids: &[&str]) -> Vec<serde_json::Value> {
    ids.iter()
        .map(|id| serde_json::json!({ "id": id }))
        .collect()
}

#[test]
fn the_registered_sinks_drive_the_registered_buses() {
    let registry = Registry::new();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let _guards = register(&registry, &catalog, &menu);
    let catalog_sink = registry
        .sink::<dyn CatalogSink>()
        .expect("the catalog sink is registered");
    let menu_sink = registry
        .sink::<dyn MenuSink>()
        .expect("the menu sink is registered");

    catalog_sink.publish(models(&["model-a"]));
    assert_eq!(
        catalog.latest().expect("the catalog sink publishes").models,
        models(&["model-a"]),
        "the catalog sink publishes onto the caller's catalog bus"
    );

    menu_sink.set_profiles(vec!["main".to_owned()], Some("main".to_owned()));
    menu_sink.set_gateway_reachable(true);
    menu_sink.restore_selection();
    let ready = menu.latest().expect("the menu sink publishes");
    assert_eq!(ready.profiles, ["main"]);
    assert_eq!(ready.active.as_deref(), Some("main"));
    assert_eq!(
        ready.selected_model.as_deref(),
        Some("model-a"),
        "the restore selects from the catalog the sink published"
    );
    assert!(
        ready.chat_ready,
        "the reachability verdict reached the menu"
    );

    catalog_sink.publish(models(&["model-b"]));
    menu_sink.reconcile_catalog();
    assert_eq!(
        menu.latest()
            .expect("the reconcile republishes")
            .selected_model,
        None,
        "the reconcile clears a selection the new catalog lacks"
    );

    let handles = registry
        .state::<MenuHandles>()
        .expect("the menu's state handles are registered");
    assert_eq!(
        handles.catalog().latest().map(|push| push.models),
        Some(models(&["model-b"])),
        "the state handles share the caller's catalog bus"
    );
    assert_eq!(
        handles.menu().latest(),
        menu.latest(),
        "the state handles share the caller's menu bus"
    );
}

#[test]
fn dropping_the_guards_deregisters_every_contribution() {
    let registry = Registry::new();
    let catalog = CatalogBus::new();
    let guards = register(&registry, &catalog, &MenuBus::new(catalog.clone(), None));
    assert!(registry.sink::<dyn CatalogSink>().is_some());
    assert!(registry.sink::<dyn MenuSink>().is_some());
    assert!(registry.state::<MenuHandles>().is_some());
    drop(guards);
    assert!(registry.sink::<dyn CatalogSink>().is_none());
    assert!(registry.sink::<dyn MenuSink>().is_none());
    assert!(registry.state::<MenuHandles>().is_none());
}
