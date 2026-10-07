# gateway-whisper-ffi

- Unsafe is confined to this crate's FFI boundary.
- Raw whisper pointers live only behind Drop-owning wrappers (`ContextInner`, `WhisperState`); never expose a raw `*mut` across the safe API, and never free or clone a pointer outside those Drop impls.
- The packaged C ABI pin covers struct sizes, the symbol set, and parameter layout. A retarget must update the pin and its size and offset assertions in the same change. Writing a `whisper_full_params` member that has no offset assertion yet means adding one first.
- Exported by the pinned b4938 library but not yet resolved, checked against `include/whisper.h` and `src/whisper.cpp` at the tag (the shared build compiles with `WHISPER_SHARED WHISPER_BUILD`, so every `WHISPER_API` declaration is exported):
  - Per-token probability: `whisper_full_get_token_p` and `whisper_full_get_token_p_from_state`. `WhisperState::token_probability` reads `p` from `whisper_full_get_token_data_from_state` instead.
  - The context-taking forms `whisper_full_get_token_data` and `whisper_full_get_segment_no_speech_prob`; the safe API resolves only the `_from_state` forms.
