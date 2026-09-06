# gateway-logging

This crate owns the gateway's log pipeline: the bounded priority queue, the `gateway.log` rotation and file sink, and the single worker thread that drains formatted records to disk.

- `gateway` is the only workspace consumer. The crate depends only on the standard library, `tracing`, and `tracing-subscriber`; it never reads the home directory, the environment, Gateway configuration, sidecar state, or STT types - the caller passes the state directory in through `LogConfig`.
- The public surface is exactly `LogConfig`, `LogRuntime`, `LogWriter`, and the opaque `LogError`. Queue lanes, records, rotation, sinks, mutexes, condition variables, and worker handles stay private.
- Global subscriber installation stays in the binary: this crate supplies the `MakeWriter` file layer and never calls `init` or `set_global_default`.
- Queue policy is fixed: 8192 records total, drain batches of 256, one deque per priority under one mutex. On a full queue, evict the oldest Debug, then Trace, then Info; Warn and Error are never evicted, and a producer with no eligible record blocks on the condition variable. Formatting and allocation happen before locking; the worker writes outside the mutex.
- File-sink failure falls back to synchronous stderr. `LogRuntime::shutdown` closes admission, drains, flushes, and joins - the gateway shuts the logger down last.
