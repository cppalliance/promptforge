//! Tests for the wire errors the route builds from session errors.

use super::session_error;
use crate::audio::AudioError;
use crate::realtime::session::SessionError;

#[test]
fn too_much_unfinalized_audio_names_the_retained_and_requested_durations_and_the_limit() {
    let error = session_error(
        &SessionError::Audio(AudioError::BufferTooLong {
            maximum_seconds: 30,
            retained_ms: 24_000,
            requested_ms: 12_000,
        }),
        Some("client_append".to_owned()),
    );

    assert_eq!(
        error.into_server_event("evt_audio_limit"),
        serde_json::json!({
            "event_id": "evt_audio_limit",
            "type": "error",
            "error": {
                "type": "overload_error",
                "code": "too_much_unfinalized_audio",
                "message": "Unfinalized audio exceeds 30 seconds: 24000 ms retained, 12000 ms requested",
                "param": "audio",
                "event_id": "client_append"
            }
        })
    );
}
