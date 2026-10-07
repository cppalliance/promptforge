//! Bridges whisper.cpp and ggml C logging into `tracing` events.

use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int, c_void};

/// The `tracing` target the gateway's default `EnvFilter` gates on.
const TARGET: &str = "whisper_cpp";

/// The pinned b4938 `ggml_log_callback` installed process-wide by
/// [`WhisperLibrary::set_log_callback`](crate::WhisperLibrary::set_log_callback).
///
/// Never panics: unwinding across the C boundary would abort the process, so
/// the body holds no locks, performs no indexing, and has no unwrap paths.
extern "C" fn tracing_bridge(level: c_int, text: *const c_char, _user_data: *mut c_void) {
    if text.is_null() {
        return;
    }
    // SAFETY: the ggml_log_callback contract passes `text` as a valid,
    // null-terminated string borrowed for the duration of the call; the null
    // case returned above.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let message = render_text(bytes);
    // The event macros need a static level for their callsite, so the mapped
    // level selects the macro.
    let level = bridged_level(level, &message);
    if level == tracing::Level::ERROR {
        tracing::error!(target: TARGET, "{message}");
    } else if level == tracing::Level::WARN {
        tracing::warn!(target: TARGET, "{message}");
    } else if level == tracing::Level::INFO {
        tracing::info!(target: TARGET, "{message}");
    } else if level == tracing::Level::DEBUG {
        tracing::debug!(target: TARGET, "{message}");
    } else {
        tracing::trace!(target: TARGET, "{message}");
    }
}

/// Line openings `whisper_vad_detect_speech_no_reset` writes at INFO on every
/// call in the pinned b4938 `src/whisper.cpp`. Streaming detection calls it
/// about 31 times a second; its failure lines share the function prefix and
/// keep their level.
const VAD_PER_CALL_LINES: [&str; 5] = [
    "whisper_vad_detect_speech_no_reset: detecting speech in ",
    "whisper_vad_detect_speech_no_reset: n_chunks: ",
    "whisper_vad_detect_speech_no_reset: props size: ",
    "whisper_vad_detect_speech_no_reset: chunk_len: ",
    "whisper_vad_detect_speech_no_reset: vad time = ",
];

/// Picks the `tracing` level for one rendered line: the per-call speech
/// detection lines drop to [`tracing::Level::TRACE`], and every other line
/// keeps its mapped native level.
#[must_use]
fn bridged_level(level: c_int, message: &str) -> tracing::Level {
    if VAD_PER_CALL_LINES
        .iter()
        .any(|line| message.starts_with(line))
    {
        tracing::Level::TRACE
    } else {
        tracing_level(level)
    }
}

/// The callback as a function pointer for `whisper_log_set`.
pub(crate) const TRACING_BRIDGE: crate::raw::LogCallback = Some(tracing_bridge);

/// Maps a pinned b4938 `enum ggml_log_level` value onto a `tracing` level.
///
/// `GGML_LOG_LEVEL_CONT` continuation fragments, `GGML_LOG_LEVEL_NONE`, and
/// values outside the pinned enum degrade to [`tracing::Level::DEBUG`], so a
/// runtime emitting an unknown level cannot spam the terminal.
#[must_use]
fn tracing_level(level: c_int) -> tracing::Level {
    const GGML_LOG_LEVEL_INFO: c_int = 2;
    const GGML_LOG_LEVEL_WARN: c_int = 3;
    const GGML_LOG_LEVEL_ERROR: c_int = 4;
    match level {
        GGML_LOG_LEVEL_ERROR => tracing::Level::ERROR,
        GGML_LOG_LEVEL_WARN => tracing::Level::WARN,
        GGML_LOG_LEVEL_INFO => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    }
}

/// Renders one C log fragment for `tracing`: lossy UTF-8 with the trailing
/// newline whisper.cpp appends to every line removed.
#[must_use]
fn render_text(bytes: &[u8]) -> Cow<'_, str> {
    match String::from_utf8_lossy(bytes) {
        Cow::Borrowed(text) => Cow::Borrowed(text.trim_end_matches(['\r', '\n'])),
        Cow::Owned(mut text) => {
            let trimmed_len = text.trim_end_matches(['\r', '\n']).len();
            text.truncate(trimmed_len);
            Cow::Owned(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_b4938_ggml_levels_map_to_tracing_levels() {
        // Values from `enum ggml_log_level` in the pinned b4938 ggml.h.
        assert_eq!(tracing_level(4), tracing::Level::ERROR);
        assert_eq!(tracing_level(3), tracing::Level::WARN);
        assert_eq!(tracing_level(2), tracing::Level::INFO);
        assert_eq!(tracing_level(1), tracing::Level::DEBUG);
        assert_eq!(tracing_level(5), tracing::Level::DEBUG, "CONT fragments");
        assert_eq!(tracing_level(0), tracing::Level::DEBUG, "NONE");
        assert_eq!(tracing_level(99), tracing::Level::DEBUG, "unknown level");
        assert_eq!(tracing_level(-1), tracing::Level::DEBUG, "negative level");
    }

    #[test]
    fn render_text_trims_the_trailing_newline() {
        assert_eq!(render_text(b"ggml_cuda_init: ok\n"), "ggml_cuda_init: ok");
        assert_eq!(render_text(b"carriage return\r\n"), "carriage return");
        assert_eq!(render_text(b"\n"), "");
    }

    #[test]
    fn render_text_consumes_every_byte_without_a_nul_terminator() {
        assert_eq!(render_text(b"no terminator"), "no terminator");
    }

    #[test]
    fn render_text_is_lossy_for_non_utf8_bytes() {
        assert_eq!(render_text(b"bad \xFF byte\n"), "bad \u{FFFD} byte");
    }

    #[test]
    fn callback_drops_null_text_without_an_event() {
        tracing_bridge(2, std::ptr::null(), std::ptr::null_mut());
    }

    #[test]
    fn callback_consumes_a_nul_terminated_c_string() {
        tracing_bridge(2, c"whisper: loaded model\n".as_ptr(), std::ptr::null_mut());
        tracing_bridge(5, c" continuation".as_ptr(), std::ptr::null_mut());
    }

    #[test]
    fn per_call_vad_lines_demote_to_trace() {
        // Rendered from the format strings in
        // `whisper_vad_detect_speech_no_reset`, pinned b4938 src/whisper.cpp.
        for line in [
            "whisper_vad_detect_speech_no_reset: detecting speech in 512 samples",
            "whisper_vad_detect_speech_no_reset: n_chunks: 1",
            "whisper_vad_detect_speech_no_reset: props size: 1",
            "whisper_vad_detect_speech_no_reset: chunk_len: 188 < n_window: 512",
            "whisper_vad_detect_speech_no_reset: vad time = 0.31 ms processing 512 samples",
        ] {
            assert_eq!(bridged_level(2, line), tracing::Level::TRACE, "{line}");
        }
        tracing_bridge(
            2,
            c"whisper_vad_detect_speech_no_reset: n_chunks: 1\n".as_ptr(),
            std::ptr::null_mut(),
        );
    }

    #[test]
    fn vad_failures_and_load_lines_keep_their_level() {
        assert_eq!(
            bridged_level(
                4,
                "whisper_vad_detect_speech_no_reset: failed to allocate the compute buffer"
            ),
            tracing::Level::ERROR
        );
        assert_eq!(
            bridged_level(
                4,
                "whisper_vad_detect_speech_no_reset: failed to compute VAD graph"
            ),
            tracing::Level::ERROR
        );
        assert_eq!(
            bridged_level(2, "whisper_vad_init_with_params: model type: silero-16k"),
            tracing::Level::INFO
        );
        assert_eq!(
            bridged_level(2, "whisper_full_with_state: n_chunks: 1"),
            tracing::Level::INFO,
            "only the VAD detector's lines are demoted"
        );
    }
}
