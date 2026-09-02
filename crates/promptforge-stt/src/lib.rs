//! Gateway-owned speech-to-text runtime and HTTP endpoints.
//!
//! [`SttRuntime`] provisions the selected profile's speech models through
//! [`ArtifactStore`](promptforge_gateway_local::artifacts::ArtifactStore),
//! loads [`SttEngine`](promptforge_transcribe::SttEngine), and unloads it on
//! profile switch. [`stt_routes`] serves the Workshop `/stt` socket,
//! while [`transcribe`] implements OpenAI-compatible multipart transcription
//! for the gateway listener.

mod api;
mod runtime;
mod stt;

pub use api::{MAX_AUDIO_BYTES, TranscriptionError, transcribe};
pub use runtime::{SttRuntime, SttRuntimeError, SttState};
pub use stt::routes as stt_routes;
