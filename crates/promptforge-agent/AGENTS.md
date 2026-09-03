# promptforge-agent

This crate is the promptforge library product's agent-program executor: `run_agent` drives a `.lua` agent program - one long-running Lua coroutine - over the library substrate, with `AgentConfig` carrying exactly what agents need.

- Agent executor only. The crate depends on `promptforge-lua`, `promptforge-model-client`, `promptforge-tools`, `promptforge-store`, and `promptforge-core-support` - never on `promptforge-parser`, `promptforge-tool-picker`, or `promptforge-core`. Core and this crate are sibling executors over the same substrate; neither depends on the other.
- Absent, not stubbed: `execute`, `fanout`, and `jump` do not exist in an agent VM. An agent touching them fails as an undefined global, exactly as a document prompt touching `models.chat` does. No courtesy stubs, no typed errors for absent calls.
- The driver is leaf dispatch only: tool calls go through the shared `promptforge_lua::dispatch_tool` body, never a duplicated dispatch loop.
- Agents have no sections: every observer call passes the agent's name (the `.lua` file stem, `AgentConfig::name`) as the `section` label. The SPA and the event JSONL both key on it.
