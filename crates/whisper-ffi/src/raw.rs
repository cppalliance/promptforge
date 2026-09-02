//! Pinned whisper.cpp b4938 C ABI declarations.

use std::ffi::{c_char, c_int, c_void};

/// Opaque `whisper_context`.
#[repr(C)]
pub(crate) struct Context {
    _private: [u8; 0],
}

/// Opaque `whisper_state`.
#[repr(C)]
pub(crate) struct State {
    _private: [u8; 0],
}

/// One alignment head in `whisper_context_params`.
#[repr(C)]
pub(crate) struct Ahead {
    pub(crate) n_text_layer: c_int,
    pub(crate) n_head: c_int,
}

/// Alignment-head slice in `whisper_context_params`.
#[repr(C)]
pub(crate) struct Aheads {
    pub(crate) n_heads: usize,
    pub(crate) heads: *const Ahead,
}

/// Parameters passed by value to `whisper_init_from_file_with_params`.
#[repr(C)]
pub(crate) struct ContextParams {
    pub(crate) use_gpu: bool,
    pub(crate) flash_attn: bool,
    pub(crate) gpu_device: c_int,
    pub(crate) dtw_token_timestamps: bool,
    pub(crate) dtw_aheads_preset: c_int,
    pub(crate) dtw_n_top: c_int,
    pub(crate) dtw_aheads: Aheads,
    pub(crate) dtw_mem_size: usize,
}

/// Greedy-decoder members embedded in `whisper_full_params`.
#[repr(C)]
pub(crate) struct GreedyParams {
    pub(crate) best_of: c_int,
}

/// Beam-search members embedded in `whisper_full_params`.
#[repr(C)]
pub(crate) struct BeamSearchParams {
    pub(crate) beam_size: c_int,
    pub(crate) patience: f32,
}

/// Whisper's built-in voice-activity detector settings.
#[repr(C)]
pub(crate) struct VadParams {
    pub(crate) threshold: f32,
    pub(crate) min_speech_duration_ms: c_int,
    pub(crate) min_silence_duration_ms: c_int,
    pub(crate) max_speech_duration_s: f32,
    pub(crate) speech_pad_ms: c_int,
    pub(crate) samples_overlap: f32,
}

/// Parameters passed by value to `whisper_full_with_state`.
///
/// Field order matches `struct whisper_full_params` in whisper.cpp b4938.
#[repr(C)]
pub(crate) struct FullParams {
    pub(crate) strategy: c_int,
    pub(crate) n_threads: c_int,
    pub(crate) n_max_text_ctx: c_int,
    pub(crate) offset_ms: c_int,
    pub(crate) duration_ms: c_int,
    pub(crate) translate: bool,
    pub(crate) no_context: bool,
    pub(crate) no_timestamps: bool,
    pub(crate) single_segment: bool,
    pub(crate) print_special: bool,
    pub(crate) print_progress: bool,
    pub(crate) print_realtime: bool,
    pub(crate) print_timestamps: bool,
    pub(crate) token_timestamps: bool,
    pub(crate) thold_pt: f32,
    pub(crate) thold_ptsum: f32,
    pub(crate) max_len: c_int,
    pub(crate) split_on_word: bool,
    pub(crate) max_tokens: c_int,
    pub(crate) debug_mode: bool,
    pub(crate) audio_ctx: c_int,
    pub(crate) tdrz_enable: bool,
    pub(crate) suppress_regex: *const c_char,
    pub(crate) initial_prompt: *const c_char,
    pub(crate) carry_initial_prompt: bool,
    pub(crate) prompt_tokens: *const c_int,
    pub(crate) prompt_n_tokens: c_int,
    pub(crate) language: *const c_char,
    pub(crate) detect_language: bool,
    pub(crate) suppress_blank: bool,
    pub(crate) suppress_nst: bool,
    pub(crate) temperature: f32,
    pub(crate) max_initial_ts: f32,
    pub(crate) length_penalty: f32,
    pub(crate) temperature_inc: f32,
    pub(crate) entropy_thold: f32,
    pub(crate) logprob_thold: f32,
    pub(crate) no_speech_thold: f32,
    pub(crate) greedy: GreedyParams,
    pub(crate) beam_search: BeamSearchParams,
    pub(crate) new_segment_callback: *mut c_void,
    pub(crate) new_segment_callback_user_data: *mut c_void,
    pub(crate) progress_callback: *mut c_void,
    pub(crate) progress_callback_user_data: *mut c_void,
    pub(crate) encoder_begin_callback: *mut c_void,
    pub(crate) encoder_begin_callback_user_data: *mut c_void,
    pub(crate) abort_callback: *mut c_void,
    pub(crate) abort_callback_user_data: *mut c_void,
    pub(crate) logits_filter_callback: *mut c_void,
    pub(crate) logits_filter_callback_user_data: *mut c_void,
    pub(crate) grammar_rules: *const *const c_void,
    pub(crate) n_grammar_rules: usize,
    pub(crate) i_start_rule: usize,
    pub(crate) grammar_penalty: f32,
    pub(crate) vad: bool,
    pub(crate) vad_model_path: *const c_char,
    pub(crate) vad_params: VadParams,
}

pub(crate) type ContextDefaultParams = unsafe extern "C" fn() -> ContextParams;
pub(crate) type InitFromFileWithParams =
    unsafe extern "C" fn(*const c_char, ContextParams) -> *mut Context;
pub(crate) type InitState = unsafe extern "C" fn(*mut Context) -> *mut State;
pub(crate) type Tokenize =
    unsafe extern "C" fn(*mut Context, *const c_char, *mut c_int, c_int) -> c_int;
pub(crate) type FullDefaultParams = unsafe extern "C" fn(c_int) -> FullParams;
pub(crate) type FullWithState =
    unsafe extern "C" fn(*mut Context, *mut State, FullParams, *const f32, c_int) -> c_int;
pub(crate) type FullNSegmentsFromState = unsafe extern "C" fn(*mut State) -> c_int;
pub(crate) type FullGetSegmentTextFromState =
    unsafe extern "C" fn(*mut State, c_int) -> *const c_char;
pub(crate) type PrintSystemInfo = unsafe extern "C" fn() -> *const c_char;
pub(crate) type Free = unsafe extern "C" fn(*mut Context);
pub(crate) type FreeState = unsafe extern "C" fn(*mut State);

pub(crate) type LogCallback = unsafe extern "C" fn(c_int, *const c_char, *mut c_void);
pub(crate) type LogSet = unsafe extern "C" fn(Option<LogCallback>, *mut c_void);
