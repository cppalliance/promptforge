# gateway-whisper-ffi

- Unsafe is confined to this crate's FFI boundary.
- Raw whisper pointers live only behind Drop-owning wrappers (`ContextInner`, `WhisperState`, `VadContext`); never expose a raw `*mut` across the safe API, and never free or clone a pointer outside those Drop impls.
- The packaged C ABI pin covers struct sizes, the symbol set, and parameter layout. A retarget must update the pin and its size and offset assertions in the same change. Writing a `whisper_full_params` member that has no offset assertion yet means adding one first.
- The streaming VAD symbol set, resolved in `WhisperLibrary::load`: `whisper_vad_default_context_params`, `whisper_vad_init_from_file_with_params`, `whisper_vad_detect_speech_no_reset`, `whisper_vad_reset_state`, `whisper_vad_n_probs`, `whisper_vad_probs`, and `whisper_vad_free`. `whisper_vad_context_params` is pinned at size 12, alignment 4, offsets 0, 4, and 8.
- The Silero model pin that `VadContext` loads:
  - URL: https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin
  - SHA-256: `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987`
  - Size: 885,098 bytes
  - License: MIT
- A VAD context never enables the GPU: `VadContext::new` forces `use_gpu = false` and one thread, because detection computes on the CPU regardless and `use_gpu = true` aborted on CUDA builds (https://github.com/ggml-org/whisper.cpp/issues/3508).
- `whisper_vad_detect_speech_no_reset` writes four INFO lines per call (five with a partial window); `log.rs` demotes them to trace by their b4938 text, so a retarget rechecks those strings.
- Exported by the pinned b4938 library but not yet resolved, checked against `include/whisper.h` and `src/whisper.cpp` at the tag (the shared build compiles with `WHISPER_SHARED WHISPER_BUILD`, so every `WHISPER_API` declaration is exported):
  - Per-token probability: `whisper_full_get_token_p` and `whisper_full_get_token_p_from_state`. `WhisperState::token_probability` reads `p` from `whisper_full_get_token_data_from_state` instead.
  - The context-taking forms `whisper_full_get_token_data` and `whisper_full_get_segment_no_speech_prob`; the safe API resolves only the `_from_state` forms.
