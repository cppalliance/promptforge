# gateway-stt-engine

This crate owns backend-neutral stateless transcription workers and shared audio policy.

- Decode jobs are stateless: workers retain no guidance, history, transcript, session, or take state between jobs.
- Blocking decoders stay on their owning threads; callers hand over owned buffers and await replies without blocking the async executor.
- Startup deadlines classify non-preemptible construction without claiming cancellation; ordinary shutdown joins every worker and surfaces join panic.
- Backend, host, transport, and UI state stay outside this crate.
