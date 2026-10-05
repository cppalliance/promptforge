//! Local upstream recovery tests.

use super::*;

fn transport_err() -> ProtocolError {
    ProtocolError::transport(std::io::Error::other("original transport failure"))
}

#[test]
fn recovery_triggers_on_connect_and_transport_but_not_protocol() {
    // A dead child looks the same whether the connection was refused or
    // died mid-flight: both transport variants trigger recovery, while a
    // protocol failure means the child answered and must not respawn.
    let connect = ProtocolError::connect(std::io::Error::other("refused"));
    assert!(is_transport_failure(&connect));
    assert!(is_transport_failure(&transport_err()));
    let protocol = ProtocolError::upstream_protocol(std::io::Error::other("bad json"));
    assert!(!is_transport_failure(&protocol));
}

#[tokio::test]
async fn map_recovery_reply_covers_every_branch() {
    // Respawned -> retry.
    assert!(matches!(
        map_recovery_reply(Ok(Ok(true)), transport_err()),
        RecoveryOutcome::Retry
    ));
    // Still-alive child (no respawn) -> return the original transport error.
    assert!(matches!(
        map_recovery_reply(Ok(Ok(false)), transport_err()),
        RecoveryOutcome::Failed(ProtocolError::UpstreamTransport(..))
    ));
    // Recovery error -> wrapped as a transport error.
    assert!(matches!(
        map_recovery_reply(Ok(Err(LocalError::TeardownTimeout)), transport_err()),
        RecoveryOutcome::Failed(ProtocolError::UpstreamTransport(..))
    ));
    // Dropped recovery reply -> synthesized transport error, never a hang.
    let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<bool, LocalError>>();
    drop(tx);
    let dropped = rx.await;
    match map_recovery_reply(dropped, transport_err()) {
        RecoveryOutcome::Failed(ProtocolError::UpstreamTransport(source, ..)) => {
            assert!(
                source.to_string().contains("dropped before reporting"),
                "unexpected message: {source}"
            );
        }
        _ => panic!("dropped recovery reply must yield a transport failure"),
    }
}
