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
struct Ahead {
    n_text_layer: c_int,
    n_head: c_int,
}

/// Alignment-head slice in `whisper_context_params`.
#[repr(C)]
struct Aheads {
    n_heads: usize,
    heads: *const Ahead,
}

/// Parameters passed by value to `whisper_init_from_file_with_params`.
#[repr(C)]
pub(crate) struct ContextParams {
    use_gpu: bool,
    flash_attn: bool,
    gpu_device: c_int,
    dtw_token_timestamps: bool,
    dtw_aheads_preset: c_int,
    dtw_n_top: c_int,
    dtw_aheads: Aheads,
    dtw_mem_size: usize,
}

/// Greedy-decoder members embedded in `whisper_full_params`.
#[repr(C)]
pub(crate) struct GreedyParams {
    pub(crate) best_of: c_int,
}

/// Beam-search members embedded in `whisper_full_params`.
#[repr(C)]
struct BeamSearchParams {
    beam_size: c_int,
    patience: f32,
}

/// Whisper's built-in voice-activity detector settings.
#[repr(C)]
struct VadParams {
    threshold: f32,
    min_speech_duration_ms: c_int,
    min_silence_duration_ms: c_int,
    max_speech_duration_s: f32,
    speech_pad_ms: c_int,
    samples_overlap: f32,
}

/// Parameters passed by value to `whisper_full_with_state`.
///
/// Field order matches `struct whisper_full_params` in whisper.cpp b4938.
#[repr(C)]
pub(crate) struct FullParams {
    strategy: c_int,
    n_threads: c_int,
    n_max_text_ctx: c_int,
    offset_ms: c_int,
    duration_ms: c_int,
    pub(crate) translate: bool,
    pub(crate) no_context: bool,
    pub(crate) no_timestamps: bool,
    pub(crate) single_segment: bool,
    pub(crate) print_special: bool,
    pub(crate) print_progress: bool,
    pub(crate) print_realtime: bool,
    pub(crate) print_timestamps: bool,
    token_timestamps: bool,
    thold_pt: f32,
    thold_ptsum: f32,
    max_len: c_int,
    split_on_word: bool,
    max_tokens: c_int,
    debug_mode: bool,
    audio_ctx: c_int,
    tdrz_enable: bool,
    suppress_regex: *const c_char,
    pub(crate) initial_prompt: *const c_char,
    carry_initial_prompt: bool,
    prompt_tokens: *const c_int,
    prompt_n_tokens: c_int,
    pub(crate) language: *const c_char,
    pub(crate) detect_language: bool,
    pub(crate) suppress_blank: bool,
    pub(crate) suppress_nst: bool,
    temperature: f32,
    max_initial_ts: f32,
    length_penalty: f32,
    temperature_inc: f32,
    entropy_thold: f32,
    logprob_thold: f32,
    no_speech_thold: f32,
    pub(crate) greedy: GreedyParams,
    beam_search: BeamSearchParams,
    new_segment_callback: *mut c_void,
    new_segment_callback_user_data: *mut c_void,
    progress_callback: *mut c_void,
    progress_callback_user_data: *mut c_void,
    encoder_begin_callback: *mut c_void,
    encoder_begin_callback_user_data: *mut c_void,
    pub(crate) abort_callback: AbortCallback,
    pub(crate) abort_callback_user_data: *mut c_void,
    logits_filter_callback: *mut c_void,
    logits_filter_callback_user_data: *mut c_void,
    grammar_rules: *const *const c_void,
    n_grammar_rules: usize,
    i_start_rule: usize,
    grammar_penalty: f32,
    vad: bool,
    vad_model_path: *const c_char,
    vad_params: VadParams,
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

/// `ggml_abort_callback` from the pinned b4938 ggml.h. whisper calls it after
/// each encoder pass and decoder step, and a true return ends the pass.
type AbortCallback = Option<extern "C" fn(*mut c_void) -> bool>;

/// `ggml_log_callback` from the pinned b4938 ggml.h. The level is a C
/// `enum ggml_log_level`, which the ABI passes as `c_int`.
pub(crate) type LogCallback = Option<extern "C" fn(c_int, *const c_char, *mut c_void)>;
pub(crate) type LogSet = unsafe extern "C" fn(LogCallback, *mut c_void);
