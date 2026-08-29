# Agentic User Interfaces: Patterns for PromptForge Workshop

Synthesis of five parallel research efforts covering Claude Code / Claude Cowork, Cursor, OpenAI (ChatGPT agent, Operator, Codex), coding-agent UIs (GitHub Copilot coding agent, Devin, Windsurf Cascade, Aider), and agent-UI frameworks (AG-UI, CopilotKit, Vercel AI SDK, LangGraph/LangSmith, OpenAI Agents SDK, MCP Apps). Full evidence with source URLs is in the companion research file dated 2026-08-29.

## The convergent architecture

Every mature agentic UI converged on the same two-level information architecture:

1. **A persistent run list** - cards with five fields: title, status (running / waiting / done / failed), current-step one-liner, elapsed time or tokens, and a click target. "Waiting for input" is a first-class state everywhere.
2. **A run detail view** - header (status, elapsed, stop/pause/resume), a grouped chronological step timeline with collapsed subagents, a diff/artifact panel, raw escape-hatch tabs, and mid-run steering input.

The main chat stays a clean narrative of collapsible summary rows; every row deep-links to full-fidelity detail.

## The event stream is the product

The highest-leverage architectural choice, made independently by Cursor, AG-UI, and the OpenAI Agents SDK: **the UI is a pure function of one append-only, typed event log**. Live streaming and history replay share one renderer and one schema.

The AG-UI protocol is the converging cross-framework standard (adopted by Google, LangChain, Microsoft, AWS Bedrock AgentCore, Mastra, PydanticAI). Its vocabulary maps directly onto what Workshop needs:

- Lifecycle: `RUN_STARTED` / `RUN_FINISHED` / `RUN_ERROR`, `STEP_STARTED` / `STEP_FINISHED` - every run brackets strictly.
- Text: `TEXT_MESSAGE_START` / `CONTENT` (delta) / `END`; reasoning gets its own family including an encrypted/opaque variant.
- Tool calls: `TOOL_CALL_START`, `TOOL_CALL_ARGS` (streams partial JSON so forms pre-fill), `TOOL_CALL_END`, `TOOL_CALL_RESULT`, joined by `toolCallId`.
- State: `STATE_SNAPSHOT` on connect, then RFC 6902 JSON Patch deltas - exactly the reconnect story a WebSocket server needs.
- Subagents: an optional `subagentRunId` on every event, so concurrent subagent streams are attributable rather than one undifferentiated stream.

Serde-tagged Rust enums map to this cleanly, and Workshop's existing protocol module already enforces the "every pushed frame is classified" discipline this requires.

## Human-in-the-loop is interrupt/resume, not inline blocking

LangGraph and the OpenAI Agents SDK converged on the same model, and it matches the deferred-completion design we discussed for `user_input()`:

- The run pauses, persists state, and emits a typed interrupt payload.
- The client resolves it with a resume command carrying the user's value.
- Interrupted runs consume no compute and can resume much later.

OpenAI's ChatGPT agent adds two refinements worth copying: interruption is a checkpoint, not a cancellation (redirect mid-task without losing progress, partial results on stop), and takeover mode visibly suspends observation/logging while the user types credentials.

## Approvals are a policy ladder, not a prompt stream

Every tool shipped tiered gating: allowlist -> sandbox -> classifier -> human, with "don't ask again" persistence on every prompt. Two documented failure modes matter more than the mechanism:

- Claude Code's "don't ask again" rules frequently fail to match, causing re-prompt storms that users file bugs about.
- Codex orchestrators built "approver daemons" to auto-confirm prompts - proof that per-action modals do not scale.

For Workshop's first version, the prompt itself is the policy: `user_input()` is the only gate, and the prompt author decides where it sits. Declarative allow/deny lists can come later.

## Subagent visibility is the number one gap in shipping products

Claude Code's most complained-about weakness is subagent invisibility: a collapsed `Done (10 tool uses · 45.6k tokens)` line with no live status, no reachable dispatch prompt, and users with parallel agents "standing in the parking lot going 'they're still in there I think.'" Cursor's documented bugs are the mirror image: subagent cards flicker between status texts and running subagents vanish from the chat pane.

The proven fix (Claude Cowork, GitHub's session logs, Devin): a compact card in the main stream - name, live status, elapsed time, current tool call - that expands or opens a full trace including the dispatch prompt and final result. GitHub groups similar tool calls to cut noise and collapses subagent activity behind a heads-up line showing what it is doing right now.

This validates the structured-activity design for Workshop: `task_started` / `task_delta` / `task_finished` events keyed by a stable task id, a durable summary card in chat, and a separate high-volume detail stream. Stable IDs and debounced status transitions are not polish; their absence is a shipped bug in two major products.

## Streaming and thinking

- Stream data, not components. Vercel's RSC approach (streaming React components) is now marked experimental - quadratic transfer, no parallel tool calls. The production pattern is streamed props rendered client-side.
- Tool-call rounds buffer until the call is complete; only final text and reasoning stream.
- Thinking is a presentation-layer concern. AG-UI even has an encrypted reasoning variant. This supports keeping raw thinking out of Lua: stream it to the UI sink, never into prompt control flow.
- Claude Code's spinner is a state machine (running / stalled / error via color on one shared animation clock). A stall indicator is information, not decoration.

## Background work and notifications

Two notification classes suffice: "done" and "blocked on you", with presence-aware suppression. Claude Code separates a task checklist (the plan of record) from a process monitor (`/tasks`: what is actually running) - two surfaces Workshop will eventually want as distinct panels. Timeouts auto-background rather than kill.

## What this means for the Workshop chat replacement

Mapped onto the design we discussed:

1. **One typed event log over the existing WebSocket**, AG-UI-shaped, rendered identically live and on replay. This subsumes the `ReplySink` idea: text deltas, reasoning deltas, tool calls, and task lifecycle are all just event families.
2. **`user_input()` as interrupt/resume**: the run suspends, emits a typed interrupt event with a token, the SPA resolves it. WebSocket frame or HTTP POST both just call `complete(token, payload)`.
3. **Explicit chat visibility**: nothing appears in chat by default; `chat.*` host calls (or the events a section opts into) decide what is conversation, what is progress, what is private. Fanout arms get their own `subagentRunId`-equivalent and never spray into the main stream unprompted.
4. **Subagent cards with drill-down** from day one: stable id, debounced status, dispatch prompt and final result reachable, detail view as a separate stream.
5. **Thinking streams to the UI only**; Lua sees `reply` and `sys`, never raw reasoning.

## Confidence

High on the event-log architecture, interrupt/resume HITL, and subagent-card patterns - three or more independent products converged on each, and the failure modes of doing otherwise are documented in public bug trackers. Medium on approval-ladder specifics and notification presence suppression - well evidenced but deferrable past the first version.

*2026-08-29 08:20 - kimi-k3*
