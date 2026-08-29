# What agent harnesses put in their contexts

Eight coding-agent harnesses examined. Every one assembles a structured, multi-role message list each turn. The system prompt carries identity and tool guidance; project context is injected as attachments or synthetic messages; conversation history accumulates with compaction; and tool schemas ride as a separate API parameter. The differences are in how much the harness re-derives each turn, how it decides what to include, and when it compacts.

## The universal context structure

Every harness examined sends the model a message list with the same five layers, in this order:

1. **System prompt** - agent identity, behavioral rules, tool-use guidance, output format instructions
2. **Injected project context** - files, rules, workspace state, environment info
3. **Conversation history** - prior turns, possibly compacted
4. **Current user input** - the latest message plus any attached context
5. **Tool schemas** - sent as a separate API parameter, not inline in messages

No harness deviates from this ordering. The variation is in what fills each layer, how it is formatted, and how the budget is managed.

## Per-harness findings

### 1. Cursor

- System prompt uses XML-tagged sections with separate blocks for identity, rules, and tool schemas; [leaked prompt analysis](https://github.com/x1xm/Cursor-System-Prompt) shows the full structure
- Three-message structure: system, injected-rules user message (`<custom_instructions>`, `<required_instructions>`), dynamic-context user message (RAG results + IDE state + query)
- [Priompt](https://cursor.com/blog/dynamic-context-discovery) manages the token budget via priority-scored JSX components with binary search for the optimal cutoff - low-priority content drops declaratively
- Files are the universal abstraction: terminal sessions, MCP schemas, chat history, and skills all appear as file-shaped content
- Summarization writes full history to disk before compressing, giving the agent a searchable backup; subagents get isolated context windows
- MCP schema [lazy-loading reduced tokens by 46.9%](https://cursor.com/blog/dynamic-context-discovery)

### 2. Claude Code

- System prompt split by a cache boundary marker: static sections (identity, safety, tool usage, tone - globally cached) and dynamic sections (per-session)
- CLAUDE.md is NOT in the system prompt; it is [injected as a synthetic user message](https://arxiv.org/html/2604.14228v1) wrapped in `<system-reminder>` tags, re-read from disk every turn - this preserves the global system prompt cache (92% prefix reuse)
- ~25 attachment types computed per turn including `changed_files`, `nested_memory`, `skill_discovery` - [deterministic rule-based context injection](https://arxiv.org/html/2604.14228v1), not vector-search RAG
- Five-layer compaction pipeline executed every turn: tool-result budget caps, history snip, microcompact (cheap cleanup of old Read/Bash/Grep results), context collapse, auto-compact (triggers at ~83.5% of 200K tokens, shrinks ~85%)
- Strict alternating user/assistant roles; tool results sent as user messages with `tool_result` blocks
- 40+ tools defined via Zod v4 schemas converted to JSON Schema

### 3. Zed

- System prompt is a [Handlebars template](https://github.com/zed-industries/zed) with conditional blocks: tool guidance appears only when tools are enabled, sandbox rules only when terminal is active, skills catalog capped at 50KB
- Project context injected via worktree paths, personal AGENTS.md (`~/.config/zed/AGENTS.md`), project rules (first match from a priority list including `.cursorrules`, `.clinerules`, `CLAUDE.md`, `AGENTS.md`), and skills catalog (name and description only - full skill body loaded on demand via `skill` tool)
- MCP tools merged from context servers, gated by profile, deduplicated with server-name prefix
- The last history message gets `cache: true` for provider prompt caching; system message is always `cache: false`
- Compaction inserts a `Message::Compaction(Summary(...))` as a synthetic user message: "The previous conversation was compacted. Use this summary as context:" - retains up to 80KB of recent user messages before the compaction point
- Tool schemas use schemars-generated JSON Schema, adapted per provider format

### 4. Unsloth Studio

- System prompt assembled in layers: client-supplied system messages, then server nudges (date, web/code/artifact guidance when those tools are active, RAG grounding nudge, compaction nudge), then a [carried-forward block](https://github.com/unslothai/unsloth) after checkpoint reset
- Files not bulk-injected; context arrives through RAG autoinject (top-K hybrid search from project corpus), whole-document mode for small thread attachments, and tools reading files on demand at runtime
- Checkpoint compaction resets the epoch to `[system + X] + [newest turn]` where X is verbatim user instructions from evicted turns, selected newest-first under a 1024-token / 8-item cap, rendered in a `<carried_forward>` block that explicitly states it is a lossy record and the newest user message outranks it
- RAG recall injected after compaction: archived evicted turns are searchable via `search_conversation` tool or inline `<recalled_conversation>` prefix
- Tool schemas sanitized against chat-template control markup injection before rendering to llama-server
- Instruction pinning (off by default) protects standing user instructions from eviction during rolling-window compaction

### 5. OpenAI Agents SDK

- Agent instructions are a string or callable that resolves each turn via `get_system_prompt()`, sent as the `instructions` parameter to the [Responses API](https://developers.openai.com/api/docs/guides/agents/running-agents)
- All tools unify through `Converter.convert_tools()`: function tools and MCP tools become `FunctionToolParam` with JSON Schema; handoffs become `transfer_to_<Name>` function tools; hosted tools (web search, file search, code interpreter) use typed params
- Conversation history is `original_input + generated_items` accumulated across turns; on handoff, `HandoffInputData` carries input history, pre-handoff items, and new items
- `nest_handoff_history` (opt-in beta) compacts prior transcript into a [numbered assistant summary wrapped in `<conversation_history>` markers](https://openai.github.io/openai-agents-python/handoffs/)
- No built-in token counting or budget management; delegates to Responses API `truncation: "auto"` and server-side `context_management`; developers implement custom trimming via `call_model_input_filter` hook
- Guardrails are NOT injected into model context - they are Python-side validators

### 6. Aider

- System prompt opens with "Act as an expert software developer," teaches the specific edit format with few-shot examples, and injects platform info (OS, shell, date); a `system_reminder` at the end of context [repeats the format rules](https://aider.chat/docs/more/context.html) to fight instruction drift
- [Repo map](https://aider.chat/docs/repomap.html) uses Tree-sitter AST parsing plus personalized PageRank (50x boost for chat files, 10x for mentioned identifiers) to select the most relevant code definitions; binary-searches to fit within `max_map_tokens`
- `/add` files are editable; `/read-only` files are reference only - the model is instructed it can only propose edits to `/add` files
- Message ordering: system, examples, readonly files, repo map, summarized history, chat files, current user message, reminder - all non-system blocks use fake user/assistant pairs for role alternation
- No function/tool calling for edits - everything is text-based prompt engineering with regex parsing of SEARCH/REPLACE blocks
- History summarization splits old messages into head/tail, summarizes the head via a cheap model in first-person perspective

### 7. Cline / Roo Code

- System prompt is a [modular assembly](https://github.com/cline/cline): identity block, environment block (OS, shell, CWD, home dir), mode instructions, workspace JSON tree - about 15-30K tokens
- Custom rules from `.clinerules` files (with YAML frontmatter for conditional activation) and `.roo/system-prompt-{mode}` files with variable interpolation
- Tool definitions use XML-style inline specification in the system prompt (legacy) or SDK `createTool()` with Zod/JSON Schema (current)
- Compaction triggers at 90% utilization targeting 70%; two strategies: Basic (deterministic `<SYSTEM_NOTICE>` summaries) and Agentic (secondary LLM continuation notes)
- Tool results capped at 8K chars; stale file reads rewritten to `[outdated]`
- Roo's "Fresh Start Model" condensation replaces history with a summary as a user-role message but tags originals with `condenseParent` rather than deleting them

### 8. Devin / Codex CLI

- Devin's ~50KB system prompt defines a persona ("a real code-wiz"), a [three-mode state machine](https://x.com/yolanda_lau/status/1875624901652828357) (planning/standard/edit), mandatory `<think>` scratchpad scrubbed between turns (model never sees its own past reasoning), and cross-session memory via org-level Knowledge and Playbooks
- [Codex CLI](https://github.com/openai/codex) uses model-family-specific markdown instruction files embedded at compile time; five instruction layers with explicit compaction lifetimes - base instructions survive compaction, developer messages do not
- AGENTS.md is concatenated root-to-cwd, capped at 32 KiB, delivered as user-role
- Codex compaction triggers at 90% context window; two paths: remote (encrypted latent state via `/responses/compact`) or inline summarization; after compaction, only the last 20K tokens of user messages survive plus ghost snapshots for undo
- Diff-based environment context updates minimize redundancy between turns
- Devin uses monolithic prompt with full re-injection each turn; Codex uses typed XML fragment markers (`<agents_md>`, `<environment_context>`, `<skill>`) that get special treatment during rollback and trim passes

## Cross-cutting patterns

### What every harness puts in the system prompt

| Content | Present in all 8? | Notes |
|---|---|---|
| Agent identity and persona | Yes | Tone varies from formal (Claude Code) to casual (Devin) |
| Tool-use guidance | 7 of 8 | Aider has no tool calling; uses edit-format instructions instead |
| Output format instructions | Yes | Edit formats, response structure, safety rules |
| Platform/environment info | 7 of 8 | OS, shell, date, CWD; OpenAI SDK delegates to caller |
| Behavioral constraints | Yes | Safety, file limits, permission rules |

### What every harness injects as project context

| Content | How many? | Injection method |
|---|---|---|
| Custom rules files | 7 of 8 | CLAUDE.md, .cursorrules, .clinerules, AGENTS.md, .roo/ |
| File tree or repo map | 6 of 8 | Tree-sitter AST (Aider), workspace JSON (Cline), worktree paths (Zed) |
| Currently open or recently edited files | 5 of 8 | IDE state injection (Cursor, Cline, Zed) or on-demand tool reads |
| Git state | 4 of 8 | Branch, recent commits, diff status |
| Linter/diagnostic output | 3 of 8 | Cursor, Cline, Zed |

### How harnesses manage the context budget

| Strategy | Used by | Trigger |
|---|---|---|
| Priority-based token budgeting | Cursor (Priompt) | Every turn; binary search for optimal cutoff |
| Multi-layer pipeline (cheapest first) | Claude Code, Unsloth | Every turn; early-stop when budget is met |
| Threshold-triggered summarization | Cline, Codex, Zed | 83-90% utilization |
| Server-side delegation | OpenAI SDK | Always; `truncation: "auto"` |
| Self-managed components | Aider | Each component (repo map, history) manages its own cap |

### How compaction appears to the model

| Strategy | Used by | What the model sees |
|---|---|---|
| Synthetic user message with summary | Zed, Roo, Aider | "The previous conversation was compacted..." |
| Synthetic user message in `<system-reminder>` tags | Claude Code | Re-read from disk, preserves cache |
| `<carried_forward>` block in system message | Unsloth | Verbatim user instructions, not summaries |
| `<conversation_history>` numbered transcript | OpenAI SDK | Nested summary on handoff |
| Instruction layers with compaction lifetimes | Codex | Base instructions survive; developer messages do not |
| Tool result replacement | Claude Code, Cline | Old results replaced with placeholders or `[outdated]` |

### Cache optimization strategies

| Strategy | Used by |
|---|---|
| Static/dynamic split in system prompt with cache boundary | Claude Code |
| `cache: true` on last history message | Zed |
| Rules re-read from disk (not preserved in history) so prefix stays stable | Claude Code |
| Checkpoint compaction preserves system prefix stability | Unsloth |
| Diff-based environment updates | Codex |

## Recommendations for PromptForge harness programs

The harness `.lua` program builds `models.chat` message lists. Based on these eight systems, the context should contain:

1. **System messages (stable prefix):** Agent identity, behavioral rules, tool-use guidance. Keep this stable across turns for prefix cache reuse. Change only when the agent type or tool set changes.

2. **System messages (dynamic):** Platform info (from `ui` table), workspace paths, date. These change per session but not per turn.

3. **Injected context (per-turn):** Current file from `ui.open_file`, relevant files from the event log, custom rules. Inject as user-role messages or system messages, separate from the conversation history.

4. **Compacted history:** Read from `runtime.events()`, apply observation masking (replace consumed tool outputs with placeholders), then optional summarization. Present as a user message.

5. **Recent history (verbatim):** The tail of `runtime.events()`, kept at full fidelity.

6. **Current user input:** From `user_input()`, as the final user message.

The Lua program decides what goes in each layer. Different agent types fill the layers differently. The runtime provides the primitives (`models.chat`, `runtime.events()`, `ui`, `tool_call`); the program provides the policy.

Confidence: high for the five-layer structure and the compaction patterns - all eight harnesses converge on them. Medium for the specific cache optimization strategies - these depend on provider support and may not apply to all backends.

*2026-08-29 11:43 - claude-opus-4-8-thinking*
