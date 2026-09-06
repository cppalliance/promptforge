use serde_json::{Map, Value};

use super::shared::{
    AUDIO_RATE, AUDIO_TYPE, ClientError, ClientEvent, Correlation, HYPOTHESIS_INCLUDE, MODEL,
    SESSION_TYPE, SessionPatch,
};

pub(in crate::realtime) fn parse_client_event(text: &str) -> Result<ClientEvent, ClientError> {
    let value: Value = serde_json::from_str(text).map_err(|_| {
        ClientError::new(
            "invalid_json",
            "The client event is not valid JSON",
            None,
            Correlation::Omitted,
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        ClientError::new(
            "invalid_json",
            "The client event is not valid JSON",
            None,
            Correlation::Omitted,
        )
    })?;
    let correlation = correlation(object)?;
    let event_type = required_string(object, "type", "type", &correlation)?;
    match event_type {
        "session.update" => parse_update(object, &correlation),
        "input_audio_buffer.append" => parse_append(object, &correlation),
        "input_audio_buffer.commit" => parse_empty(object, &correlation, true),
        "input_audio_buffer.clear" => parse_empty(object, &correlation, false),
        unsupported => Err(ClientError::new(
            "unsupported_event_type",
            format!("Unsupported client event type {unsupported}"),
            Some("type"),
            correlation,
        )),
    }
}

fn correlation(object: &Map<String, Value>) -> Result<Correlation, ClientError> {
    match object.get("event_id") {
        None => Ok(Correlation::Omitted),
        Some(Value::String(id)) if !id.is_empty() => Ok(Correlation::Client(id.clone())),
        Some(_) => Err(ClientError::new(
            "invalid_event_id",
            "event_id must be a string",
            Some("event_id"),
            Correlation::Null,
        )),
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    path: &str,
    correlation: &Correlation,
) -> Result<&'a str, ClientError> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(ClientError::new(
            "invalid_field",
            format!("{path} must be a string"),
            Some(path),
            correlation.clone(),
        )),
        None => Err(ClientError::new(
            "missing_required_field",
            format!("Missing required field {path}"),
            Some(path),
            correlation.clone(),
        )),
    }
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    prefix: &str,
    correlation: &Correlation,
) -> Result<(), ClientError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        let path = format!("{prefix}{field}");
        return Err(ClientError::new(
            "unknown_field",
            format!("Unknown field {path}"),
            Some(&path),
            correlation.clone(),
        ));
    }
    Ok(())
}

fn object_at<'a>(
    value: &'a Value,
    path: &str,
    correlation: &Correlation,
) -> Result<&'a Map<String, Value>, ClientError> {
    value.as_object().ok_or_else(|| {
        ClientError::new(
            "invalid_field",
            format!("{path} must be an object"),
            Some(path),
            correlation.clone(),
        )
    })
}

fn parse_append(
    object: &Map<String, Value>,
    correlation: &Correlation,
) -> Result<ClientEvent, ClientError> {
    reject_unknown(object, &["type", "audio", "event_id"], "", correlation)?;
    let audio = required_string(object, "audio", "audio", correlation)?.to_owned();
    Ok(ClientEvent::Append {
        event_id: client_id(correlation),
        audio,
    })
}

fn parse_empty(
    object: &Map<String, Value>,
    correlation: &Correlation,
    commit: bool,
) -> Result<ClientEvent, ClientError> {
    reject_unknown(object, &["type", "event_id"], "", correlation)?;
    let event_id = client_id(correlation);
    Ok(if commit {
        ClientEvent::Commit { event_id }
    } else {
        ClientEvent::Clear { event_id }
    })
}

fn client_id(correlation: &Correlation) -> Option<String> {
    match correlation {
        Correlation::Client(id) => Some(id.clone()),
        Correlation::Omitted | Correlation::Null => None,
    }
}

fn parse_update(
    object: &Map<String, Value>,
    correlation: &Correlation,
) -> Result<ClientEvent, ClientError> {
    reject_unknown(object, &["type", "session", "event_id"], "", correlation)?;
    let session_value = object.get("session").ok_or_else(|| {
        ClientError::new(
            "missing_required_field",
            "Missing required field session",
            Some("session"),
            correlation.clone(),
        )
    })?;
    let session = object_at(session_value, "session", correlation)?;
    reject_unknown(
        session,
        &["type", "audio", "include"],
        "session.",
        correlation,
    )?;
    let session_type = required_string(session, "type", "session.type", correlation)?;
    if session_type != SESSION_TYPE {
        return Err(ClientError::new(
            "unsupported_session_type",
            "Only transcription sessions are supported",
            Some("session.type"),
            correlation.clone(),
        ));
    }
    let prompt = match session.get("audio") {
        Some(audio) => parse_audio(audio, correlation)?,
        None => None,
    };
    let include_hypothesis = match session.get("include") {
        Some(include) => Some(parse_include(include, correlation)?),
        None => None,
    };
    Ok(ClientEvent::SessionUpdate {
        event_id: client_id(correlation),
        patch: SessionPatch {
            prompt,
            include_hypothesis,
        },
    })
}

fn parse_audio(value: &Value, correlation: &Correlation) -> Result<Option<String>, ClientError> {
    let audio = object_at(value, "session.audio", correlation)?;
    reject_unknown(audio, &["input"], "session.audio.", correlation)?;
    let Some(input) = audio.get("input") else {
        return Ok(None);
    };
    let input = object_at(input, "session.audio.input", correlation)?;
    reject_unknown(
        input,
        &[
            "format",
            "noise_reduction",
            "transcription",
            "turn_detection",
        ],
        "session.audio.input.",
        correlation,
    )?;
    if input
        .get("noise_reduction")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ClientError::new(
            "unsupported_noise_reduction",
            "Only null noise reduction is supported",
            Some("session.audio.input.noise_reduction"),
            correlation.clone(),
        ));
    }
    if input
        .get("turn_detection")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ClientError::new(
            "unsupported_turn_detection",
            "Only null turn detection is supported",
            Some("session.audio.input.turn_detection"),
            correlation.clone(),
        ));
    }
    if let Some(format) = input.get("format") {
        parse_format(format, correlation)?;
    }
    input
        .get("transcription")
        .map(|transcription| parse_transcription(transcription, correlation))
        .transpose()
        .map(Option::flatten)
}

fn parse_format(value: &Value, correlation: &Correlation) -> Result<(), ClientError> {
    let format = object_at(value, "session.audio.input.format", correlation)?;
    reject_unknown(
        format,
        &["type", "rate"],
        "session.audio.input.format.",
        correlation,
    )?;
    if let Some(kind) = format.get("type")
        && kind.as_str() != Some(AUDIO_TYPE)
    {
        return Err(ClientError::new(
            "unsupported_audio_format",
            "Only audio/pcm is supported",
            Some("session.audio.input.format.type"),
            correlation.clone(),
        ));
    }
    if let Some(rate) = format.get("rate")
        && rate.as_u64() != Some(u64::from(AUDIO_RATE))
    {
        return Err(ClientError::new(
            "unsupported_audio_format",
            "Only 24 kHz PCM audio is supported",
            Some("session.audio.input.format.rate"),
            correlation.clone(),
        ));
    }
    Ok(())
}

fn parse_transcription(
    value: &Value,
    correlation: &Correlation,
) -> Result<Option<String>, ClientError> {
    let transcription = object_at(value, "session.audio.input.transcription", correlation)?;
    for (field, code, message) in [
        (
            "language",
            "unsupported_language",
            "A transcription language is not supported",
        ),
        (
            "logprobs",
            "unsupported_logprobs",
            "Transcription logprobs are not supported",
        ),
        (
            "keywords",
            "unsupported_keywords",
            "Transcription keywords are not supported",
        ),
        (
            "delay_ms",
            "unsupported_delay",
            "Transcription delay is not supported",
        ),
    ] {
        if transcription.contains_key(field) {
            let path = format!("session.audio.input.transcription.{field}");
            return Err(ClientError::new(
                code,
                message,
                Some(&path),
                correlation.clone(),
            ));
        }
    }
    reject_unknown(
        transcription,
        &["model", "prompt"],
        "session.audio.input.transcription.",
        correlation,
    )?;
    if let Some(model) = transcription.get("model")
        && model.as_str() != Some(MODEL)
    {
        return Err(ClientError::new(
            "unsupported_model",
            "Only realtime-transcribe is supported",
            Some("session.audio.input.transcription.model"),
            correlation.clone(),
        ));
    }
    match transcription.get("prompt") {
        None => Ok(None),
        Some(Value::String(prompt)) => Ok(Some(prompt.clone())),
        Some(_) => Err(ClientError::new(
            "invalid_prompt",
            "Transcription prompt must be a string",
            Some("session.audio.input.transcription.prompt"),
            correlation.clone(),
        )),
    }
}

fn parse_include(value: &Value, correlation: &Correlation) -> Result<bool, ClientError> {
    let values = value.as_array().ok_or_else(|| {
        ClientError::new(
            "invalid_include",
            "session.include must be an array",
            Some("session.include"),
            correlation.clone(),
        )
    })?;
    if values.is_empty() {
        return Ok(false);
    }
    if values.len() == 1 && values[0].as_str() == Some(HYPOTHESIS_INCLUDE) {
        return Ok(true);
    }
    let unsupported = values
        .iter()
        .find_map(Value::as_str)
        .unwrap_or("<non-string>");
    Err(ClientError::new(
        "unsupported_include",
        format!("Unsupported include value {unsupported}"),
        Some("session.include"),
        correlation.clone(),
    ))
}
