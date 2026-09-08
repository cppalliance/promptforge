# gateway-logging

This crate owns the gateway's log pipeline: the bounded priority queue, the `gateway.log` rotation and file sink, the redaction pass, and the single worker thread that drains formatted records to disk.

- The caller supplies the state directory. This crate does not discover process configuration or shared state.
- Global subscriber installation stays in the Gateway binary. This crate supplies the file layer without installing process-global tracing state.
- Every record crosses `redact::redact_line` at the enqueue chokepoint before it reaches the queue. Logs never carry credentials, environment values, request bodies, audio, transcript text, prompts, or full local model paths.
- Queue pressure may displace lower-priority records. Warning and error records are never evicted.
- `LogConfig::log_path` and `LogConfig::retained_log_paths` own the disk layout used by rotation and diagnostics.
- File-sink failure falls back to synchronous stderr. Shutdown closes admission, drains, flushes, and joins, so the Gateway shuts the logger down last.
