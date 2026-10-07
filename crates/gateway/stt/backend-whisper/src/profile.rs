//! Per-role whisper decode settings.

use std::ffi::c_int;
use std::num::NonZeroUsize;

use gateway_stt_engine::EnginePolicy;
use gateway_whisper_ffi::FullParams;

/// b4938 builds its fallback temperature ladder only for a step above zero.
const NO_TEMPERATURE_FALLBACK: f32 = 0.0;
const ENCODER_FRAMES_PER_SECOND: u64 = 50;
const SAMPLES_PER_ENCODER_FRAME: u64 = EnginePolicy::SAMPLE_RATE as u64 / ENCODER_FRAMES_PER_SECOND;
const AUDIO_CTX_MARGIN_FRAMES: u64 = 128;
const AUDIO_CTX_BLOCK_FRAMES: u64 = 64;
const MIN_AUDIO_CTX_FRAMES: u64 = 512;
/// Every whisper model's encoder context; a larger `audio_ctx` fails the pass.
const FULL_AUDIO_CTX_FRAMES: u64 = 1500;
const INTERIM_TOKENS_PER_SECOND: u64 = 4;
const MAX_DECODE_THREADS: usize = 4;

/// Decode settings one decoder role applies to every pass it runs.
///
/// A `None` member leaves whisper's default in place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoleProfile {
    pub(crate) single_segment: bool,
    pub(crate) temperature_inc: Option<f32>,
    /// Whether each pass sizes whisper's encoder context to its own audio.
    pub(crate) fit_audio_ctx: bool,
    pub(crate) max_tokens: Option<c_int>,
    pub(crate) n_threads: c_int,
    /// Whether each pass times its tokens, which word end times need.
    ///
    /// b4938 times no tokens while `no_timestamps` is set, so a timed pass
    /// also clears it.
    pub(crate) token_timestamps: bool,
}

impl RoleProfile {
    /// The interim role for a `window_seconds` window on `cores` cores.
    pub(crate) fn interim(window_seconds: u64, cores: usize) -> Self {
        let max_tokens = window_seconds.saturating_mul(INTERIM_TOKENS_PER_SECOND);
        Self {
            single_segment: true,
            temperature_inc: Some(NO_TEMPERATURE_FALLBACK),
            fit_audio_ctx: true,
            max_tokens: Some(c_int::try_from(max_tokens).unwrap_or(c_int::MAX)),
            n_threads: decode_threads(cores),
            token_timestamps: true,
        }
    }

    /// The final role on `cores` cores.
    pub(crate) fn final_pass(cores: usize) -> Self {
        Self {
            single_segment: false,
            temperature_inc: None,
            fit_audio_ctx: false,
            max_tokens: None,
            n_threads: decode_threads(cores),
            token_timestamps: false,
        }
    }

    /// Sets the profile on `params` for a pass over `samples` samples.
    pub(crate) fn apply(&self, params: &mut FullParams, samples: usize) {
        params.set_single_segment(self.single_segment);
        params.set_n_threads(self.n_threads);
        if let Some(value) = self.temperature_inc {
            params.set_temperature_inc(value);
        }
        if self.fit_audio_ctx
            && let Some(value) = interim_audio_ctx(samples)
        {
            params.set_audio_ctx(value);
        }
        if let Some(value) = self.max_tokens {
            params.set_max_tokens(value);
        }
        if self.token_timestamps {
            params.set_token_timestamps(true);
            params.set_no_timestamps(false);
        }
    }
}

/// Encoder frames covering `samples` samples plus margin, or `None` when that
/// reaches past the model's full context.
pub(crate) fn interim_audio_ctx(samples: usize) -> Option<c_int> {
    let frames = u64::try_from(samples)
        .unwrap_or(u64::MAX)
        .div_ceil(SAMPLES_PER_ENCODER_FRAME)
        .saturating_add(AUDIO_CTX_MARGIN_FRAMES)
        .div_ceil(AUDIO_CTX_BLOCK_FRAMES)
        .saturating_mul(AUDIO_CTX_BLOCK_FRAMES)
        .max(MIN_AUDIO_CTX_FRAMES);
    if frames > FULL_AUDIO_CTX_FRAMES {
        return None;
    }
    c_int::try_from(frames).ok()
}

/// CPU threads for one decoder, leaving the other half of `cores` to its
/// peer role.
pub(crate) fn decode_threads(cores: usize) -> c_int {
    let threads = (cores / 2).clamp(1, MAX_DECODE_THREADS);
    c_int::try_from(threads).unwrap_or(1)
}

/// The cores this process may use, counting one when the platform cannot say.
pub(crate) fn available_cores() -> usize {
    std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
}

#[cfg(test)]
mod tests {
    use gateway_whisper_ffi::SamplingStrategy;

    use super::*;

    fn greedy() -> FullParams {
        FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
    }

    fn seconds(seconds: usize) -> usize {
        seconds * EnginePolicy::SAMPLE_RATE
    }

    /// The params an interim profile on 8 cores with `max_tokens` applies to
    /// a pass whose encoder context is `audio_ctx`, or whisper's when `None`.
    fn interim_params(audio_ctx: Option<c_int>, max_tokens: c_int) -> FullParams {
        let mut expected = greedy();
        expected.set_single_segment(true);
        expected.set_n_threads(4);
        expected.set_temperature_inc(0.0);
        if let Some(frames) = audio_ctx {
            expected.set_audio_ctx(frames);
        }
        expected.set_max_tokens(max_tokens);
        expected.set_token_timestamps(true);
        expected.set_no_timestamps(false);
        expected
    }

    /// `FullParams` keeps its members private, so its derived `Debug` output
    /// is the only view of what `apply` set.
    fn assert_applies_as(profile: RoleProfile, samples: usize, expected: &FullParams) {
        let mut applied = greedy();
        profile.apply(&mut applied, samples);
        let applied = format!("{applied:?}");
        assert_ne!(
            applied,
            format!("{:?}", greedy()),
            "Debug output must show the members apply sets"
        );
        assert_eq!(applied, format!("{expected:?}"), "{samples} samples");
    }

    #[test]
    fn interim_apply_sets_every_profile_member_on_the_whisper_params() {
        assert_applies_as(
            RoleProfile::interim(15, 8),
            seconds(15),
            &interim_params(Some(896), 60),
        );
    }

    #[test]
    fn interim_apply_sizes_the_encoder_context_to_each_pass_rather_than_the_configured_window() {
        let profile = RoleProfile::interim(10, 8);
        assert_applies_as(profile, seconds(3), &interim_params(Some(512), 40));
        assert_applies_as(profile, seconds(10), &interim_params(Some(640), 40));
        assert_applies_as(profile, seconds(12), &interim_params(Some(768), 40));
        assert_applies_as(profile, seconds(27), &interim_params(None, 40));
    }

    #[test]
    fn final_apply_sets_segments_and_threads_and_leaves_other_whisper_defaults() {
        let mut expected = greedy();
        expected.set_single_segment(false);
        expected.set_n_threads(4);
        assert_applies_as(RoleProfile::final_pass(8), seconds(3), &expected);
        assert_applies_as(RoleProfile::final_pass(8), seconds(12), &expected);
    }

    #[test]
    fn interim_audio_ctx_covers_the_window_and_margin_in_whole_64_frame_blocks() {
        assert_eq!(
            interim_audio_ctx(seconds(8)),
            Some(576),
            "528 frames round up to 576"
        );
        assert_eq!(
            interim_audio_ctx(seconds(12)),
            Some(768),
            "728 frames round up to 768"
        );
        assert_eq!(
            interim_audio_ctx(seconds(15)),
            Some(896),
            "878 frames round up to 896"
        );
        assert_eq!(
            interim_audio_ctx(seconds(26)),
            Some(1472),
            "1428 frames round up to the last block within the full context"
        );
    }

    #[test]
    fn interim_audio_ctx_counts_a_partial_encoder_frame_as_a_whole_one() {
        assert_eq!(
            interim_audio_ctx(163_840),
            Some(640),
            "512 frames and the margin fill 640"
        );
        assert_eq!(
            interim_audio_ctx(163_841),
            Some(704),
            "one more sample starts frame 513, and 641 frames round up to 704"
        );
    }

    #[test]
    fn interim_audio_ctx_never_drops_below_512_frames() {
        assert_eq!(interim_audio_ctx(0), Some(512));
        assert_eq!(interim_audio_ctx(seconds(1)), Some(512));
        assert_eq!(
            interim_audio_ctx(40_000),
            Some(512),
            "a 2.5 s window takes the floor"
        );
        assert_eq!(
            interim_audio_ctx(seconds(7)),
            Some(512),
            "478 frames round to the floor"
        );
    }

    #[test]
    fn interim_audio_ctx_past_the_full_encoder_context_keeps_the_full_context() {
        assert_eq!(
            interim_audio_ctx(seconds(27)),
            None,
            "1478 frames round up to 1536, past whisper's 1500"
        );
        assert_eq!(interim_audio_ctx(usize::MAX), None);
    }

    #[test]
    fn decode_threads_take_half_the_cores_between_one_and_four() {
        assert_eq!(decode_threads(0), 1);
        assert_eq!(decode_threads(1), 1);
        assert_eq!(decode_threads(2), 1);
        assert_eq!(decode_threads(6), 3);
        assert_eq!(decode_threads(8), 4);
        assert_eq!(decode_threads(32), 4);
    }

    #[test]
    fn interim_profile_disables_fallback_and_bounds_context_tokens_and_threads() {
        assert_eq!(
            RoleProfile::interim(15, 8),
            RoleProfile {
                single_segment: true,
                temperature_inc: Some(0.0),
                fit_audio_ctx: true,
                max_tokens: Some(60),
                n_threads: 4,
                token_timestamps: true,
            }
        );
        assert_eq!(RoleProfile::interim(12, 2).n_threads, 1);
    }

    #[test]
    fn only_the_interim_role_requests_token_timestamps() {
        assert!(RoleProfile::interim(15, 8).token_timestamps);
        assert!(!RoleProfile::final_pass(8).token_timestamps);
    }

    #[test]
    fn interim_max_tokens_allow_four_per_window_second() {
        assert_eq!(RoleProfile::interim(1, 8).max_tokens, Some(4));
        assert_eq!(RoleProfile::interim(8, 8).max_tokens, Some(32));
        assert_eq!(
            RoleProfile::interim(u64::MAX, 8).max_tokens,
            Some(c_int::MAX)
        );
    }

    #[test]
    fn final_profile_keeps_whisper_fallback_full_context_and_unlimited_tokens() {
        assert_eq!(
            RoleProfile::final_pass(8),
            RoleProfile {
                single_segment: false,
                temperature_inc: None,
                fit_audio_ctx: false,
                max_tokens: None,
                n_threads: 4,
                token_timestamps: false,
            }
        );
        assert_eq!(RoleProfile::final_pass(32).n_threads, 4);
        assert_eq!(RoleProfile::final_pass(1).n_threads, 1);
    }
}
