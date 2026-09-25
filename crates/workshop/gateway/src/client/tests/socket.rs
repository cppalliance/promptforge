//! Realtime socket tests: the gateway base URL upgrades to its matching
//! WebSocket scheme, and any other scheme is refused before connecting.

use super::*;

use crate::client::socket::realtime_url;

#[test]
fn an_http_gateway_upgrades_to_ws() {
    let url = realtime_url("http://127.0.0.1:8081").expect("http upgrades");
    assert_eq!(
        url.as_str(),
        "ws://127.0.0.1:8081/v1/realtime?intent=transcription"
    );
}

#[test]
fn an_https_gateway_upgrades_to_wss() {
    let url = realtime_url("https://gateway.example/prefix").expect("https upgrades");
    assert_eq!(
        url.as_str(),
        "wss://gateway.example/prefix/v1/realtime?intent=transcription"
    );
}

#[test]
fn a_gateway_on_any_other_scheme_is_refused() {
    for (base_url, scheme) in [
        ("ftp://gateway.example", "ftp"),
        ("ws://127.0.0.1:8081", "ws"),
    ] {
        let Err(error) = realtime_url(base_url) else {
            panic!("{base_url} must not upgrade");
        };
        let GatewayError::Transport(source) = &error else {
            panic!("a refused scheme is a transport error, got {error:?}");
        };
        assert!(
            source.to_string().contains(&format!("{scheme:?}")),
            "the refusal names the scheme, got {source}"
        );
    }
}
