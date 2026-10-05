//! Local error classification and source-chain tests.

use super::*;
use std::error::Error as _;

fn io() -> io::Error {
    io::Error::other("boom")
}

#[test]
fn is_retryable_classifies_transient_versus_permanent() {
    // Transient: transport, process liveness, port contention.
    assert!(LocalError::TeardownTimeout.is_retryable());
    assert!(
        LocalError::EarlyExit {
            status: "signal: 9".to_owned()
        }
        .is_retryable()
    );
    assert!(LocalError::ReadinessTimeout { seconds: 180 }.is_retryable());
    assert!(
        LocalError::Spawn {
            executable: PathBuf::from("llama-server"),
            source: io(),
        }
        .is_retryable()
    );
    assert!(LocalError::Kill { source: io() }.is_retryable());
    // Permanent: integrity, config, validation, and misuse.
    assert!(!LocalError::StartupInterrupted.is_retryable());
    assert!(!LocalError::Cancelled.is_retryable());
    assert!(
        !LocalError::DigestMismatch {
            name: "m".to_owned(),
            expected: "a".to_owned(),
            actual: "b".to_owned(),
        }
        .is_retryable()
    );
    assert!(
        !LocalError::RespawnCooldown {
            model: "m".to_owned()
        }
        .is_retryable()
    );
    assert!(!LocalError::Capture { stream: "stdout" }.is_retryable());
}

#[test]
fn transport_and_json_variants_reach_their_causes_through_the_shared_wrappers() {
    let Err(transport) = reqwest::Proxy::all("http://") else {
        panic!("a proxy URL with an empty host must not build");
    };
    let client = LocalError::HttpClient(transport.into());
    let Some(cause) = client.source() else {
        panic!("the http-client variant returns its transport cause from source()");
    };
    let Some(wrapper) = cause.downcast_ref::<HttpSource>() else {
        panic!("the transport cause is the shared HttpSource");
    };
    assert!(wrapper.as_inner().is_builder());

    let Err(json) = serde_json::from_str::<u32>("nope") else {
        panic!("`nope` must not parse as a u32");
    };
    let decode = LocalError::DialectDecode {
        operation: "GET /props",
        source: json.into(),
    };
    let Some(cause) = decode.source() else {
        panic!("the dialect-decode variant returns its JSON cause from source()");
    };
    let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
        panic!("the JSON cause is the shared JsonSource");
    };
    assert!(wrapper.as_inner().is_syntax());
}

#[test]
fn source_bearing_variants_preserve_their_cause_without_doubling_display() {
    let spawn = LocalError::Spawn {
        executable: PathBuf::from("llama-server"),
        source: io(),
    };
    assert!(spawn.source().is_some());

    // A wrapped readiness failure is preserved as `source()`, and the outer
    // Display renders only the wrapper message (no doubled chain).
    let startup = LocalError::Startup {
        detail: "invocation + diagnostics".to_owned(),
        source: Box::new(LocalError::ReadinessTimeout { seconds: 5 }),
    };
    assert!(startup.source().is_some());
    assert!(startup.to_string().contains("invocation + diagnostics"));
    assert!(!startup.to_string().contains("did not expose"));
}
