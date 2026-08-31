//! OpenAI-compatible multipart transcription handling.

use std::io::Cursor;

use axum::extract::Multipart;
use axum::response::{IntoResponse, Response};
use promptforge_transcribe::SAMPLE_RATE;
use serde::Serialize;

use crate::runtime::{LoadedModelRole, SttState};

/// Maximum accepted audio file size: 25 MiB.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

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

/// Parses and executes one OpenAI-compatible multipart transcription.
///
/// The multipart dialect accepts `file`, `model`, `language`, `prompt`,
/// `temperature`, `response_format`, and the literal repeated field name
/// `timestamp_granularities[]`.
///
/// # Errors
/// Returns [`TranscriptionError::FileTooLarge`] above 25 MiB,
/// [`TranscriptionError::ModelNotFound`] when `model` is not active,
/// [`TranscriptionError::InvalidAudio`] for audio other than 16 kHz mono
/// WAV, and the other variants for malformed multipart fields or inference
/// failure.
pub async fn transcribe(
    state: &SttState,
    multipart: Multipart,
) -> Result<Response, TranscriptionError> {
    let form = parse_form(multipart).await?;
    let Some((engine, role)) = state.select(&form.model) else {
        return Err(TranscriptionError::ModelNotFound(form.model));
    };
    let (samples, duration) = decode_wav(&form.file)?;
    let text = match role {
        LoadedModelRole::Interim => engine.transcribe(samples).await,
        LoadedModelRole::Final => engine
            .transcribe_final(samples)
            .await
            .ok_or_else(|| TranscriptionError::ModelNotFound(form.model.clone()))?,
    }
    .map_err(TranscriptionError::Inference)?;
    Ok(axum::Json(response(form, text, duration)).into_response())
}

async fn parse_form(mut multipart: Multipart) -> Result<TranscriptionForm, TranscriptionError> {
    let mut file = None;
    let mut model = None;
    let mut language = None;
    let mut format = ResponseFormat::Json;
    let mut granularities = default_granularities();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(TranscriptionError::Multipart)?
    {
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };
        match name.as_str() {
            "file" => {
                let mut bytes = Vec::new();
                while let Some(chunk) =
                    field.chunk().await.map_err(TranscriptionError::Multipart)?
                {
                    if bytes.len().saturating_add(chunk.len()) > MAX_AUDIO_BYTES {
                        return Err(TranscriptionError::FileTooLarge);
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
                        return Err(TranscriptionError::UnsupportedResponseFormat(
                            value.to_owned(),
                        ));
                    }
                };
            }
            "timestamp_granularities[]" => {
                granularities.push(match field_text(field).await?.as_str() {
                    "word" => TimestampGranularity::Word,
                    "segment" => TimestampGranularity::Segment,
                    value => {
                        return Err(TranscriptionError::InvalidField {
                            field: "timestamp_granularities[]",
                            value: value.to_owned(),
                        });
                    }
                });
            }
            "temperature" => {
                let value = field_text(field).await?;
                let parsed =
                    value
                        .parse::<f32>()
                        .map_err(|_| TranscriptionError::InvalidField {
                            field: "temperature",
                            value: value.clone(),
                        })?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(TranscriptionError::InvalidField {
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
        file: file.ok_or(TranscriptionError::MissingField("file"))?,
        model: model.ok_or(TranscriptionError::MissingField("model"))?,
        language,
        format,
        granularities,
    })
}

async fn field_text(
    field: axum::extract::multipart::Field<'_>,
) -> Result<String, TranscriptionError> {
    field.text().await.map_err(TranscriptionError::Multipart)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "PCM normalization and clip duration intentionally convert bounded audio counts to floating point"
)]
fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, f64), TranscriptionError> {
    const SAMPLE_RATE_U32: u32 = 16_000;
    let mut reader =
        hound::WavReader::new(Cursor::new(bytes)).map_err(TranscriptionError::InvalidAudio)?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != SAMPLE_RATE_U32 {
        return Err(TranscriptionError::UnsupportedAudio {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        });
    }
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(TranscriptionError::InvalidAudio)?,
        hound::SampleFormat::Int => {
            let denominator = 2_f32.powi(i32::from(spec.bits_per_sample.saturating_sub(1)));
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 / denominator)
                        .map_err(TranscriptionError::InvalidAudio)
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let duration = samples.len() as f64 / SAMPLE_RATE as f64;
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

/// A multipart transcription request failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscriptionError {
    /// Multipart framing could not be decoded.
    #[non_exhaustive]
    #[error("invalid multipart transcription request")]
    Multipart(#[source] axum::extract::multipart::MultipartError),

    /// A required form field was absent.
    #[non_exhaustive]
    #[error("missing multipart field {0}")]
    MissingField(&'static str),

    /// One form field carried an unsupported value.
    #[non_exhaustive]
    #[error("invalid multipart field {field}: {value}")]
    InvalidField {
        /// Literal field name.
        field: &'static str,
        /// Refused field value.
        value: String,
    },

    /// The requested response format is not implemented.
    #[non_exhaustive]
    #[error("unsupported transcription response format {0}")]
    UnsupportedResponseFormat(String),

    /// The audio file exceeded 25 MiB.
    #[error("audio file exceeds the 25 MiB limit")]
    FileTooLarge,

    /// The requested model is not loaded in the active profile.
    #[non_exhaustive]
    #[error("unknown model {0}")]
    ModelNotFound(String),

    /// WAV parsing failed.
    #[non_exhaustive]
    #[error("invalid WAV audio")]
    InvalidAudio(#[source] hound::Error),

    /// The WAV sample rate or channel count is unsupported.
    #[non_exhaustive]
    #[error("audio must be 16 kHz mono, got {sample_rate} Hz and {channels} channels")]
    UnsupportedAudio {
        /// Input sample rate.
        sample_rate: u32,
        /// Input channel count.
        channels: u16,
    },

    /// Whisper rejected the audio.
    #[non_exhaustive]
    #[error("transcribe audio")]
    Inference(#[source] promptforge_transcribe::TranscribeError),
}

impl TranscriptionError {
    /// Builds a loaded-model selection failure.
    #[must_use]
    pub fn model_not_found_error(model: impl Into<String>) -> Self {
        Self::ModelNotFound(model.into())
    }

    /// Returns the unknown model name for a model-selection failure.
    #[must_use]
    pub fn model_not_found(&self) -> Option<&str> {
        match self {
            Self::ModelNotFound(model) => Some(model),
            _ => None,
        }
    }

    /// Returns whether the caller exceeded the upload cap.
    #[must_use]
    pub fn is_file_too_large(&self) -> bool {
        matches!(self, Self::FileTooLarge)
    }

    /// Returns whether whisper inference failed after request validation.
    #[must_use]
    pub fn is_inference(&self) -> bool {
        matches!(self, Self::Inference(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use tower::ServiceExt;

    fn wav(samples: &[i16]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut bytes,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .expect("writer builds");
            for sample in samples {
                writer.write_sample(*sample).expect("sample writes");
            }
            writer.finalize().expect("WAV finalizes");
        }
        bytes.into_inner()
    }

    fn wav_f32(samples: &[f32]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut bytes,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16_000,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .expect("writer builds");
            for sample in samples {
                writer.write_sample(*sample).expect("sample writes");
            }
            writer.finalize().expect("WAV finalizes");
        }
        bytes.into_inner()
    }

    #[test]
    fn wav_decode_accepts_the_voice_wire_sample_rate() {
        let (samples, duration) = decode_wav(&wav(&[0, i16::MAX])).expect("WAV decodes");
        assert_eq!(samples.len(), 2);
        assert!(samples[1] > 0.99);
        assert!((duration - 2.0 / 16_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn verbose_json_honors_segment_granularity() {
        let response = response(
            TranscriptionForm {
                file: Vec::new(),
                model: "speech".to_owned(),
                language: Some("en".to_owned()),
                format: ResponseFormat::VerboseJson,
                granularities: vec![TimestampGranularity::Segment],
            },
            "hello".to_owned(),
            1.25,
        );
        let json = serde_json::to_value(response).expect("response serializes");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["duration"], 1.25);
        assert_eq!(json["segments"][0]["end"], 1.25);
    }

    #[test]
    fn verbose_json_defaults_to_segment_timestamps() {
        let granularities = default_granularities();
        let response = response(
            TranscriptionForm {
                file: Vec::new(),
                model: "speech".to_owned(),
                language: None,
                format: ResponseFormat::VerboseJson,
                granularities,
            },
            "hello".to_owned(),
            1.25,
        );
        let json = serde_json::to_value(response).expect("response serializes");
        assert_eq!(json["segments"][0]["text"], "hello");
    }

    #[test]
    fn compact_json_contains_only_text() {
        let response = response(
            TranscriptionForm {
                file: Vec::new(),
                model: "speech".to_owned(),
                language: None,
                format: ResponseFormat::Json,
                granularities: Vec::new(),
            },
            "hello".to_owned(),
            1.0,
        );
        assert_eq!(
            serde_json::to_value(response).expect("response serializes"),
            serde_json::json!({"text": "hello"})
        );
    }

    fn multipart_body(file: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
        const BOUNDARY: &str = "promptforge-stt-boundary";
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
              Content-Type: audio/wav\r\n\r\n",
        );
        body.extend_from_slice(file);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        (BOUNDARY.to_owned(), body)
    }

    async fn test_endpoint(State(state): State<SttState>, multipart: Multipart) -> Response {
        match transcribe(&state, multipart).await {
            Ok(response) => response.into_response(),
            Err(error) if error.model_not_found().is_some() => {
                (StatusCode::NOT_FOUND, error.to_string()).into_response()
            }
            Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
        }
    }

    #[tokio::test]
    async fn an_unloaded_model_is_not_found() {
        let (boundary, body) = multipart_body(
            &wav(&vec![0; 16_000]),
            &[("model", "not-loaded"), ("response_format", "json")],
        );
        let response = axum::Router::new()
            .route("/v1/audio/transcriptions", post(test_endpoint))
            .layer(axum::extract::DefaultBodyLimit::max(
                MAX_AUDIO_BYTES + 1024 * 1024,
            ))
            .with_state(SttState::default())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("route answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_audio_file_over_25_mib_is_rejected_before_decode() {
        let oversized = vec![0_u8; MAX_AUDIO_BYTES + 1];
        let (boundary, body) = multipart_body(&oversized, &[("model", "speech")]);
        let response = axum::Router::new()
            .route("/v1/audio/transcriptions", post(test_endpoint))
            .layer(axum::extract::DefaultBodyLimit::max(
                MAX_AUDIO_BYTES + 1024 * 1024,
            ))
            .with_state(SttState::default())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("route answers");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        assert_eq!(&body[..], b"audio file exceeds the 25 MiB limit");
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn verbose_round_trip_accepts_literal_timestamp_granularities_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = promptforge_transcribe::fixtures::require_model()
            .display()
            .to_string()
            .replace('\\', "/");
        let cache = dir.path().display().to_string().replace('\\', "/");
        let catalog = promptforge_gateway_config::Config::from_toml_str(&format!(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
             [local]\ncache_dir = {cache:?}\n\
             [workshop]\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
             vram_gb = 1.0\n\
             [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
        ))
        .expect("catalog parses");
        let config = catalog
            .select_profile(&promptforge_gateway_config::ProfileName::parse("work").expect("name"))
            .expect("profile selects");
        let state = SttState::default();
        let runtime = crate::SttRuntime::start(&config, state.clone(), None).expect("engine loads");
        let samples = promptforge_transcribe::fixtures::jfk_samples();
        let (boundary, body) = multipart_body(
            &wav_f32(&samples),
            &[
                ("model", "speech"),
                ("response_format", "verbose_json"),
                ("timestamp_granularities[]", "segment"),
            ],
        );
        let response = axum::Router::new()
            .route("/v1/audio/transcriptions", post(test_endpoint))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("route answers");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert!(
            json["text"]
                .as_str()
                .is_some_and(|text| text.to_lowercase().contains("country"))
        );
        assert_eq!(json["segments"][0]["start"], 0.0);
        runtime.shutdown();
    }
}
