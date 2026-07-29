# PromptForge Status

## What exists and works

Tranche 1 complete. `promptforge run prompts/hello.md` parses the file, sends the
entry section's prose to the model, and prints the reply ("Hello, world!").

- `promptforge-core::parser` - frontmatter + H1/description + recursive H2-H6 tree + leading Lua-fence separation. 10 unit tests, all green.
- `promptforge-core::client` - OpenAI-compatible chat completions (non-streaming).
- `promptforge-core::execute::run` - one round trip on the entry section.
- `promptforge-cli` - `promptforge run <file.md>`.

## What's next

Tranche 2: the `call` control-flow tool, the tool-call loop, and multi-section
fall-through (context clears on each transition). Then Lua.

## How to run

```
export ANTHROPIC_API_KEY=sk-ant-...
cargo run -p promptforge-cli -- run prompts/hello.md
```

## Decisions settled

- Rust multi-crate workspace: promptforge-core (lib), promptforge-cli (bin)
- Edition 2024, resolver 3, rust-version 1.85
- Lint policy in [workspace.lints]: unsafe_code forbid, missing_docs, clippy all=deny + pedantic=warn, unwrap_used deny (doc_markdown allowed for product names); tests allowed unwrap/expect via clippy.toml
- Public error type is #[non_exhaustive] and does not leak dependency error types
- OpenAI-compatible HTTP (Anthropic URL hardcoded now, gateway later)
- Base URL default is Anthropic's endpoint; override with PROMPTFORGE_BASE_URL
- API key from PROMPTFORGE_API_KEY, else ANTHROPIC_API_KEY. Required.
- Default model claude-sonnet-4-6; override with PROMPTFORGE_MODEL
- Entry point is the first H2, not a named section
- Recursive heading nesting (H2-H6); skipped levels tolerated
- Section ends when the model returns text with no tool calls (auto termination)
- tool_choice: auto when tools present, required when only call is present, omitted when no tools
- `call` is the unified control-flow tool (type discriminator: return, goto, task, fanout) - keeps control-flow tool count at 1
- No Lua yet (tranche 2). Streaming later (Talktron needs it).

## Open questions

- call syntax: positional vs table-form
- Context-preserving call: support it or not?
