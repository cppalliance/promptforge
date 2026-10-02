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
        apply_bool(&mut native.translate, self.translate);
        apply_bool(&mut native.no_context, self.no_context);
        apply_bool(&mut native.single_segment, self.single_segment);
        apply_bool(&mut native.no_timestamps, self.no_timestamps);
        apply_bool(&mut native.print_special, self.print_special);
        apply_bool(&mut native.print_progress, self.print_progress);
        apply_bool(&mut native.print_realtime, self.print_realtime);
        apply_bool(&mut native.print_timestamps, self.print_timestamps);
        apply_bool(&mut native.suppress_blank, self.suppress_blank);
        apply_bool(&mut native.suppress_nst, self.suppress_nst);
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

fn apply_bool(target: &mut bool, value: Option<bool>) {
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
