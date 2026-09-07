//! Gateway-owned speech facade and HTTP endpoints.
//!
//! [`SpeechService`] owns artifact preparation, complete generation
//! publication, batch transcription, and Realtime transcription.

mod artifacts;
#[allow(dead_code)]
mod audio;
mod batch;
mod generation;
mod model;
#[allow(dead_code)]
mod realtime;
mod replacement;
mod segment;
mod service;
mod status;
mod take;
#[cfg(all(test, not(feature = "test-fixtures")))]
mod test_fixtures;
#[cfg(feature = "test-fixtures")]
pub mod test_fixtures;

pub use artifacts::{PreparedSpeech, SpeechError};
pub use generation::SpeechReplacement;
pub use model::SpeechModelInfo;
pub use service::SpeechService;
pub use status::SpeechStatus;

#[cfg(all(test, miri))]
mod miri_tests {
    use super::SpeechService;

    #[test]
    fn miri_facade_target_executes_without_native_route_fixtures() {
        let service = SpeechService::new();
        assert!(!service.status().ready());
    }
}
