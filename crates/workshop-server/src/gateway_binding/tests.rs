use super::*;

mod atomic;

fn validated_connection(
    gateway: &crate::test_gateway::ValidatedGateway,
    api_key: &str,
    epoch: u64,
    started_at: &str,
) -> shared_sidecar::ValidatedConnection {
    gateway.validate(api_key, epoch, started_at)
}

#[test]
fn capability_replacement_publishes_one_coherent_snapshot() {
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let old = binding.snapshot();
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let port = gateway.port();
    let validated =
        validated_connection(&gateway, "new-key", 1_778_000_001, "2026-09-07T18:00:01Z");

    binding
        .updater()
        .replace_sidecar(&validated)
        .expect("replacement builds");
    let new = binding.snapshot();

    assert_eq!(old.client().base_url, "http://127.0.0.1:54375");
    assert_eq!(old.client().api_key, "old-key");
    assert_eq!(old.base_url(), "http://127.0.0.1:54375");
    assert_eq!(old.api_key(), "old-key");
    assert!(
        old.identity.is_none(),
        "configured gateways have no local identity"
    );
    assert_eq!(new.client().base_url, format!("http://127.0.0.1:{port}"));
    assert_eq!(new.client().api_key, "new-key");
    assert_eq!(new.base_url(), format!("http://127.0.0.1:{port}"));
    assert_eq!(new.api_key(), "new-key");
    assert!(
        format!("{:?}", new.model_client().expect("the model client builds"))
            .contains(&format!("http://127.0.0.1:{port}/v1")),
        "the model client carries the same endpoint"
    );
    let identity = new
        .identity
        .as_ref()
        .expect("the validated identity is published");
    assert_eq!(identity, &validated);
    assert!(new.generation() > old.generation());
    assert_eq!(binding.generation(), new.generation());
}

#[test]
fn same_port_and_key_new_boot_still_publishes_a_new_identity() {
    let gateway = crate::test_gateway::ValidatedGateway::spawn("stable-key");
    let first = validated_connection(
        &gateway,
        "stable-key",
        1_778_000_001,
        "2026-09-07T18:00:01Z",
    );
    let replacement = validated_connection(
        &gateway,
        "stable-key",
        1_778_000_002,
        "2026-09-07T18:00:02Z",
    );
    let binding =
        GatewayBinding::new("http://127.0.0.1:54375", "stable-key").expect("binding builds");
    binding
        .updater()
        .replace_sidecar(&first)
        .expect("the first identity publishes");
    let first_snapshot = binding.snapshot();
    binding
        .updater()
        .replace_sidecar(&replacement)
        .expect("the replacement identity publishes");
    let replacement_snapshot = binding.snapshot();

    assert_eq!(replacement_snapshot.base_url(), first_snapshot.base_url());
    assert_eq!(replacement_snapshot.api_key(), first_snapshot.api_key());
    assert!(
        replacement_snapshot.generation() > first_snapshot.generation(),
        "identity replacement advances the generation even with a stable endpoint and bearer"
    );
    assert_eq!(
        replacement_snapshot.identity.as_ref(),
        Some(&replacement),
        "the snapshot carries the new validated boot"
    );
}
