//! The agent socket's rendering of a refused launch: the error frame's
//! text includes the refusal's cause chain, not just its outermost
//! message.

use std::io;

use workshop_agents::LaunchError;

use super::*;

#[test]
fn a_refused_launch_frame_includes_the_cause_text() {
    let cause = "agents directory is locked by another process";
    let refusal = LaunchRefusal::Refused(LaunchError::Unreadable {
        source: io::Error::new(io::ErrorKind::PermissionDenied, cause),
    });

    let rendered = refusal_text(&refusal);

    assert!(
        rendered.contains("agent session state unavailable"),
        "the refusal's own message is missing: {rendered}"
    );
    assert!(
        rendered.contains(cause),
        "the cause text is missing from the frame: {rendered}"
    );
}

#[test]
fn an_unusable_gateway_renders_its_own_message_alone() {
    assert_eq!(
        refusal_text(&LaunchRefusal::GatewayUnusable),
        "agent sessions need a usable gateway binding; check the gateway base URL and key"
    );
}
