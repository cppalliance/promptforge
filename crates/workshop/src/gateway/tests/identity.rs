//! Attachment and process-boot identity coverage.

use super::validated_gateway;
use crate::gateway::identity::{GatewayAttachment, same_gateway_identity};
use crate::gateway::supervisor::{RecoveryCandidate, RecoveryOwnership};

fn owned_candidate(identity: shared_sidecar::ValidatedConnection) -> RecoveryCandidate {
    match RecoveryCandidate::authenticate(identity.pid(), identity) {
        RecoveryOwnership::Owned(candidate) => candidate,
        RecoveryOwnership::Unowned(_) => panic!("the validated child pid authenticates ownership"),
    }
}

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

#[test]
fn server_publication_disarms_launched_cleanup_only_for_the_same_process_boot() {
    let mut launched = validated_gateway("launched-key");
    let identity = launched.validate("launched-key", 1_757_000_000, "2026-09-03T12:00:00Z");
    let attachment = GatewayAttachment::Launched(owned_candidate(identity.clone()));

    let attachment = attachment.reconcile_publication(Some(identity));
    drop(attachment);

    assert!(
        !launched.received_shutdown(std::time::Duration::from_millis(100)),
        "the child the server actually published remains authoritative"
    );
}

#[test]
fn server_publication_replaces_a_mismatched_candidate_and_cleans_it_up() {
    let mut launched = validated_gateway("launched-key");
    let published = validated_gateway("published-key");
    let launched_identity =
        launched.validate("launched-key", 1_757_000_000, "2026-09-03T12:00:00Z");
    let published_identity =
        published.validate("published-key", 1_757_000_001, "2026-09-03T12:00:01Z");
    let attachment = GatewayAttachment::Launched(owned_candidate(launched_identity));

    let attachment = attachment.reconcile_publication(Some(published_identity.clone()));

    assert!(
        attachment
            .sidecar_identity()
            .is_some_and(|identity| identity.same_boot(&published_identity)),
        "supervision follows the identity the server actually published"
    );
    assert!(
        launched.received_shutdown(std::time::Duration::from_secs(1)),
        "the unpublished owned child receives authenticated cleanup"
    );
}
