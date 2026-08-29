# Document prompts and harness programs: prior art and design options for PromptForge

## Answer

PromptForge needs two prompt types. A document prompt uses sections, prose, and the semantic picker to do structured work. A harness program uses Lua to drive an agent loop with explicit model calls, tool dispatch, and context assembly. Every surveyed framework arrived at this split. No system uses an embedded scripting language for the agent loop. PromptForge would be the first.

## Evidence base

Five parallel research threads produced 116 finding cards across 40+ systems. The full evidence is in the companion research files dated 2026-08-29. This report draws conclusions from that evidence.

## The split is real and universal

Every framework that ships both simple and complex AI work has two authoring surfaces. The terminology varies (declarative/imperative, template/programmatic, pipeline/agent, low-code/pro-code) but the boundary is consistent:

- Promptflow: DAG Flow (.dag.yaml + .jinja2) vs Flex Flow (.flex.yaml + .py)
- CrewAI: Crews (role/goal/backstory config) vs Flows (@start/@listen/@router code)
- Haystack: Pipeline (YAML DAG) vs Agent (Python hooks)
- LlamaIndex: QueryEngine (single-turn retrieval) vs Agent (reasoning loop with tools)
- Semantic Kernel: YAML prompt templates vs ChatCompletionAgent
- LangChain: prompt templates vs StateGraph
- AWS Bedrock: Prompt Management (ARN-addressed templates) vs AgentCore (microVMs)
- Vercel AI SDK: generateText/streamText vs ToolLoopAgent

The split is not a design choice. It is a discovery. Every framework started with templates and grew an agent layer. None went the other way. PromptFlow's YAML DAG is being retired April 2027, replaced by code-first Python. The evolutionary trajectory is one-directional: template, then agent loop, then harness.

## The layering pattern

The dominant relationship between the two modes is containment, not coexistence. Templates become leaf components inside programmatic harnesses:

- DSPy modules inside LangGraph nodes
- Prompt templates inside LCEL pipes
- BAML document compiles to typed client code
- LlamaIndex QueryEngine becomes a tool inside an Agent via QueryEngineTool
- Haystack Pipeline becomes a tool inside an Agent via PipelineTool

The document is absorbed as a submodule of the program. This is directly relevant to PromptForge: a harness .lua file should be able to call a document prompt via execute(), and a document prompt should work unchanged. The two types compose.

## No system uses a scripting language for the loop

This is the most important finding. The seven major agent harnesses surveyed occupy a spectrum:

| System | Loop | User can change the loop? |
|---|---|---|
| Cursor | Hardcoded ReAct while-loop | No. Rules, hooks, MCP extend it. |
| Claude Code | Hardcoded ReAct async generator | No. CLAUDE.md, hooks, MCP extend it. |
| OpenAI Agents SDK | Hardcoded while-loop in Runner | No. RunConfig, handoffs, guardrails configure it. |
| Microsoft Agent Framework | Hidden agent loop + BSP workflow | Workflow graph is user-defined; inner loop is not. |
| AG2 | Middleware-driven model loop | Yes. Middleware stack is composable. Closest to scriptable. |
| LangGraph | User-defined StateGraph | Yes. The graph IS the loop. Maximum control. |
| Bedrock AgentCore | Harness (config) or Runtime (bring your own) | Split: managed or fully custom. |

Configurability comes through composition patterns (middleware in AG2, graph topology in LangGraph, hooks in Cursor, handoffs in OpenAI), never through interpreted loop scripts. PromptForge using Lua for the agent loop is genuinely novel.

## Three projects already use Lua for LLM agent work

The combination of Lua-as-agent-runtime has independent prior art, but for tool execution, not loop control:

- onetool (Rust + sandboxed Lua REPL) - LLM writes Lua for computation instead of using dozens of specialized tools. Functions tiered as safe/unsafe/forbidden.
- ORCS-CLI (Rust + capability-gated Lua) - every agent behavior (agents, skills, tools) is a Lua script with explicit capability grants.
- Lua.ex (pure Elixir Lua VM) - one VM per conversation, per tool call, per user. Designed for AI agent use.

All three chose Lua for the same reasons: small runtime, clean embedding API, straightforward sandboxing, LLMs write correct Lua.

## The host/script boundary patterns

Seven domains of embedded Lua scripting provide the design vocabulary:

**Game AI (behavior trees).** Host provides frame timing and world state. Script provides decision logic. Communication is through a shared blackboard (read/write data) and success/fail/running signals. This maps to: host provides model calls and tool dispatch, script provides the agent policy, communication is through the event log and var.

**Redis.** Host provides atomicity (single-threaded execution) and the data store. Script provides multi-step logic that would otherwise require round-trips and optimistic locking. The script cannot escape the sandbox. This maps to: host provides async execution and the gateway client, script provides the turn logic.

**OpenResty.** Host owns the event loop and request lifecycle. Script provides logic within defined phases. The configuration file literally contains both declarative config and imperative code. This is the closest structural parallel to PromptForge's document/harness split.

**Neovim.** Host provides the editor core. Script provides all high-level features (LSP, treesitter, plugins). Trust is implicit - if you installed the plugin, you trust it. The boundary has been moving toward Lua over time. This maps to: human-authored harness programs get Neovim-level trust; LLM-authored code would need tighter controls.

**WoW addons.** The most sophisticated Lua sandboxing in production. Taint propagation tracks trust through data flow. Protected functions require hardware events. This is relevant if PromptForge ever runs untrusted prompts from a marketplace.

**Roblox/Luau.** Capability-based sandboxing with hierarchical permission intersection. Relevant for multi-tenant prompt execution (MCP server running untrusted prompts).

## The "prompt-as-program" ecosystem

Thirteen systems treat the prompt specification as executable code:

- Constrained-decoding engines (LMQL, SGLang, Guidance, Outlines) control generation token-by-token
- Schema-contract systems (Instructor, BAML, DSPy) treat the output shape as the spec
- Typed agent frameworks (Pydantic AI, Genkit, Marvin) wrap agent loops in type-safe functions

The industry signal is clear: config/YAML approaches are being retired. Code-first with type safety is the direction. BAML is the most relevant parallel - a DSL compiled to typed clients, with the thesis that "agent-authored software needs a source format precise enough for machines to edit and legible for humans to own."

## What this means for PromptForge

The evidence supports two prompt types sharing one runtime:

**Document prompts (.md)** - the current model. Sections, prose, variable substitution, the semantic picker, the built-in tool loop. The structure carries meaning. For reports, analyses, and structured single-run work. Unchanged.

**Harness programs (.lua)** - a Lua program with access to the same host calls. No sections, no prose, no picker, no implicit tool loop. For interactive agents, chat loops, and long-lived sessions. New.

The runtime provides the same kernel to both: the gateway client (models.chat, models.infer), tool dispatch (tool_call), the Observer (runtime.events), the store, var, and cancellation. The entry point differs: the document prompt is parsed and walked by the section executor; the harness program is loaded and run as a Lua main loop.

Composition: a harness program can call a document prompt via execute(). A document prompt is a callable unit of work that runs and returns a result. The harness orchestrates; the document does focused work. This matches the dominant layering pattern found across every surveyed framework.

## Confidence levels

- The two-type split is necessary: high. Every framework arrives here. The evolutionary direction is one-way.
- Lua is the right scripting language: high. Three independent projects chose it for the same reasons. The embedding API is designed around the host/script boundary. PromptForge already uses it.
- The agent loop as a Lua program is novel: high. No surveyed system does this. The closest are LangGraph (user-defined graph) and AG2 (composable middleware), both in the host language.
- Document prompts can invoke harness programs and vice versa: medium. The evidence supports layering (documents as components of programs), but bidirectional invocation is less common. Haystack and LlamaIndex do it; most others layer one way.

## Open question

Where does the harness .lua file live? Three options:

1. In prompts/ alongside .md files, distinguished by extension. Simple, but mixes two very different things.
2. In a separate directory (harnesses/, agents/). Clean separation, but splits the prompt catalog.
3. As a section type within a .md file (a pure-Lua section with no prose). Keeps one file format, but stretches the document model.

The evidence does not settle this. Promptflow uses separate file extensions (.dag.yaml vs .flex.yaml). BAML uses its own extension (.baml). Most frameworks use the host language's native file format (.py, .ts). The pragmatic choice is probably option 1: .lua files in prompts/, with the runtime detecting the format from the extension.

*2026-08-29 11:17 - claude-opus-4-8-thinking*
