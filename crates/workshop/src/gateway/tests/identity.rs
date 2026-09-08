//! Attachment and process-boot identity coverage.

use super::validated_gateway;
use crate::gateway::identity::{GatewayAttachment, same_gateway_identity};

#[test]
fn an_explicit_config_attachment_holds_no_local_sidecar_identity() {
    let gateway = validated_gateway("k");
    let identity = gateway.validate("k", 1_757_000_000, "2026-09-03T12:00:00Z");
    let sidecar = GatewayAttachment::Sidecar(identity.clone());
    assert!(
        sidecar
            .sidecar_identity()
            .is_some_and(|attached| attached.same_boot(&identity)),
        "a sidecar attachment retains its validated process identity"
    );
    let config = GatewayAttachment::Config;
    assert_eq!(
        config.sidecar_identity(),
        None,
        "a LAN Gateway from explicit config carries no local sidecar identity"
    );
}

#[test]
fn validated_process_boot_identity_distinguishes_live_gateway_children() {
    let first = validated_gateway("stable-key");
    let second = validated_gateway("stable-key");
    let original = first.validate("stable-key", 1_757_000_000, "2026-09-03T12:00:00Z");
    let replacement = second.validate("stable-key", 1_757_000_000, "2026-09-03T12:00:00Z");

    assert!(same_gateway_identity(&original, &original.clone()));
    assert!(
        !same_gateway_identity(&original, &replacement),
        "equal file-supplied boot metadata cannot alias another validated process"
    );
}
