# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-12

### Added

- Initial public release of PromptForge
- promptforge-core: prompt parser, Lua sandbox, execution engine, model resolution, tool dispatch, fanout, virtual store
- promptforge-cli: command-line prompt runner
- promptforge-gateway: OpenAI-compatible model backend with TOML configuration, named profiles, and local GGUF inference
- promptforge-mcp-server: MCP tool server for agentic harnesses (Cursor, Claude Code)
- promptforge-tool-picker: semantic tool resolution with embedded embedding model
- promptforge-webfetch: web page fetcher with readability extraction and SSRF protection
- promptforge-dev: interactive development runner with watch mode and store dump inspection
- User guide published at https://cppalliance.github.io/promptforge/

[0.1.0]: https://github.com/cppalliance/promptforge/releases/tag/v0.1.0
