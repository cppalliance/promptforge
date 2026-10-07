//! Runtime-loaded safe bindings for PromptForge's pinned whisper.cpp API.
//!
//! [`WhisperLibrary`] opens the packaged shared library and resolves the C
//! symbols. Contexts and states keep that library loaded through reference
//! counting, and all raw pointers stay behind the safe wrapper.

#![expect(
    unsafe_code,
    reason = "loading and calling the whisper.cpp C ABI requires unsafe operations"
)]

mod context;
mod error;
mod library;
mod log;
mod params;
mod raw;

pub use context::{ContextParams, TokenSpan, WhisperContext, WhisperState};
pub use error::WhisperError;
pub use library::WhisperLibrary;
pub use params::{FullParams, SamplingStrategy};

// Miri excludes dynamic library loading and native log callback tests; native CI owns them.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::library::system_info_has_gpu;

    const SAMPLE_RATE: u64 = 16_000;

    /// Records every `tracing` event message on the threads it is the default
    /// subscriber for.
    #[derive(Clone, Default)]
    struct CapturedLog(Arc<Mutex<Vec<String>>>);

    impl CapturedLog {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().expect("log capture lock"))
        }
    }

    struct Message(String);

    impl tracing::field::Visit for Message {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl tracing::Subscriber for CapturedLog {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut message = Message(String::new());
            event.record(&mut message);
            self.0.lock().expect("log capture lock").push(message.0);
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    fn native_fixture(variable: &str) -> PathBuf {
        let Some(path) = std::env::var_os(variable) else {
            panic!("{variable} is set");
        };
        PathBuf::from(path)
    }

    fn jfk_samples() -> Vec<f32> {
        let mut reader = hound::WavReader::open(native_fixture("PROMPTFORGE_WHISPER_AUDIO"))
            .expect("JFK fixture opens");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
        assert_eq!(spec.channels, 1, "fixture must be mono");
        assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
        reader
            .samples::<i16>()
            .map(|sample| f32::from(sample.expect("fixture sample decodes")) / 32_768.0)
            .collect()
    }

    #[test]
    fn flash_attention_lands_in_context_params_only_when_set() {
        // SAFETY: every raw::ContextParams member is an integer, bool, raw
        // pointer, or a struct of those, and all-zero bytes are valid for each.
        let mut native: raw::ContextParams = unsafe { std::mem::zeroed() };
        ContextParams::default().apply(&mut native);
        assert!(!native.flash_attn, "unset keeps whisper's default");

        let mut params = ContextParams::default();
        params.set_flash_attn(true);
        params.apply(&mut native);
        assert!(native.flash_attn, "the setter lands in the native member");
    }

    #[test]
    fn gpu_probe_recognizes_cuda_and_metal_markers() {
        assert!(system_info_has_gpu("CUDA = 1 | CPU = 1"));
        assert!(system_info_has_gpu("METAL : EMBED_LIBRARY = 1"));
        assert!(!system_info_has_gpu("CUDA = 0 | METAL = 0 | CPU = 1"));
    }

    #[test]
    fn language_and_prompt_reject_interior_nulls() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        assert!(params.set_language(Some("e\0n")).is_err());
        assert!(params.set_initial_prompt("hello\0world").is_err());
    }

    #[test]
    fn wrapper_types_keep_native_ownership_private() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WhisperLibrary>();
    }

    #[test]
    fn pinned_b4938_parameter_layout_matches_the_64_bit_c_abi() {
        assert_eq!(usize::BITS, 64, "PromptForge ships only 64-bit targets");
        assert_eq!(std::mem::size_of::<raw::ContextParams>(), 48);
        assert_eq!(std::mem::size_of::<raw::FullParams>(), 304);
        assert_eq!(std::mem::align_of::<raw::FullParams>(), 8);
    }

    // Offsets of every member PromptForge writes, from `struct
    // whisper_full_params` in the b4938 whisper.h under the 64-bit C ABI.
    const _: () = {
        use std::mem::offset_of;

        use raw::FullParams as P;

        assert!(usize::BITS == 64, "PromptForge ships only 64-bit targets");
        assert!(offset_of!(P, n_threads) == 4);
        assert!(offset_of!(P, translate) == 20);
        assert!(offset_of!(P, no_context) == 21);
        assert!(offset_of!(P, no_timestamps) == 22);
        assert!(offset_of!(P, single_segment) == 23);
        assert!(offset_of!(P, print_special) == 24);
        assert!(offset_of!(P, print_progress) == 25);
        assert!(offset_of!(P, print_realtime) == 26);
        assert!(offset_of!(P, print_timestamps) == 27);
        assert!(offset_of!(P, token_timestamps) == 28);
        assert!(offset_of!(P, max_tokens) == 48);
        assert!(offset_of!(P, audio_ctx) == 56);
        assert!(offset_of!(P, initial_prompt) == 72);
        assert!(offset_of!(P, language) == 104);
        assert!(offset_of!(P, detect_language) == 112);
        assert!(offset_of!(P, suppress_blank) == 113);
        assert!(offset_of!(P, suppress_nst) == 114);
        assert!(offset_of!(P, temperature_inc) == 128);
        assert!(offset_of!(P, entropy_thold) == 132);
        assert!(offset_of!(P, logprob_thold) == 136);
        assert!(offset_of!(P, no_speech_thold) == 140);
        assert!(offset_of!(P, greedy.best_of) == 144);
        assert!(offset_of!(P, abort_callback) == 208);
        assert!(offset_of!(P, abort_callback_user_data) == 216);

        assert!(offset_of!(raw::ContextParams, flash_attn) == 1);

        // `whisper_token_data`, returned by value from the token-data getter.
        assert!(size_of::<raw::TokenData>() == 56);
        assert!(align_of::<raw::TokenData>() == 8);
        assert!(offset_of!(raw::TokenData, p) == 8);
        assert!(offset_of!(raw::TokenData, t0) == 24);
        assert!(offset_of!(raw::TokenData, t1) == 32);
    };

    #[test]
    #[ignore = "requires PROMPTFORGE_WHISPER_LIBRARY to name a packaged runtime"]
    fn packaged_runtime_loads_and_reports_its_backend() {
        let path = std::env::var_os("PROMPTFORGE_WHISPER_LIBRARY")
            .map(PathBuf::from)
            .expect("PROMPTFORGE_WHISPER_LIBRARY is set");
        let library = WhisperLibrary::load(&path).expect("packaged whisper runtime loads");
        // `WhisperLibrary::load` resolves `whisper_log_set` eagerly, so the
        // load above proves the pinned b4938 library exports it; installing
        // the bridge proves the resolved symbol is callable.
        library.set_log_callback();
        let info = library
            .system_info()
            .expect("system information is exported");
        assert!(!info.is_empty());
        assert_eq!(
            library.gpu_available().expect("GPU probe succeeds"),
            system_info_has_gpu(&info)
        );
    }

    #[test]
    #[ignore = "requires packaged whisper and model fixtures"]
    fn packaged_runtime_receives_the_flash_attention_setting_at_context_init() {
        let library = WhisperLibrary::load(&native_fixture("PROMPTFORGE_WHISPER_LIBRARY"))
            .expect("packaged whisper runtime loads");
        library.set_log_callback();
        let model = native_fixture("PROMPTFORGE_WHISPER_MODEL");
        let log = CapturedLog::default();
        // whisper.cpp exposes no context-parameter getter, so its init log is
        // the only place the value it received is observable.
        tracing::subscriber::with_default(log.clone(), || {
            for (enabled, reported) in [(false, "flash attn = 0"), (true, "flash attn = 1")] {
                let mut params = ContextParams::default();
                params.set_flash_attn(enabled);
                let _context = WhisperContext::with_params(&library, &model, &params)
                    .expect("tiny model loads");
                let lines = log.take();
                assert!(
                    lines.iter().any(|line| line.contains(reported)),
                    "whisper reports `{reported}` at context init: {lines:?}"
                );
            }
        });
    }

    #[test]
    #[ignore = "requires packaged whisper, model, and audio fixtures"]
    fn packaged_runtime_reports_token_probabilities_and_spans() {
        let library = WhisperLibrary::load(&native_fixture("PROMPTFORGE_WHISPER_LIBRARY"))
            .expect("packaged whisper runtime loads");
        let context = WhisperContext::new(&library, &native_fixture("PROMPTFORGE_WHISPER_MODEL"))
            .expect("tiny model loads");
        let mut state = context.create_state().expect("decoding state allocates");
        let samples = jfk_samples();
        let clip = Duration::from_millis(
            u64::try_from(samples.len()).expect("sample count fits u64") * 1_000 / SAMPLE_RATE,
        );

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params
            .set_language(Some("en"))
            .expect("language has no null");
        params.set_token_timestamps(true);
        params.set_temperature_inc(0.0);
        params.set_n_threads(2);
        state.full(&params, &samples).expect("JFK decodes");

        let segments = state.segment_count();
        assert!(segments > 0, "JFK yields at least one segment");
        let mut tokens = 0;
        let mut most_probable = 0.0_f32;
        let mut latest_end = Duration::ZERO;
        for segment in 0..segments {
            let no_speech = state
                .segment_no_speech_probability(segment)
                .expect("segment is in range");
            assert!(
                (0.0..=1.0).contains(&no_speech),
                "no-speech probability {no_speech} is a probability"
            );
            let count = state.token_count(segment).expect("segment is in range");
            for token in 0..count {
                let probability = state
                    .token_probability(segment, token)
                    .expect("token is in range");
                assert!(
                    (0.0..=1.0).contains(&probability),
                    "token probability {probability} is a probability"
                );
                most_probable = most_probable.max(probability);
                let span = state
                    .token_span(segment, token)
                    .expect("token is in range")
                    .expect("token timestamps were requested");
                assert!(span.start <= span.end, "{span:?} is ordered");
                assert!(
                    span.end <= clip + Duration::from_secs(1),
                    "{span:?} ends within the {clip:?} clip"
                );
                latest_end = latest_end.max(span.end);
                tokens += 1;
            }
            assert!(
                matches!(
                    state.token_probability(segment, count),
                    Err(WhisperError::InvalidToken { .. })
                ),
                "a token past the segment is rejected"
            );
        }
        assert!(tokens > 20, "JFK decodes to {tokens} tokens");
        assert!(
            most_probable > 0.5,
            "a confident token is read, not zeroed: {most_probable}"
        );
        assert!(
            latest_end >= clip / 2,
            "token spans reach into the clip: {latest_end:?} of {clip:?}"
        );
        assert!(
            matches!(
                state.token_count(segments),
                Err(WhisperError::InvalidSegment { .. })
            ),
            "a segment past the result is rejected"
        );

        let mut untimed = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        untimed
            .set_language(Some("en"))
            .expect("language has no null");
        state.full(&untimed, &samples).expect("JFK decodes again");
        assert_eq!(
            state.token_span(0, 0).expect("first token is in range"),
            None,
            "a pass without token timestamps reports no spans"
        );
    }
}
