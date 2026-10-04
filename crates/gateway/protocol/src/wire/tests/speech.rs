//! Tests for the speech request body's validation and closed vocabularies.

use super::*;

fn speech_request(model: &str, input: &str) -> SpeechRequest {
    SpeechRequest {
        model: model.to_owned(),
        input: input.to_owned(),
        voice: SpeechVoice::Name("tara".to_owned()),
        response_format: SpeechResponseFormat::Mp3,
        speed: None,
        instructions: None,
        stream_format: None,
        rest: Map::new(),
    }
}

#[test]
fn speech_request_validation_table() {
    assert!(speech_request("  ", "hi").validate().is_err());
    assert!(speech_request("m", "").validate().is_err());
    // The wire cap is 4096 characters: 4096 passes, 4097 fails.
    assert!(speech_request("m", &"x".repeat(4096)).validate().is_ok());
    assert!(speech_request("m", &"x".repeat(4097)).validate().is_err());
    // The unit is characters, not bytes: U+00E9 is two UTF-8 bytes, so
    // 4096 of them are 8192 bytes and still pass, 4097 fail.
    assert!(
        speech_request("m", &"\u{e9}".repeat(4096))
            .validate()
            .is_ok()
    );
    assert!(
        speech_request("m", &"\u{e9}".repeat(4097))
            .validate()
            .is_err()
    );
    // Speed outside 0.25..=4.0 is rejected; both bounds are accepted.
    let too_slow = SpeechRequest {
        speed: Some(0.24),
        ..speech_request("m", "hi")
    };
    assert!(too_slow.validate().is_err());
    let too_fast = SpeechRequest {
        speed: Some(4.01),
        ..speech_request("m", "hi")
    };
    assert!(too_fast.validate().is_err());
    let at_bounds = SpeechRequest {
        speed: Some(0.25),
        ..speech_request("m", "hi")
    };
    assert!(at_bounds.validate().is_ok());
    let at_top = SpeechRequest {
        speed: Some(4.0),
        ..speech_request("m", "hi")
    };
    assert!(at_top.validate().is_ok());
    assert!(speech_request("m", "hi").validate().is_ok());
}

#[test]
fn speech_request_rejects_unknown_response_format() {
    // The format set is closed: an unknown spelling fails deserialization,
    // so no route can forward one. Together's `raw` is a known provider
    // spelling deliberately excluded from the enum; covering it here pins
    // the exclusion rather than leaving it incidental.
    for format in ["ogg", "raw"] {
        let json = serde_json::json!({
            "model": "m",
            "input": "hi",
            "voice": "tara",
            "response_format": format,
        });
        assert!(
            serde_json::from_value::<SpeechRequest>(json).is_err(),
            "response_format {format:?} must be rejected"
        );
    }
}

#[test]
fn speech_request_rejects_unknown_stream_format() {
    // The framing set is closed: an unknown spelling fails deserialization,
    // so no route can forward one.
    let json = serde_json::json!({
        "model": "m",
        "input": "hi",
        "voice": "tara",
        "stream_format": "chunked",
    });
    assert!(serde_json::from_value::<SpeechRequest>(json).is_err());
}

#[test]
fn speech_request_defaults_response_format_to_mp3() {
    // The mp3 pin is structural: an omitted field resolves at
    // deserialization and always serializes back onto the wire.
    let json = serde_json::json!({
        "model": "m",
        "input": "hi",
        "voice": "tara",
    });
    let req: SpeechRequest = serde_json::from_value(json).expect("parse request");
    assert_eq!(req.response_format, SpeechResponseFormat::Mp3);
    assert_eq!(
        serde_json::to_value(&req)
            .expect("serialize")
            .get("response_format")
            .and_then(Value::as_str),
        Some("mp3")
    );
}

#[test]
fn speech_request_round_trips_both_voice_forms() {
    let string_form: SpeechRequest = serde_json::from_value(serde_json::json!({
        "model": "m",
        "input": "hi",
        "voice": "tara",
    }))
    .expect("parse request");
    assert_eq!(string_form.voice, SpeechVoice::Name("tara".to_owned()));
    let object_form: SpeechRequest = serde_json::from_value(serde_json::json!({
        "model": "m",
        "input": "hi",
        "voice": { "id": "tara" },
    }))
    .expect("parse request");
    assert_eq!(
        object_form.voice,
        SpeechVoice::Id {
            id: "tara".to_owned()
        }
    );
    for req in [string_form, object_form] {
        let reparsed: SpeechRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }
}

#[test]
fn speech_request_preserves_unnamed_fields_verbatim() {
    let json = serde_json::json!({
        "model": "m",
        "input": "hi",
        "voice": "tara",
        "response_format": "wav",
        "speed": 1.5,
        "instructions": "speak cheerfully",
        "stream_format": "sse",
        "sample_rate": 24000,
    });
    let req: SpeechRequest = serde_json::from_value(json).expect("parse request");
    assert_eq!(req.response_format, SpeechResponseFormat::Wav);
    assert_eq!(req.speed, Some(1.5));
    assert_eq!(req.instructions.as_deref(), Some("speak cheerfully"));
    assert_eq!(req.stream_format, Some(SpeechStreamFormat::Sse));
    // Unnamed fields land in `rest`, not on named fields.
    assert!(req.rest.contains_key("sample_rate"));
    for key in [
        "model",
        "input",
        "voice",
        "response_format",
        "speed",
        "instructions",
        "stream_format",
    ] {
        assert!(!req.rest.contains_key(key));
    }
    let reparsed: SpeechRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn speech_request_rejects_reserved_keys_in_rest() {
    let mut req = speech_request("m", "hi");
    req.rest.insert("input".to_owned(), serde_json::json!("y"));
    assert!(req.validate().is_err());
}
