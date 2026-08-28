# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- promptforge-ws / promptforge-gateway / promptforge-ws-server: `metal` / `workshop-metal` / `metal` feature chain building the whisper voice engine with Metal acceleration, making workshop voice available on macOS (`cargo build -p promptforge-ws --no-default-features --features metal`)

### Fixed

- promptforge-gateway: the archive extractor now materializes tar symlink entries as regular-file copies of their targets (confined, chain-following, cycle-rejecting), so the macOS llama.cpp release tarballs, which ship dylibs behind versioned symlink chains, provision instead of failing with an unsafe-entry error
- promptforge-ws: microphone capture works in the macOS webview; the shell builds the webview configuration itself to allow capture on the plain-http loopback origin, and embeds an Info.plist with `NSMicrophoneUsageDescription` in the executable so WKWebView exposes `navigator.mediaDevices`

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
