//! The gateway binding the client pushes across the door.

use std::path::PathBuf;

use harness_api::{GatewayBinding, Harness, HarnessConfig};

fn harness() -> Harness {
    Harness::new(HarnessConfig {
        agents_path: PathBuf::from("agents"),
        state_dir: PathBuf::from("state"),
    })
}

fn binding(generation: u64) -> GatewayBinding {
    GatewayBinding {
        base_url: format!("http://127.0.0.1:{}", 8000 + generation),
        key: format!("key-{generation}"),
        generation,
    }
}

#[test]
fn a_fresh_harness_has_no_gateway() {
    assert_eq!(harness().gateway(), None);
}

#[test]
fn set_gateway_called_twice_leaves_the_latest_generation() {
    let harness = harness();
    harness.set_gateway(binding(1));
    harness.set_gateway(binding(2));
    let current = harness.gateway().expect("a binding was set");
    assert_eq!(current.generation, 2);
    assert_eq!(current.base_url, "http://127.0.0.1:8002");
    assert_eq!(current.key, "key-2");
}

#[test]
fn a_gateway_binding_never_prints_its_key() {
    let rendered = format!("{:?}", binding(7));
    assert!(
        !rendered.contains("key-7"),
        "the bearer key leaked into Debug output: {rendered}"
    );
    assert!(
        rendered.contains("generation: 7"),
        "Debug output keeps the generation: {rendered}"
    );
}
