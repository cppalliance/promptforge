//! OpenAI-compatible multipart transcription handling.

use std::io::Cursor;

use axum::extract::multipart::MultipartRejection;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_stt_engine::{DecodeRequest, EnginePolicy};
use serde::Serialize;

use crate::artifacts::SpeechError;
use crate::generation::GenerationState;

const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;
const BODY_LIMIT: usize = MAX_AUDIO_BYTES + 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseFormat {
    Json,
    VerboseJson,
}

#[derive(Debug)]
struct TranscriptionForm {
    file: Vec<u8>,
    model: String,
    language: Option<String>,
    format: ResponseFormat,
    granularities: Vec<TimestampGranularity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimestampGranularity {
    Word,
    Segment,
}

fn default_granularities() -> Vec<TimestampGranularity> {
    vec![TimestampGranularity::Segment]
}

/// A basic OpenAI transcription response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct JsonTranscription {
    /// The decoded transcript.
    pub text: String,
}

/// One clip-level segment in a verbose transcription response.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct TranscriptionSegment {
    /// Zero-based segment identifier.
    pub id: u32,
    /// Segment start in seconds.
    pub start: f64,
    /// Segment end in seconds.
    pub end: f64,
    /// Text decoded for the segment.
    pub text: String,
}

/// An OpenAI verbose transcription response.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct VerboseJsonTranscription {
    /// Requested task name.
    pub task: &'static str,
    /// Detected or caller-supplied language.
    pub language: String,
    /// Audio duration in seconds.
    pub duration: f64,
    /// The decoded transcript.
    pub text: String,
    /// Clip-level segments when segment granularity was requested.
    pub segments: Vec<TranscriptionSegment>,
    /// Word timestamps. The current engine exposes no word alignment, so this
    /// array stays empty when word granularity is requested.
    pub words: Vec<serde_json::Value>,
}

/// A successful transcription in the requested OpenAI JSON dialect.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
enum TranscriptionResponse {
    /// The compact `json` response.
    Json(JsonTranscription),
    /// The `verbose_json` response.
    VerboseJson(VerboseJsonTranscription),
}

pub(crate) fn routes(state: GenerationState) -> Router {
    Router::new()
        .route("/v1/audio/transcriptions", post(handler))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

async fn handler(
    State(state): State<GenerationState>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Response {
    let multipart = match multipart {
        Ok(multipart) => multipart,
        Err(error) => {
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "malformed_request",
                &format!("malformed request: {error}"),
            );
        }
    };
    match transcribe(&state, multipart).await {
        Ok(response) => response,
        Err(error) => error_response(&error),
    }
}

async fn transcribe(
    state: &GenerationState,
    multipart: Multipart,
) -> Result<Response, SpeechError> {
    let form = parse_form(multipart).await?;
    let Some((generation, mode)) = state.select(&form.model) else {
        return Err(SpeechError::ModelNotFound(form.model));
    };
    let (samples, duration) = decode_wav(&form.file)?;
    let text = generation
        .engine()
        .decode(DecodeRequest::new(
            mode,
            samples,
            generation.guidance().to_vec(),
            String::new(),
        ))
        .await
        .map_err(SpeechError::Inference)?;
    Ok(axum::Json(response(form, text, duration)).into_response())
}

async fn parse_form(mut multipart: Multipart) -> Result<TranscriptionForm, SpeechError> {
    let mut file = None;
    let mut model = None;
    let mut language = None;
    let mut format = ResponseFormat::Json;
    let mut granularities = default_granularities();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(SpeechError::Multipart)?
    {
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };
        match name.as_str() {
            "file" => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(SpeechError::Multipart)? {
                    if bytes.len().saturating_add(chunk.len()) > MAX_AUDIO_BYTES {
                        return Err(SpeechError::FileTooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                file = Some(bytes);
            }
            "model" => model = Some(field_text(field).await?),
            "language" => language = Some(field_text(field).await?),
            "response_format" => {
                format = match field_text(field).await?.as_str() {
                    "json" => ResponseFormat::Json,
                    "verbose_json" => ResponseFormat::VerboseJson,
                    value => {
                        return Err(SpeechError::UnsupportedResponseFormat(value.to_owned()));
                    }
                };
            }
            "timestamp_granularities[]" => {
                granularities.push(match field_text(field).await?.as_str() {
                    "word" => TimestampGranularity::Word,
                    "segment" => TimestampGranularity::Segment,
                    value => {
                        return Err(SpeechError::InvalidField {
                            field: "timestamp_granularities[]",
                            value: value.to_owned(),
                        });
                    }
                });
            }
            "temperature" => {
                let value = field_text(field).await?;
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| SpeechError::InvalidField {
                        field: "temperature",
                        value: value.clone(),
                    })?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(SpeechError::InvalidField {
                        field: "temperature",
                        value,
                    });
                }
            }
            // OpenAI-compatible hints accepted by the dialect. The current
            // English whisper workers already own their prompt policy.
            "prompt" => {
                let _ignored = field_text(field).await?;
            }
            _ => {}
        }
    }
    Ok(TranscriptionForm {
        file: file.ok_or(SpeechError::MissingField("file"))?,
        model: model.ok_or(SpeechError::MissingField("model"))?,
        language,
        format,
        granularities,
    })
}

async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, SpeechError> {
    field.text().await.map_err(SpeechError::Multipart)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "PCM normalization and clip duration intentionally convert bounded audio counts to floating point"
)]
fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, f64), SpeechError> {
    const SAMPLE_RATE_U32: u32 = 16_000;
    let mut reader =
        hound::WavReader::new(Cursor::new(bytes)).map_err(SpeechError::InvalidAudio)?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != SAMPLE_RATE_U32 {
        return Err(SpeechError::UnsupportedAudio {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        });
    }
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(SpeechError::InvalidAudio)?,
        hound::SampleFormat::Int => {
            let denominator = 2_f32.powi(i32::from(spec.bits_per_sample.saturating_sub(1)));
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 / denominator)
                        .map_err(SpeechError::InvalidAudio)
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let duration = samples.len() as f64 / EnginePolicy::SAMPLE_RATE as f64;
    Ok((samples, duration))
}

fn response(form: TranscriptionForm, text: String, duration: f64) -> TranscriptionResponse {
    match form.format {
        ResponseFormat::Json => TranscriptionResponse::Json(JsonTranscription { text }),
        ResponseFormat::VerboseJson => {
            let segments = if form.granularities.contains(&TimestampGranularity::Segment) {
                vec![TranscriptionSegment {
                    id: 0,
                    start: 0.0,
                    end: duration,
                    text: text.clone(),
                }]
            } else {
                Vec::new()
            };
            TranscriptionResponse::VerboseJson(VerboseJsonTranscription {
                task: "transcribe",
                language: form.language.unwrap_or_else(|| "en".to_owned()),
                duration,
                text,
                segments,
                words: Vec::new(),
            })
        }
    }
}

fn error_response(error: &SpeechError) -> Response {
    let (status, kind, code) = if error.model_not_found().is_some() {
        (
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            "model_not_found",
        )
    } else if error.is_file_too_large() {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_request_error",
            "file_too_large",
        )
    } else if error.is_inference() {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "transcription_error",
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "malformed_request",
        )
    };
    let message = if error.is_inference() {
        "transcription failed".to_owned()
    } else if error.model_not_found().is_some() || error.is_file_too_large() {
        error.to_string()
    } else {
        format!("malformed request: {error}")
    };
    openai_error_response(status, kind, code, &message)
}

fn openai_error_response(
    status: StatusCode,
    kind: &'static str,
    code: &'static str,
    message: &str,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": kind,
                "code": code,
            }
        })),
    )
        .into_response()
}

#[cfg(all(test, not(miri)))]
mod native_tests;

#[cfg(test)]
mod tests;
