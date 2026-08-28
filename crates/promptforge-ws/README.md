# promptforge-ws

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workshop desktop window. It boots the merged gateway (`promptforge-gateway` with the `workshop` feature) in-process, waits for the hosted workshop's health endpoint to answer, and opens a native window (wry/tao, WebView2 on Windows) pointed at the workshop UI. Closing the window shuts the gateway down cleanly. One binary, one process: the window, the gateway, and the workshop it hosts ship together.

## Quick start

```bash
# Windows/Linux with the NVIDIA CUDA toolkit (voice on CUDA, the default):
cargo run -p promptforge-ws

# macOS (voice on Metal):
cargo run -p promptforge-ws --no-default-features --features metal

# Any machine without a GPU toolchain (voice stays off at runtime):
cargo run -p promptforge-ws --no-default-features
```

Building the voice engine compiles whisper.cpp, which needs `cmake` on
`PATH`. The workshop UI bundle needs Node.js and one `npm install` in
`crates/promptforge-ws-server/ui/` per checkout (see that crate's README).

## Configuration

The shell loads the gateway boot config `gateway.toml` (see the [promptforge-gateway README](../promptforge-gateway/README.md) for the field reference), searching three places, first found wins:

1. Beside the executable
2. The current directory
3. `~/.promptforge/gateway.toml` (Windows: `%USERPROFILE%\.promptforge\gateway.toml`)

On first run - when no file exists at any of these locations - the shell writes a default `gateway.toml` into `~/.promptforge/` (Windows: `%USERPROFILE%\.promptforge\`): a loopback `[server]` bind on 8081 with a freshly generated random `api_key`, and a `[workshop]` section hosting the UI on a second loopback listener with the current voice-model defaults. It also writes `profiles/default.toml` beside it (the gateway boots into a named profile; the generated one includes the boot config), logs the path, and loads the pair. An existing `profiles/default.toml` is never overwritten. The app never exits on missing config.

The shell always boots the `default` profile. Development against an external gateway uses the standalone `promptforge-ws-server` binary and its `workshop.toml`; that flow is unchanged.

## Voice on macOS

The shell carries what WKWebView needs for microphone capture: the
webview configuration allows capture on the plain-http loopback origin
(WebKit has no localhost exemption, unlike WebView2), and the executable
embeds an Info.plist with `NSMicrophoneUsageDescription` in its
`__TEXT,__info_plist` section (WKWebView hides `navigator.mediaDevices`
from a host without one).

The microphone permission itself is macOS's. On first capture the OS
prompts; note that TCC charges a process launched from a terminal to the
terminal app, so the prompt (and any past allow/deny) may belong to your
terminal rather than to `promptforge-ws`. If the mic reports "permission
denied" with no prompt, check System Settings > Privacy & Security >
Microphone for a switched-off entry.

## Browser opening

The generated config leaves `workshop.open_browser` off. Setting it in a boot config the shell loads opens a browser tab in addition to the desktop window, because the gateway honors the flag wherever it runs; the flag is meant for running the gateway (or the standalone `promptforge-ws-server`) without the shell.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
