# promptforge-agent

This crate is the PromptForge agent-program executor.

- Core and this crate are sibling executors over the same substrate. Neither executor depends on the other.
- Absent, not stubbed: `execute`, `fanout`, and `jump` do not exist in an agent VM. An agent touching them fails as an undefined global, exactly as a document prompt touching `models.chat` does. No courtesy stubs, no typed errors for absent calls.
- Tool calls go through the shared `promptforge_lua::dispatch_tool` body, never a duplicated dispatch loop.
- Every observer call uses the agent name as its stable `section` label.
