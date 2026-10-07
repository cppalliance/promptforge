//! Safe full-decoding parameters.

use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::WhisperError;
use crate::raw;

#[derive(Debug)]
enum Language {
    Unchanged,
    Auto,
    Explicit(CString),
}

/// Decoding strategy for a full whisper pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SamplingStrategy {
    /// Greedy decoding with `best_of` candidates.
    Greedy {
        /// Number of candidates considered at each step.
        best_of: c_int,
    },
}

impl SamplingStrategy {
    pub(crate) fn native_value(self) -> c_int {
        match self {
            Self::Greedy { .. } => 0,
        }
    }
}

/// Safe settings for one full whisper decoding pass.
#[derive(Debug)]
pub struct FullParams {
    pub(crate) strategy: SamplingStrategy,
    language: Language,
    initial_prompt: Option<CString>,
    translate: Option<bool>,
    no_context: Option<bool>,
    single_segment: Option<bool>,
    no_timestamps: Option<bool>,
    print_special: Option<bool>,
    print_progress: Option<bool>,
    print_realtime: Option<bool>,
    print_timestamps: Option<bool>,
    suppress_blank: Option<bool>,
    suppress_nst: Option<bool>,
    temperature_inc: Option<f32>,
    audio_ctx: Option<c_int>,
    max_tokens: Option<c_int>,
    n_threads: Option<c_int>,
    token_timestamps: Option<bool>,
    entropy_thold: Option<f32>,
    logprob_thold: Option<f32>,
    no_speech_thold: Option<f32>,
    abort_flag: Option<Arc<AtomicBool>>,
}

impl FullParams {
    /// Creates settings over whisper.cpp's defaults for `strategy`.
    #[must_use]
    pub fn new(strategy: SamplingStrategy) -> Self {
        Self {
            strategy,
            language: Language::Unchanged,
            initial_prompt: None,
            translate: None,
            no_context: None,
            single_segment: None,
            no_timestamps: None,
            print_special: None,
            print_progress: None,
            print_realtime: None,
            print_timestamps: None,
            suppress_blank: None,
            suppress_nst: None,
            temperature_inc: None,
            audio_ctx: None,
            max_tokens: None,
            n_threads: None,
            token_timestamps: None,
            entropy_thold: None,
            logprob_thold: None,
            no_speech_thold: None,
            abort_flag: None,
        }
    }

    /// Sets the spoken language, or restores automatic detection with `None`.
    ///
    /// # Errors
    /// Returns [`WhisperError::InteriorNull`] when `language` contains a null.
    pub fn set_language(&mut self, language: Option<&str>) -> Result<(), WhisperError> {
        self.language = match language {
            Some(language) => Language::Explicit(CString::new(language).map_err(|source| {
                WhisperError::InteriorNull {
                    value: "whisper language",
                    source,
                }
            })?),
            None => Language::Auto,
        };
        Ok(())
    }

    /// Sets whether whisper translates the result into English.
    pub fn set_translate(&mut self, value: bool) {
        self.translate = Some(value);
    }

    /// Sets whether whisper discards context from the previous pass.
    pub fn set_no_context(&mut self, value: bool) {
        self.no_context = Some(value);
    }

    /// Sets whether whisper emits one segment for the whole input.
    pub fn set_single_segment(&mut self, value: bool) {
        self.single_segment = Some(value);
    }

    /// Sets whether timestamp tokens are disabled.
    pub fn set_no_timestamps(&mut self, value: bool) {
        self.no_timestamps = Some(value);
    }

    /// Sets whether special tokens print to whisper's output stream.
    pub fn set_print_special(&mut self, value: bool) {
        self.print_special = Some(value);
    }

    /// Sets whether native progress prints to whisper's output stream.
    pub fn set_print_progress(&mut self, value: bool) {
        self.print_progress = Some(value);
    }

    /// Sets whether partial text prints during inference.
    pub fn set_print_realtime(&mut self, value: bool) {
        self.print_realtime = Some(value);
    }

    /// Sets whether timestamps print beside native text output.
    pub fn set_print_timestamps(&mut self, value: bool) {
        self.print_timestamps = Some(value);
    }

    /// Sets whether blank tokens are suppressed.
    pub fn set_suppress_blank(&mut self, value: bool) {
        self.suppress_blank = Some(value);
    }

    /// Sets whether non-speech tokens are suppressed.
    pub fn set_suppress_nst(&mut self, value: bool) {
        self.suppress_nst = Some(value);
    }

    /// Sets the temperature step for fallback retries; zero or below
    /// disables the fallback.
    pub fn set_temperature_inc(&mut self, value: f32) {
        self.temperature_inc = Some(value);
    }

    /// Sets the encoder's audio context in frames; zero keeps the model's
    /// full 1500-frame context, and a value above it fails the pass.
    pub fn set_audio_ctx(&mut self, value: c_int) {
        self.audio_ctx = Some(value);
    }

    /// Sets the most text tokens per segment; zero leaves it unlimited.
    pub fn set_max_tokens(&mut self, value: c_int) {
        self.max_tokens = Some(value);
    }

    /// Sets the CPU thread count for the pass.
    pub fn set_n_threads(&mut self, value: c_int) {
        self.n_threads = Some(value);
    }

    /// Sets whether whisper computes per-token timestamps.
    pub fn set_token_timestamps(&mut self, value: bool) {
        self.token_timestamps = Some(value);
    }

    /// Sets the token-entropy threshold below which a decode counts as
    /// repetitive and falls back to the next temperature.
    pub fn set_entropy_thold(&mut self, value: f32) {
        self.entropy_thold = Some(value);
    }

    /// Sets the mean log-probability threshold below which a decode falls
    /// back to the next temperature.
    pub fn set_logprob_thold(&mut self, value: f32) {
        self.logprob_thold = Some(value);
    }

    /// Sets the no-speech probability above which, together with a failed
    /// log-probability check, whisper treats the window as silence.
    pub fn set_no_speech_thold(&mut self, value: f32) {
        self.no_speech_thold = Some(value);
    }

    /// Sets the explicit decoder-conditioning prompt.
    ///
    /// # Errors
    /// Returns [`WhisperError::InteriorNull`] when `prompt` contains a null.
    pub fn set_initial_prompt(&mut self, prompt: &str) -> Result<(), WhisperError> {
        self.initial_prompt =
            Some(
                CString::new(prompt).map_err(|source| WhisperError::InteriorNull {
                    value: "whisper initial prompt",
                    source,
                })?,
            );
        Ok(())
    }

    /// Ends the pass early once `flag` reads true.
    ///
    /// whisper reads the flag after each encoder pass and decoder step, never
    /// inside one, and a set flag ends the pass with
    /// [`WhisperError::Inference`]. A flag already set when the pass begins is
    /// first read after its first encoder pass.
    pub fn set_abort_flag(&mut self, flag: Arc<AtomicBool>) {
        self.abort_flag = Some(flag);
    }

    pub(crate) fn apply(&self, native: &mut raw::FullParams) {
        let SamplingStrategy::Greedy { best_of } = self.strategy;
        native.greedy.best_of = best_of;
        match &self.language {
            Language::Unchanged => {}
            Language::Auto => {
                native.language = ptr::null();
                native.detect_language = true;
            }
            Language::Explicit(language) => {
                native.language = language.as_ptr();
                native.detect_language = false;
            }
        }
        native.initial_prompt = self
            .initial_prompt
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        apply_value(&mut native.translate, self.translate);
        apply_value(&mut native.no_context, self.no_context);
        apply_value(&mut native.single_segment, self.single_segment);
        apply_value(&mut native.no_timestamps, self.no_timestamps);
        apply_value(&mut native.print_special, self.print_special);
        apply_value(&mut native.print_progress, self.print_progress);
        apply_value(&mut native.print_realtime, self.print_realtime);
        apply_value(&mut native.print_timestamps, self.print_timestamps);
        apply_value(&mut native.suppress_blank, self.suppress_blank);
        apply_value(&mut native.suppress_nst, self.suppress_nst);
        apply_value(&mut native.temperature_inc, self.temperature_inc);
        apply_value(&mut native.audio_ctx, self.audio_ctx);
        apply_value(&mut native.max_tokens, self.max_tokens);
        apply_value(&mut native.n_threads, self.n_threads);
        apply_value(&mut native.token_timestamps, self.token_timestamps);
        apply_value(&mut native.entropy_thold, self.entropy_thold);
        apply_value(&mut native.logprob_thold, self.logprob_thold);
        apply_value(&mut native.no_speech_thold, self.no_speech_thold);
        if let Some(flag) = &self.abort_flag {
            native.abort_callback = Some(abort_requested);
            native.abort_callback_user_data = Arc::as_ptr(flag).cast_mut().cast();
        }
    }
}

/// The pinned b4938 `ggml_abort_callback` that [`FullParams::apply`] installs
/// for an abort flag: `data` is that flag's address, and a true answer ends
/// the pass.
///
/// Never panics: unwinding across the C boundary would abort the process, so
/// the body holds no locks and has no unwrap paths.
extern "C" fn abort_requested(data: *mut c_void) -> bool {
    if data.is_null() {
        return false;
    }
    // SAFETY: `FullParams::apply` passes `Arc::as_ptr` of the flag its
    // `FullParams` owns, and `WhisperState::full` borrows those params for the
    // whole whisper call that invokes this callback, so `data` points to a
    // live `AtomicBool`; the null case returned above.
    let flag = unsafe { &*data.cast::<AtomicBool>() };
    flag.load(Ordering::Acquire)
}

fn apply_value<T: Copy>(target: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *target = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag_data(flag: &Arc<AtomicBool>) -> *mut c_void {
        Arc::as_ptr(flag).cast_mut().cast()
    }

    /// Native parameters with the abort members at whisper's null defaults.
    fn null_native() -> raw::FullParams {
        // SAFETY: every raw::FullParams member is an integer, float, bool, raw
        // pointer, nullable function pointer, or a struct of those, and
        // all-zero bytes are a valid value of each.
        unsafe { std::mem::zeroed() }
    }

    /// Native parameters with the decode members at whisper's b4938 defaults.
    fn default_native() -> raw::FullParams {
        let mut native = null_native();
        native.n_threads = 4;
        native.temperature_inc = 0.2;
        native.entropy_thold = 2.4;
        native.logprob_thold = -1.0;
        native.no_speech_thold = 0.6;
        native
    }

    #[test]
    fn each_decode_setter_lands_in_its_native_member() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_temperature_inc(0.0);
        params.set_audio_ctx(768);
        params.set_max_tokens(44);
        params.set_n_threads(2);
        params.set_token_timestamps(true);
        params.set_entropy_thold(2.8);
        params.set_logprob_thold(-0.5);
        params.set_no_speech_thold(0.3);
        let mut native = default_native();
        params.apply(&mut native);
        assert_eq!(native.temperature_inc.to_bits(), 0.0_f32.to_bits());
        assert_eq!(native.audio_ctx, 768);
        assert_eq!(native.max_tokens, 44);
        assert_eq!(native.n_threads, 2);
        assert!(native.token_timestamps);
        assert_eq!(native.entropy_thold.to_bits(), 2.8_f32.to_bits());
        assert_eq!(native.logprob_thold.to_bits(), (-0.5_f32).to_bits());
        assert_eq!(native.no_speech_thold.to_bits(), 0.3_f32.to_bits());
    }

    #[test]
    fn unset_decode_setters_keep_whisper_defaults() {
        let mut native = default_native();
        FullParams::new(SamplingStrategy::Greedy { best_of: 1 }).apply(&mut native);
        let defaults = default_native();
        assert_eq!(
            native.temperature_inc.to_bits(),
            defaults.temperature_inc.to_bits()
        );
        assert_eq!(native.audio_ctx, defaults.audio_ctx);
        assert_eq!(native.max_tokens, defaults.max_tokens);
        assert_eq!(native.n_threads, defaults.n_threads);
        assert_eq!(native.token_timestamps, defaults.token_timestamps);
        assert_eq!(
            native.entropy_thold.to_bits(),
            defaults.entropy_thold.to_bits()
        );
        assert_eq!(
            native.logprob_thold.to_bits(),
            defaults.logprob_thold.to_bits()
        );
        assert_eq!(
            native.no_speech_thold.to_bits(),
            defaults.no_speech_thold.to_bits()
        );
    }

    #[test]
    fn abort_requested_reads_the_flag_at_its_data_pointer() {
        assert!(!abort_requested(ptr::null_mut()), "null data never aborts");
        let flag = Arc::new(AtomicBool::new(false));
        assert!(
            !abort_requested(flag_data(&flag)),
            "an unset flag does not abort"
        );
        flag.store(true, Ordering::Release);
        assert!(abort_requested(flag_data(&flag)), "a set flag aborts");
    }

    #[test]
    fn apply_installs_the_abort_callback_and_flag_only_when_a_flag_is_set() {
        let mut native = null_native();
        FullParams::new(SamplingStrategy::Greedy { best_of: 1 }).apply(&mut native);
        assert!(
            native.abort_callback.is_none(),
            "no flag leaves the callback null"
        );
        assert!(
            native.abort_callback_user_data.is_null(),
            "no flag leaves the user data null"
        );

        let flag = Arc::new(AtomicBool::new(false));
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_abort_flag(Arc::clone(&flag));
        let mut native = null_native();
        params.apply(&mut native);
        assert!(
            ptr::eq(native.abort_callback_user_data, flag_data(&flag)),
            "the user data is the flag's address"
        );
        let callback = native
            .abort_callback
            .expect("a set flag installs the callback");
        assert!(!callback(native.abort_callback_user_data));
        flag.store(true, Ordering::Release);
        assert!(
            callback(native.abort_callback_user_data),
            "the installed callback reads the flag"
        );
    }
}
