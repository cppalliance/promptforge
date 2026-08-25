# promptforge-ws

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workshop desktop window. It starts the workshop server (`promptforge-ws-server`) in-process on its own thread, waits for its health endpoint to answer, and opens a native window (wry/tao, WebView2 on Windows) pointed at the chat UI. Closing the window shuts the server down cleanly. One binary, one process: the window and the server it frames ship together.

## Quick start

```bash
cargo run -p promptforge-ws
```

## Configuration

The shell loads the same `workshop.toml` as the server (see the [promptforge-ws-server README](../promptforge-ws-server/README.md) for the field reference), but searches for it in three places, first found wins. At each place, `workshop.toml` is preferred and a leftover `workbench.toml` is still accepted:

1. Beside the executable
2. The current directory
3. `%USERPROFILE%\.promptforge\workshop.toml` (then `workbench.toml`)

On first run - when no file exists at any of these locations - the shell creates `%USERPROFILE%\.promptforge\` if needed, writes a default `workshop.toml` there, logs the path, and loads it. The generated file interpolates the gateway settings from `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`; unset, they resolve to the built-in defaults (loopback gateway, no `Authorization` header). The app never exits on missing config.

## Browser fallback

The shell ignores `server.open_browser` - it has a window. That flag belongs to the server binary: run `promptforge-ws-server` directly with `open_browser = true` to use the workshop as a browser tab instead of a desktop window.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
