---
name: Gateway phases 4-5
overview: "Embeddings, classifiers, streaming, and minimal normalization. A rule-7 fix, then steps 14-26. Each step is one commit: code + test + docs."
todos:
  - id: fix-validate-502
    content: "Rule-7 fix: response validation failure maps to UpstreamProtocol, not a fabricated UpstreamStatus{502}"
    status: completed
  - id: step-14
    content: "Step 14: kind-routing guards"
    status: completed
  - id: step-15
    content: "Step 15: POST /v1/embeddings route [Verify]"
    status: completed
  - id: step-16
    content: "Step 16: local embedding model launch (--embeddings) [Verify]"
    status: completed
  - id: step-17
    content: "Step 17: rerank wire types + POST /v1/rerank route"
    status: completed
  - id: step-18
    content: "Step 18: local classifier model launch (--reranking) [Verify]"
    status: completed
  - id: step-19
    content: "Step 19: ChatChunk wire types + Upstream::stream method"
    status: completed
  - id: step-20
    content: "Step 20: stream:true SSE relay with permit lifetime"
    status: completed
  - id: step-21
    content: "Step 21: client-disconnect cancellation + chunk validation [Verify]"
    status: completed
  - id: step-22
    content: "Step 22: Capabilities on config structs and ModelInfo [Verify]"
    status: completed
  - id: step-23
    content: "Step 23: move Gemma3ToolCode dialect from core to gateway"
    status: completed
  - id: step-24
    content: "Step 24: delete dialect machinery from promptforge-core [Verify]"
    status: completed
  - id: step-25
    content: "Step 25: split UpstreamTransport into connect vs mid-flight"
    status: completed
  - id: step-26
    content: "Step 26: final sweep [Verify]"
    status: completed
isProject: false
---

# Gateway Program - Phases 4-5

<execution-context>

Parent plan: `~/.cursor/plans/dominion_refactor_ae33684d.plan.md` (steps 1-13 landed, HEAD 009f180).

Execution: `tools-public/rulebooks/vibe-rulebook.md` + `tools-public/rulebooks/rust-rulebook.md`.

Dispatch pattern: the subagent receives the path to the plan file, and instructions to GREP for the specific xml tag, and read ONLY what is inbetween the tags, do NOT read the entire plan file. Each step's content lives in a `<step-N>` tag; the entropy rules live in the `<entropy-rules>` tag. Every dispatch references the execution-context tag; a coder dispatch also references its step tag and the entropy-rules tag; a review dispatch references its step tag and the entropy-rules tag (check 7 is exactly what the rules target).

Guide sync: edit `guide/src/gateway.md` and `crates/promptforge-gateway/user-guide-promptforge-gateway.md` manually, then regenerate `guide/promptforge-user-guide.md` via `cargo run -p make-user-guide`.

</execution-context>

## Phase 4: embeddings, classifiers, streaming

E1 (model kinds) landed as step 13. The `Upstream` trait has only `send` and `shutdown`; no embedding wire types or methods exist yet.

<fix-validate-502>

**Rule-7 fix (lands first, own commit):** the chat handler maps a `response.validate()` failure to `UpstreamStatus { status: 502 }` (lib.rs), fabricating an upstream status that never happened. Map it to `GatewayError::UpstreamProtocol` instead - the existing variant for "success status but wrong shape" (UP-004). Same 502 to the client; the code and message stop lying. Test: a 200 upstream response that fails shape validation produces code `upstream_protocol`, not `upstream_error`.

</fix-validate-502>

<step-14>

**Step 14: kind-routing guards.** Add `GatewayError::KindMismatch` (400) in `error.rs`. Add `fn require_kind(model, expected) -> Result<_, GatewayError>` in `routing.rs`. The chat handler rejects non-chat kinds. Test: each non-chat kind on the chat path returns 400. Docs: kind-mismatch in the error table.

</step-14>

<step-15>

**Step 15: embeddings wire types + POST /v1/embeddings route.** Add `EmbeddingRequest` (OpenAI shape: `input` string-or-array, `model`, `encoding_format`) and `EmbeddingResponse` (`data[{embedding, index}]` + `usage`) to `wire.rs`. Add `fn send_embeddings(&self, req) -> Result<EmbeddingsResponse>` to the `Upstream` trait with a default returning `ModelUnavailable`; implement it on `OpenAiUpstream` (POST to `{base_url}/embeddings`). Register `/v1/embeddings` in `lib.rs`. Handler: parse, `Routing::model` + `require_kind(Embedding)`, dominion queue admit, `send_embeddings`. Test: wire serde round-trips; remote passthrough through mock upstream with queue admission; `ModelUnavailable` for local; kind-mismatch 400. Docs: `/v1/embeddings` section. Verify.

</step-15>

<step-16>

**Step 16: local embedding launch.** `local/mod.rs`: `kind = Embedding` launches `llama-server --embeddings`. Artifact download/digest unchanged. Dominion binding unchanged. Test: launch args include `--embeddings`; route-through-child follows the existing local test harness pattern (live llama-server tests stay `#[ignore]`d). Docs: local embedding section. Verify (end of E2).

</step-16>

<step-17>

**Step 17: rerank wire types + POST /v1/rerank.** Add `RerankRequest`/`RerankResponse` to `wire.rs` (llama-server/vLLM/Jina shape: query + documents in, ranked scores out). Register `/v1/rerank` in `lib.rs`. Handler: parse, `require_kind(Classifier)`, queue admit, remote passthrough. Local classifiers return `ModelUnavailable`. Test: serde round-trips; remote passthrough; kind-mismatch 400; `ModelUnavailable` for local. Docs: `/v1/rerank` section.

</step-17>

<step-18>

**Step 18: local classifier launch.** `local/mod.rs`: `kind = Classifier` launches `llama-server --reranking`. Test: launch args include `--reranking`; route-through-child follows the existing local test harness pattern (live llama-server tests stay `#[ignore]`d). Docs: local classifier section. Verify (end of E3).

</step-18>

<step-19>

**Step 19: ChatChunk + Upstream::stream.** `Upstream` is a trait (`#[async_trait] trait Upstream: Send + Sync`) used as `Arc<dyn Upstream>`, so `impl Stream` return breaks object safety. Add `ChatChunk`/`ChatChunkChoice` to `wire.rs` (OpenAI streaming shape: `delta` instead of `message`). Add `fn stream(&self, req) -> Result<BoxStream<'static, Result<ChatChunk>>>` to the trait with a default returning `Err(ModelUnavailable)`. Use `async_stream` or `futures::stream` for the boxed stream. Test: `ChatChunk` serde round-trips; the method compiles and is object-safe. Docs: wire type rustdoc.

</step-19>

<step-20>

**Step 20: stream:true SSE relay.** Chat handler checks `stream: true` on `ChatRequest` (add the field if missing). When set: call `Upstream::stream`, hold the dominion queue permit for the stream's lifetime, re-emit each chunk as an SSE `data:` line (typed relay - the gateway validates and re-serializes per chunk, it does not splice upstream bytes), finish with `data: [DONE]`. Fail before the SSE response starts: an upstream non-2xx is consumed as a normal JSON error, never an SSE stream that dies mid-flight (litellm-rust pattern, `ai-gateway/src/routes/messages/mod.rs`). Forward upstream `Content-Type`/`Cache-Control`, defaulting to `text/event-stream`. Never apply reqwest's total `.timeout()` to the stream path - it covers the entire body read and would kill any long-lived stream. The shared `http_util::bounded_client()` has a 120s whole-request timeout baked in, so the stream path builds its own client with only the connect timeout; do not reuse `bounded_client()`. Non-streaming path unchanged. Check reqwest's `stream` feature in Cargo.toml; add it if missing. Test: mock upstream emits 3 chunks; client receives 3 SSE lines; permit held until stream ends; upstream 500 before stream start returns a JSON error. Docs: streaming section.

</step-20>

<step-21>

**Step 21: disconnect cancellation + chunk validation.** Client disconnect cancels the upstream stream: Drop is the entire mechanism (client disconnect drops the response body, which drops the upstream `reqwest::Response`, which aborts the connection - litellm-rust relies on exactly this and nothing more). Per-chunk validation: each chunk needs at least one choice with index + delta; malformed chunks logged and skipped. The terminal `data: [DONE]` sentinel is not JSON - validation special-cases it, or every healthy stream ends with a spurious malformed-chunk log. Test: client disconnect cancels upstream; malformed chunk skipped without breaking the stream; `[DONE]` produces no warning. Docs: streaming caveats. Verify (end of S1, end of phase 4).

</step-21>

## Phase 5: minimal normalization

Core has two dialects: `OpenAi` (identity) and `Gemma3ToolCode` (content-fence parsing). No translator framework. No effort mapping. No DialectConfig. Those arrive when a model that needs them is configured.

<step-22>

**Step 22: Capabilities on config + wire.** Add `Capabilities { max_output, default_temperature, images, parallel_tool_calls, effort_levels, default_effort, adaptive_thinking }` with `#[serde(flatten)]` on `ModelConfig` and `LocalModelConfig`. Validation: `effort_levels` non-empty when `default_effort` set; `default_effort` must name a listed level; effort fields rejected when `thinking = "never"`; `max_output <= context` when both set; the new chat-only fields (`effort_levels`, `default_effort`, `adaptive_thinking`) extend step 13's kind-scoped rejection - embedding/classifier kinds reject them. `ModelInfo` gains the same fields. Test: each validation rule; catalog response includes the fields. Docs: config reference + catalog response. Verify (end of N1).

</step-22>

<step-23>

**Step 23: move Gemma3ToolCode to gateway.** `tool_dialect` is currently hardcoded `"openai"` in `Routing::from_config`; this step makes it a real config field on `ModelConfig` (default `"openai"`, only other value `"gemma3_tool_code"`). When `tool_dialect = "gemma3_tool_code"`: the chat handler injects the Gemma3 tool-code system guide (ported from `promptforge-core/src/dialects/gemma3_tool_code/guide.rs`) into the system message, strips tool definitions from the outgoing request, and parses the response content for `tool_code` fences (codec from `codec.rs`/`content.rs`) into `tool_calls` objects. Malformed fences become empty content + a `gateway_warning` field that is always present on recovery and logged at warn - never silent. The `gateway_warning` field is a gateway-specific extension on the OpenAI shape; downstream serde ignores unknown fields, and the wire docs note it. (litellm-rust's discipline is decline-rather-than-drop; the gateway is terminal with no fallback, so warn-and-continue is the adaptation, but the warning must be surfaced). Post-receipt parse failures stay distinct from pre-call translation errors (litellm-rust `as_response_error` pattern, `crates/core/src/chat_completions/handler.rs`: errors raised while normalizing an already-received response collapse into one invalid-response class, never confused with never-reached-the-provider failures). No `response_format: json_object` - the Gemma dialect uses content-fence parsing, not JSON mode. Move the codec into the gateway as a plain module (no trait - a `match` on the dialect string). Test: request with tools gets the guide injected; response with tool_code fence parsed into `tool_calls`; malformed fence produces warning; config rejects an unknown `tool_dialect` value. Docs: emulated tools section.

</step-23>

<step-24>

**Step 24: delete dialect machinery from core.** Delete `dialects.rs`, `dialects/`, `ToolDialectId`, and `ToolsMode`. `normalize.rs` SURVIVES - it holds core's response canonicalization (`NormalizedTurn`, the empty-reply invariant, `parse_openai_tool_calls`, `extract_reasoning`), which core still needs for the gateway's OpenAI-shaped responses; only the dialect delegation points go, and the single-implementor `CompletionNormalizer` trait collapses into plain functions (no trait without a second implementation). Remove `tool_dialect`/`tools_mode` from `ModelInfo` (wire) and from core's `ModelDescriptor`/`ModelCatalog` consumption. The gateway config's `tool_dialect` field (added in step 23) STAYS - it is the gateway's dialect selector; only the wire and core stop exposing it. The blast radius includes `execute/tool_loop.rs`, `lua_models/`, `client/transport.rs`, and `error.rs` - all reference the dialect machinery and must be updated. Test: core compiles with no dialect code; integration suite passes. Docs: updated core crate docs. Verify (every 3rd step + end of core removal).

</step-24>

<step-25>

**Step 25: split connect from mid-flight transport errors.** `GatewayError::UpstreamTransport` lumps connect-refused (the request never left; nothing was billed; safe to retry) with mid-flight read/timeout failures (the provider may have received it). Split per the litellm-rust Connect/Network precedent (`litellm-rust/crates/core/src/error.rs`): classify with `err.is_connect()` at the `upstream_transport` wrap point into a new `UpstreamConnect` variant (502, code `upstream_connect`) vs the existing `UpstreamTransport`. A timeout is NEVER connect - it may have reached the provider. No retry policy changes; this is honest diagnostics plus the foundation for the parked fallback work. Local-child recovery behavior is unchanged: the recovery match arm must cover both variants. Test: connect-refused produces `upstream_connect`; a stalled-server timeout stays `upstream_transport`; local recovery triggers on both. Docs: error table gains the new code.

</step-25>

<step-26>

**Step 26: final sweep.** Obsolescence sweep over the whole workspace. Remove "streaming" and "Anthropic protocol shim" from the crate-docs deferred list. Fix the recurring guide-generator drift at the source: `make-user-guide` writes to the repo root while the checked-in aggregate lives at `guide/promptforge-user-guide.md`, and other crates' per-crate guide sources have drifted from the aggregate (two hunks kept reappearing on regen: the run fall-through wording and the `tools.add_local` alias paragraph) - regenerate the aggregate into the right path and commit the reconciliation as its own sweep commit. Verify (final step). Manual checks: a Gemma3 model returns `tool_calls` to the client; a chat model with `stream: true` receives SSE chunks.

</step-26>

## Verify schedule

- Step 15: every 3rd + end of E2 remote
- Step 16: end of E2
- Step 18: every 3rd + end of E3
- Step 21: every 3rd + end of S1 + end of phase 4
- Step 22: end of N1
- Step 24: every 3rd + end of core removal
- Step 26: final step

## Settled decisions

- E3 route: `POST /v1/rerank`. Falsifier: an OpenAI-standard classifier endpoint emerges.
- One `kind` field (landed step 13).
- No translator trait until a second backend needs it (rust-rulebook section 7).
- Emulated tools via JSON mode, not XML prompt injection.
- `tool_dialect`/`tools_mode` wire removal in step 24, same commit as core cleanup.
- Transport errors split connect vs mid-flight in step 25 (litellm-rust Connect/Network precedent); a timeout is never connect.

## NOT in this plan

Demand-driven residency (rejected). DRR/VTC fairness (parked). Priority weights (parked). Fallback chains (parked). Multi-instance admission (not needed). Anthropic inbound shim (rejected). Anthropic upstream translator (deferred to follow-on plan). Translator framework, effort mapping, DialectConfig TOML, streaming tool-call assembly, conformance matrix (all deferred until a model that needs them is configured).

## Entropy rules

<entropy-rules>
Every edit reverses entropy. Apply these wherever they fire during your step; each is mechanically checkable (grep, compiler, or test suite confirms the deletion is safe). The exception clauses are part of the rule - do not apply a rule whose exception matches your situation. The diff nets negative or barely positive on lines unless the step adds genuinely new behavior.

1. Delete a function with zero callers. Trait impls and tests count as callers; grep the whole workspace.
2. Delete a struct field nothing reads. Constructors, accessors, matches, and serde deserialization all count as reads.
3. Delete a config key whose parsed value is never read. Parsing alone does not make it live.
4. Inline a function called from exactly one site. If its name carried meaning, give the name to a local binding at the call site.
5. Delete a comment by renaming what it describes. A comment stating an invariant the type system cannot express stays - a name carries the what, never the why.
6. Replace a bool parameter with a two-variant enum. Skip when the signature is fixed by a trait or external API.
7. Replace a runtime check with a type (NonZeroU32, a newtype, an enum). Only when every construction site can produce the proof; untrusted input parses into the type at the boundary.
8. Merge two functions only when every caller uses them in sequence. One independent caller means they stay separate.
9. Delete a wrapper that forwards unchanged. Check trait impls and pub reach first - a wrapper satisfying a trait or public boundary stays.
10. Replace a stringly-typed field with an enum when the value set is closed. If the string passes through to an external system that accepts anything, keep the string or newtype it.
11. Delete a comment that lies. Verify against the code first; a comment documenting deliberate deviation is not lying.
12. Delete a doc comment that restates the signature. If missing_docs denies the deletion on a public item, rewrite it to carry the why instead.
13. Collapse `if c { true } else { false }` into `c`. Always safe; evaluation order is unchanged.
14. Replace a single-arm match with let-else. A catch-all arm that logs or recovers is not single-arm.
15. Delete a dependency used at one call site when std covers it. Check Cargo.toml features - a feature may pull the dep even after the call is gone.
16. Delete a test that duplicates another's arrange and assert. A different edge input is not a duplicate even on the same code path.
17. Replace Option<Vec<T>> with Vec<T> when None and Some(empty) mean the same thing. If None means "absent" and Some(empty) means "provided empty" (a serde distinction), keep the Option.
18. Delete an #[allow] or #[expect] that no longer fires. Remove and build; if the lint refires, restore with a reason.
19. Narrow pub to pub(crate) when nothing outside the crate uses it. Check the whole workspace, including re-exports.
20. Delete a test helper used by one test. Move the body inline; if the helper's name carried the test's intent, rename the test to carry it.
21. Merge two structs almost always used together only when the exception case can hold a fully valid value (secondary constructor or cheap default). If the exception would hold a placeholder, or the halves have conflicting borrows or lifecycles, keep them apart and destructure at use sites.
</entropy-rules>


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: gateway_phases_4-5_5ef756eb

## Origin

The plan was created inside the dominion_refactor execution chat. While a coder subagent was mid-work on step 14 of the parent plan (4 gateway files modified, uncommitted), the user stopped it and redirected: "discard partial work, decompose the remaining work into finer grained steps into a new plan file" and "decompose phases 4-5 into a new plan file, apply [the vibe and rust rulebooks] to the plan file, then stop for review." The partial work was discarded; HEAD stayed at 009f180 (step 13).

## The decisive scope cut: minimal normalization

The first draft had 19 steps (14-32); phase 5 alone was 11 steps building a general translator framework: effort mapping, LiteLLM-style param policy, JSON-mode emulation, streaming tool-call assembly, conformance matrix. The user pushed back:

- "I am a little shocked we need 16 steps for this... how did we get so many steps?"
- "I was thinking we just add them as needed, we don't try to cover every possible model at once."

Checking the code settled it: core has exactly two dialects, OpenAi (identity passthrough) and Gemma3ToolCode (content-fence parsing). (Paraphrase) An 11-step framework to replace two dialects, one of which is identity, was disproportionate; phase 5 was cut to 4 steps (Capabilities, move Gemma3ToolCode to the gateway, delete core dialect machinery, sweep). The framework pieces became the plan's "NOT in this plan" deferred list, each gated on a model that needs it being configured. This is the why behind the plan's "minimal normalization" framing and its settled decision "No translator trait until a second backend needs it."

## Semantic blur and the entropy rules

The user demanded a rewrite: "I want it going in clean, no extra unnecessary shit, I do not want blurbloat," then clarified the lesson applies to code, not just the plan document: "the point of semantic blur is that when you edit the code I dont want you to bloat it. I want you to make the edits in a way that reverses entropy." He asked for "up to 20 unambiguous ways that entropy can be reversed," probed the hardest corner case himself ("what about combining two structs that are *almost* always used together" - the answer baked into rule 21: merge only when the exception case can hold a fully valid value; placeholder construction is entropy in a new costume), then directed: "bake the rules into the plan as an xml-delimited section that the subagent will refer to," with the grep-only dispatch instruction. This is the origin of the plan's entropy-rules block and its grep-dispatch pattern.

## litellm-rust comparison

At the user's request ("use subagents and compare this to what it is in litellm-rust"), two subagents checked the plan against the reference codebase. Validated: the no-translator-trait decision, Drop-based cancellation, holding the queue permit for the stream's lifetime, and the reqwest stream feature requirement. Folded into steps 20-21 and 23: fail before the SSE response starts, forward upstream Content-Type/Cache-Control, never a total reqwest timeout on the stream path, the [DONE] sentinel special-cased in chunk validation, the loud gateway_warning on malformed fences (warn-and-continue as the terminal-gateway adaptation of litellm's decline-rather-than-drop discipline), and post-receipt parse errors kept distinct from pre-call failures (the as_response_error pattern).

The user then asked the question that produced the plan's first and next-to-last items: "do any of the litellm improvements suggest improvements we can make to the existing model completion implementation?" Two findings in existing code, both ruled on by the user:

1. The chat handler mapped a response.validate() failure to a fabricated UpstreamStatus{502} - an upstream status that never happened. (Paraphrase) Same 502 to the client, but the code and message stop lying by mapping to the existing UpstreamProtocol variant. User decision: land it first as a rule-7 fix in its own commit (the plan's fix-validate-502 block).
2. UpstreamTransport lumped connect-refused (never reached, nothing billed, safe to retry) with mid-flight failures (maybe reached, maybe billed). User decision: add the Connect/Network split as a plan step (step 25), a timeout is never connect, no retry policy changes - honest diagnostics plus the foundation for the parked fallback work.

## Late correction: normalize.rs survives

The final review caught that step 24 as drafted implied deleting normalize.rs as dialect machinery. It is not: it holds core's response canonicalization (NormalizedTurn, the empty-reply invariant, parse_openai_tool_calls, extract_reasoning), which core needs permanently for the gateway's OpenAI-shaped responses. Only the dialect delegation points go, and the single-implementor CompletionNormalizer trait collapses into plain functions. The reviewer called this "the plan's biggest remaining error."

## Run chat

The designated run chat (59952dc1) contains no execution of this plan and no deviations - it is an unrelated transcript-processing session, so it contributes nothing here.
