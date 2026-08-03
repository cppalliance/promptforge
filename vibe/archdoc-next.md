# PromptForge architecture queue

- [2026-07-29-1-gateway-v0] caller-model alias: Restore the caller's model name after upstream routing; this may remain a wire-level detail.
- [2026-07-29-2-lua-args-substitution] runtime metadata snapshots: Clarify which system values are fixed per run, fixed per step, or refreshed live.
- [2026-07-29-4-webfetch-crate-extraction] fetch provenance contract: Return final URL, truncation, and extraction mode with fetched text so consumers know what they received.
- [2026-07-29-5-multi-turn-research-prompt] prompt-owned tool budget: Let a prompt declare its tool-iteration budget while retaining a finite default.
- [2026-07-29-5-multi-turn-research-prompt] soft output target: A prose token target is guidance, not an enforced output limit.
- [2026-07-29-6-per-section-tool-scoping] add-only scope API: The current Lua scoping surface accumulates and deduplicates names but has no remove or clear operation.
- [2026-07-29-7-guard-wrap-untrusted-output] nonce delimiter mechanics: Random XML-style tags and escaping are model-specific mitigation details, not an isolation guarantee.
- [2026-07-31-1-orchestrator-only] three-axis budgets: Track nesting, section transitions, and tool calls independently rather than using one global limit.
- [2026-08-02-1-tool-picker] picker threshold calibration: Similarity floors and duplicate thresholds are workload evidence, not general architecture.
