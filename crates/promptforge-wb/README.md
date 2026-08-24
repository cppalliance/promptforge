# promptforge-wb

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workbench desktop window. It starts the workbench server (`promptforge-wb-server`) in-process on its own thread, waits for its health endpoint to answer, and opens a native window (wry/tao, WebView2 on Windows) pointed at the chat UI. Closing the window shuts the server down cleanly. One binary, one process: the window and the server it frames ship together.

## Quick start

```bash
cargo run -p promptforge-wb
```

## Configuration

The shell loads the same `workbench.toml` as the server (see the [promptforge-wb-server README](../promptforge-wb-server/README.md) for the field reference), but searches for it in three places, first found wins:

1. Beside the executable
2. The current directory
3. `%USERPROFILE%\.promptforge\workbench.toml`

On first run - when no file exists at any of these locations - the shell creates `%USERPROFILE%\.promptforge\` if needed, writes a default `workbench.toml` there, logs the path, and loads it. The generated file interpolates the gateway settings from `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`; unset, they resolve to the built-in defaults (loopback gateway, no `Authorization` header). The app never exits on missing config.

## Browser fallback

The shell ignores `server.open_browser` - it has a window. That flag belongs to the server binary: run `promptforge-wb-server` directly with `open_browser = true` to use the workbench as a browser tab instead of a desktop window.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
