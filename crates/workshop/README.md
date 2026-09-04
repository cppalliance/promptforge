# workshop

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workshop desktop window. It hosts the workshop server in-process on a loopback listener with an OS-assigned port, waits for its health endpoint to answer, and opens a Tauri window (WebView2 on Windows) pointed at it. The server resolves the gateway endpoint itself: a running gateway's connection file first, explicit `workshop.toml` config second. Closing the window stops the in-process server only - the gateway is a separate process and keeps running.

## Quick start

```bash
cargo run -p workshop
```

## Configuration

The shell reads no `gateway.toml`; the gateway owns its own configuration. What the shell discovers is `workshop.toml`, searching three places, first found wins:

1. Beside the executable
2. The current directory
3. `%USERPROFILE%\.promptforge\workshop.toml`

The file supplies the `[gateway]` connection (`base_url`, `api_key`) for attaching to a gateway discovery cannot see - a LAN gateway - plus the state and agent-program paths. The listener settings are the shell's own and cannot be configured away: the in-process server always binds `127.0.0.1:0` and never opens a browser. With no `workshop.toml`, state anchors in `%USERPROFILE%\.promptforge\` and the gateway endpoint resolves through the connection file a running gateway writes. With no gateway running and no explicit config, boot fails with the plain no-gateway error naming both remedies.

Development against the standalone `workshop-server` binary flow is unchanged.

## Browser opening

The shell drives its own window and never opens a browser tab. The `open_browser` flag belongs to the standalone `workshop-server` binary.

## Native runtimes

The desktop build has no native-backend feature flags. At run time the artifact store downloads the pinned whisper.cpp bundle for the host - CUDA on Windows, Metal on Apple Silicon, and CPU on the other supported targets - alongside the managed `llama-server`.

## Updates

The installed desktop app checks the latest GitHub Release after startup. Signed updater bundles are verified with the public key embedded in `tauri.conf.json`; the matching private key exists only in the release workflow secrets. Help > About PromptForge also provides a manual check.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
