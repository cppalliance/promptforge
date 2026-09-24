The model-call metrics reply events hold.

# Where metrics appear

A completed model round is reported as an [`AssistantReply`](crate::event::Event::AssistantReply) event whose `metrics` field is a [`CallMetrics`], present when anything measured the call; a round that requested tools is reported as an [`AssistantToolCalls`](crate::event::Event::AssistantToolCalls) event holding one [`ToolCallEvent`] per call, with the model-authored name and raw arguments. Metrics are a report: the engine never reads them back to decide anything.

# Sources

Each section of a [`CallMetrics`] is present when its source reported it:

- [`Usage`]: token accounting as the serving backend reported it, including cached and reasoning tokens when the backend counts them.
- [`LlamaTimings`]: the `timings` a llama.cpp server reports for prompt processing, generation, and speculative drafts.
- [`VllmMetrics`]: vLLM's per-request metrics, each optional because vLLM omits what it did not measure.
- [`ClientTiming`]: time to first token, mean inter-token latency, and end-to-end time, measured on the calling client's own clock. The [`transport`](crate::transport) codec measures it against the clock the host's transport supplies.
