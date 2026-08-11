# promptforge-mcp-server

The PromptForge MCP server. It runs PromptForge prompts for an agentic harness
(Cursor, Claude Code) behind a fixed set of built-in tools: a prompt runs
because a caller named it to `run_prompt`, so the catalog is never published as
individual tools and `tools/list` is stable whatever the catalog holds.

## Running

```
promptforge-mcp-server serve [--stdio] <prompts.toml>
```

- Without `--stdio`, the server binds the streamable-HTTP transport at `/mcp`
  behind a shared bearer, with an unauthenticated `/healthz` beside it.
- With `--stdio`, the server speaks JSON-RPC over standard input and output and
  never reads the shared token.

`prompts.toml` names the bind address, the shared token, the prompts directory,
the gateway every run's model calls go through, and which prompts the harness
sees. See the crate's library documentation for the configuration shape and the
boot/serve seam.

## License

BSL-1.0
