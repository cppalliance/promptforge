---
produced: 2026-08-23
title: Zed and LiteLLM codebase exploration - model metadata schemas, provider translation layers, gateway overlap
---

# Zed and LiteLLM codebase exploration

Source-tree reconnaissance for the gateway redesign (local checkouts: `zed/`, `litellm/`).

## Zed (editor, potential gateway client)

Zed models a language model in three layers:

1. Runtime capability trait `LanguageModel` (`crates/language_model/src/language_model.rs:65`): everything is a method - `max_token_count()`, `max_output_tokens()`, `supports_tools()`, `supports_thinking()`, `supports_disabling_thinking()`, `supported_effort_levels()`, `supports_images()`, `tool_input_format()`.
2. Hardcoded per-provider `Model` enums with a `Custom { ... }` escape hatch. OpenAI's crate does not fetch `/v1/models`; the model list is compile-time knowledge.
3. Settings overlays (`*AvailableModel` structs in `crates/settings_content/src/language_model.rs`) for user-declared custom models with capability flags.

Key findings:

- **Zed never fetches the catalog for `openai_compatible` providers** - the one a user would point at our gateway. Models come from static settings (`available_models`). A rich `/v1/models` catalog serves OUR consumers (promptforge-core fetches it), not Zed.
- **Zed has no description field** on model descriptors. OpenRouter's API returns one and Zed discards it. Our catalog's `description` (used by semantic bind) is a genuine differentiator.
- Thinking: `supports_thinking` + `supports_disabling_thinking` + `ModelMode { Default | Thinking { budget_tokens } | Adaptive }` + effort ladders (`ReasoningEffort { None | Minimal | Low | Medium | High | XHigh | Max }`, per-model `supported_effort_levels` with a default flag). Our tri-state (never/always/switchable) maps cleanly onto the first two.
- Zed is richer on: effort ladders, adaptive mode, thinking budgets, `max_output_tokens`/`max_completion_tokens` split, `default_temperature`, prompt caching, protocol selection (Chat Completions vs Responses vs Anthropic Messages).
- Zed is thinner on: caller-facing description, `tools_mode` (native/emulated - not modeled), multi-endpoint binding.
- Credentials live in the editor process (env var or system keychain, keyed by provider URL). Zed has no interest in being a credential proxy - confirms the gateway's role.
- Zed never spawns local inference processes; it connects to already-running Ollama/llama.cpp servers and discovers models over HTTP.
- Concurrency is a fixed `Semaphore(4)` per model (`crates/language_model_core/src/rate_limiter.rs`) - a soft client-side cap, no fairness.
- Settings profiles can hot-swap `api_url` and provider config editor-wide; no inference-only profile concept.

Fields adopted for the gateway catalog (normalization phase N1): `max_output`, `default_temperature`, `images`, `parallel_tool_calls`, `effort_levels` + `default_effort`, `adaptive_thinking`. Not taken: fast mode, server-side compaction, cache anchors, billing/policy fields.

## LiteLLM (Python proxy + litellm-rust)

### Translation layer (reference for normalization N2-N4)

- Per-provider adapters at `litellm/llms/<provider>/chat/transformation.py`, all implementing `BaseConfig` (`litellm/llms/base_llm/chat/transformation.py`): `get_supported_openai_params`, `map_openai_params`, `validate_environment`, `transform_request`, `transform_response`, `get_error_class`. Transport stays out of the translator. This five-method shape is the model for our per-dialect translator trait.
- **Shared effort-to-budget table** in `litellm/constants.py`: canonical `reasoning_effort` rung to token-budget map with per-provider emitters (Anthropic `budget_tokens`, Gemini `thinkingBudget`/`thinkingLevel`, Qwen `enable_thinking`, vLLM bucketing). The de-facto interop table; adopt the table, not the 78-file scatter.
- **Strict param policy**: unsupported params error by default (`UnsupportedParamsError`); dropping is explicit opt-in (`drop_params`). No silent mutation of client intent.
- Emulated tools: the living precedent is Ollama (`format=json` + JSON parsed from message content into `tool_calls`). The XML prompt-injection path (`construct_tool_use_system_prompt`) is dead code with no callers - prompt-convention tool parsing is NOT the approach.
- `response_format` emulation via a forced tool (`_add_response_format_to_tools`) for backends without native JSON schema.
- Avoid: the giant `get_optional_params` if/elif ladder plus a second registry that drifts from it; three parallel Anthropic stacks; soft auto-mutation of requests.
- Anthropic translation exists in both directions; we need only OpenAI-ingress to Anthropic-upstream.

### litellm-rust (their Rust ai-gateway subproject)

- A staged rewrite of hot paths in front of the Python control plane, not a standalone gateway: YAML config loaded via embedded Python, `simple-shuffle` router only, no concurrency queues, no graceful shutdown, no hot reload.
- Worth noting: auth as an axum extractor with constant-time compare (`subtle`) and fail-closed defaults - independent convergence with our `check_auth`/`secret_eq`.
- Error taxonomy splits `Connect` (never reached the provider - safe to retry/fallback) from `Http` (upstream already called - already billed). This is the foundation to use if fallback routing ever unparks.

## Implications recorded in the plans

- The gateway catalog is a superset play, not a Zed clone.
- `max_output` and `default_temperature` are cheap catalog additions for Zed-grade clients.
- The translator trait is the only registry; one impl per backend dialect.
- Anthropic is an upstream dialect target only; clients are always OpenAI-shaped.
