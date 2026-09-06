# gateway-stt-backend-whisper

This crate owns safe Whisper backend construction and decode policy: model loading, prompt fitting, parameters, progress, and translation into engine errors.

- Unsafe code, ABI layouts, raw pointers, and C symbols stay in `gateway-whisper-ffi`.
- Host configuration types and HTTP, WebSocket, UI, session, and take state stay outside this crate.
