//! Gateway-owned speech-to-text runtime and HTTP endpoints.
//!
//! [`SttRuntime`] provisions the selected profile's speech models through
//! [`ArtifactStore`](gateway_local::artifacts::ArtifactStore),
//! loads [`SttEngine`](gateway_transcribe::SttEngine), and unloads it on
//! profile switch. [`gateway_routes`] serves the gateway's streaming STT
//! surface, [`stt_routes`] remains the Workshop-listener attachment seam,
//! and [`transcribe`] implements OpenAI-compatible multipart transcription.

mod api;
mod runtime;
mod stt;

pub use api::{MAX_AUDIO_BYTES, TranscriptionError, transcribe};
pub use runtime::{SttRuntime, SttRuntimeError, SttState};
pub use stt::{gateway_routes, routes as stt_routes};
