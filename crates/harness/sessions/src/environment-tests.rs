use super::*;

fn binding(generation: u64) -> GatewayBinding {
    GatewayBinding {
        base_url: format!("http://127.0.0.1:{}", 8000 + generation),
        key: format!("key-{generation}"),
        generation,
    }
}

#[test]
fn a_generation_change_rebuilds_the_registry_and_client() {
    let bindings = Bindings::new();
    assert!(bindings.set_gateway(binding(1)), "the first push builds");
    let first = bindings.gateway().expect("resources exist after a push");
    assert_eq!(first.generation(), 1);
    assert!(
        first.registry().is_some(),
        "a valid binding builds the registry"
    );
    assert!(
        first.client().is_some(),
        "a valid binding builds the client"
    );

    assert!(
        bindings.set_gateway(binding(2)),
        "a new generation rebuilds"
    );
    let second = bindings.gateway().expect("resources exist after a rebuild");
    assert_eq!(second.generation(), 2);
    assert_eq!(second.binding().base_url, "http://127.0.0.1:8002");
    assert!(
        !Arc::ptr_eq(
            first.registry().expect("first registry"),
            second.registry().expect("second registry")
        ),
        "the registry is a fresh build, not the first generation's"
    );
    assert_eq!(
        *bindings.subscribe_gateway().borrow(),
        Some(2),
        "the watch carries the rebuilt generation"
    );
}

#[test]
fn a_repeated_generation_keeps_the_built_resources() {
    let bindings = Bindings::new();
    assert!(bindings.set_gateway(binding(3)));
    let built = bindings.gateway().expect("resources exist");
    assert!(
        !bindings.set_gateway(GatewayBinding {
            base_url: "http://127.0.0.1:9999".to_owned(),
            ..binding(3)
        }),
        "the same generation is the client's word that nothing changed"
    );
    let kept = bindings.gateway().expect("resources still exist");
    assert!(
        Arc::ptr_eq(&built, &kept),
        "no rebuild happened for a repeated generation"
    );
}

#[test]
fn an_unusable_binding_leaves_its_resources_absent() {
    let resources = GatewayResources::build(GatewayBinding {
        base_url: "not a url".to_owned(),
        key: String::new(),
        generation: 1,
    });
    assert!(
        resources.registry().is_none(),
        "no registry from a bad root"
    );
    assert!(resources.client().is_none(), "no client from an empty key");
}

#[test]
fn a_gateway_binding_never_prints_its_key() {
    let rendered = format!("{:?}", binding(7));
    assert!(
        !rendered.contains("key-7"),
        "the bearer key leaked into Debug output: {rendered}"
    );
    assert!(rendered.contains("generation: 7"));
}

#[test]
fn the_host_snapshot_serves_the_first_root_and_the_selection() {
    let host = HostSnapshot {
        selected_model: Some("gpt".to_owned()),
        workspace_roots: vec![PathBuf::from("/w/one"), PathBuf::from("/w/two")],
    };
    let ui = host.ui();
    assert_eq!(ui["selected_model"], "gpt");
    assert_eq!(
        ui["workspace_root"],
        PathBuf::from("/w/one").display().to_string()
    );
    let empty = HostSnapshot::default().ui();
    assert!(empty["selected_model"].is_null());
    assert!(empty["workspace_root"].is_null());
}

#[tokio::test]
async fn no_selection_and_no_catalog_binds_no_model_without_a_fetch() {
    // No selection and an empty catalog: nothing to resolve, so nothing
    // is fetched from the (unreachable) gateway and the roles stay
    // unbound.
    let model = current_model(
        &HostSnapshot::default(),
        Some(&CatalogBinding::default()),
        &binding(1),
    )
    .await
    .expect("no fetch is attempted");
    assert!(model.is_none());
}
