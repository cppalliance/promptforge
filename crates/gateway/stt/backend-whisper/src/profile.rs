//! Per-role whisper decode settings.

use std::ffi::c_int;
use std::num::NonZeroUsize;

use gateway_whisper_ffi::FullParams;

/// b4938 builds its fallback temperature ladder only for a step above zero.
const NO_TEMPERATURE_FALLBACK: f32 = 0.0;
const ENCODER_FRAMES_PER_SECOND: u64 = 50;
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
    pub(crate) audio_ctx: Option<c_int>,
    pub(crate) max_tokens: Option<c_int>,
    pub(crate) n_threads: c_int,
}

impl RoleProfile {
    /// The interim role for a `window_seconds` window on `cores` cores.
    pub(crate) fn interim(window_seconds: u64, cores: usize) -> Self {
        let max_tokens = window_seconds.saturating_mul(INTERIM_TOKENS_PER_SECOND);
        Self {
            single_segment: true,
            temperature_inc: Some(NO_TEMPERATURE_FALLBACK),
            audio_ctx: interim_audio_ctx(window_seconds),
            max_tokens: Some(c_int::try_from(max_tokens).unwrap_or(c_int::MAX)),
            n_threads: decode_threads(cores),
        }
    }

    /// The final role on `cores` cores.
    pub(crate) fn final_pass(cores: usize) -> Self {
        Self {
            single_segment: false,
            temperature_inc: None,
            audio_ctx: None,
            max_tokens: None,
            n_threads: decode_threads(cores),
        }
    }

    pub(crate) fn apply(&self, params: &mut FullParams) {
        params.set_single_segment(self.single_segment);
        params.set_n_threads(self.n_threads);
        if let Some(value) = self.temperature_inc {
            params.set_temperature_inc(value);
        }
        if let Some(value) = self.audio_ctx {
            params.set_audio_ctx(value);
        }
        if let Some(value) = self.max_tokens {
            params.set_max_tokens(value);
        }
    }
}

/// Encoder frames covering a `window_seconds` window plus margin, or `None`
/// when that reaches past the model's full context.
pub(crate) fn interim_audio_ctx(window_seconds: u64) -> Option<c_int> {
    let frames = window_seconds
        .saturating_mul(ENCODER_FRAMES_PER_SECOND)
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

    /// `FullParams` keeps its members private, so its derived `Debug` output
    /// is the only view of what `apply` set.
    fn assert_applies_as(profile: RoleProfile, expected: &FullParams) {
        let mut applied = greedy();
        profile.apply(&mut applied);
        let applied = format!("{applied:?}");
        assert_ne!(
            applied,
            format!("{:?}", greedy()),
            "Debug output must show the members apply sets"
        );
        assert_eq!(applied, format!("{expected:?}"));
    }

    #[test]
    fn interim_apply_sets_every_profile_member_on_the_whisper_params() {
        let mut expected = greedy();
        expected.set_single_segment(true);
        expected.set_n_threads(4);
        expected.set_temperature_inc(0.0);
        expected.set_audio_ctx(896);
        expected.set_max_tokens(60);
        assert_applies_as(RoleProfile::interim(15, 8), &expected);
    }

    #[test]
    fn final_apply_sets_segments_and_threads_and_leaves_other_whisper_defaults() {
        let mut expected = greedy();
        expected.set_single_segment(false);
        expected.set_n_threads(4);
        assert_applies_as(RoleProfile::final_pass(8), &expected);
    }

    #[test]
    fn interim_audio_ctx_covers_the_window_and_margin_in_whole_64_frame_blocks() {
        assert_eq!(
            interim_audio_ctx(8),
            Some(576),
            "528 frames round up to 576"
        );
        assert_eq!(
            interim_audio_ctx(12),
            Some(768),
            "728 frames round up to 768"
        );
        assert_eq!(
            interim_audio_ctx(15),
            Some(896),
            "878 frames round up to 896"
        );
        assert_eq!(
            interim_audio_ctx(26),
            Some(1472),
            "1428 frames round up to the last block within the full context"
        );
    }

    #[test]
    fn interim_audio_ctx_never_drops_below_512_frames() {
        assert_eq!(interim_audio_ctx(0), Some(512));
        assert_eq!(interim_audio_ctx(1), Some(512));
        assert_eq!(
            interim_audio_ctx(7),
            Some(512),
            "478 frames round to the floor"
        );
    }

    #[test]
    fn interim_audio_ctx_past_the_full_encoder_context_keeps_the_full_context() {
        assert_eq!(
            interim_audio_ctx(27),
            None,
            "1478 frames round up to 1536, past whisper's 1500"
        );
        assert_eq!(interim_audio_ctx(u64::MAX), None);
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
                audio_ctx: Some(896),
                max_tokens: Some(60),
                n_threads: 4,
            }
        );
        assert_eq!(RoleProfile::interim(12, 2).n_threads, 1);
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
                audio_ctx: None,
                max_tokens: None,
                n_threads: 4,
            }
        );
        assert_eq!(RoleProfile::final_pass(32).n_threads, 4);
        assert_eq!(RoleProfile::final_pass(1).n_threads, 1);
    }
}
