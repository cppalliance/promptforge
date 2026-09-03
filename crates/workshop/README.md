# workshop

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workshop desktop window. It boots the merged gateway (`gateway` with the `workshop` feature) in-process, waits for the hosted workshop's health endpoint to answer, and opens a Tauri window (WebView2 on Windows) pointed at the workshop UI served on the gateway's loopback listener. Closing the window shuts the gateway down cleanly. One binary, one process: the window, the gateway, and the workshop it hosts ship together.

## Quick start

```bash
cargo run -p workshop
```

## Configuration

The shell loads the gateway boot config `gateway.toml` (see the [gateway README](../gateway/README.md) for the field reference), searching three places, first found wins:

1. Beside the executable
2. The current directory
3. `%USERPROFILE%\.promptforge\gateway.toml`

On first run - when no file exists at any of these locations - the shell writes a default `gateway.toml` into `%USERPROFILE%\.promptforge\`: a loopback `[server]` bind on 8081 with a freshly generated random `api_key`, and a `[workshop]` section hosting the UI on a second loopback listener with the current voice-model defaults. It also writes `profiles\default.toml` beside it (the gateway boots into a named profile; the generated one includes the boot config), logs the path, and loads the pair. An existing `profiles\default.toml` is never overwritten. The app never exits on missing config.

The shell always boots the `default` profile. Development against an external gateway uses the standalone `workshop-server` binary and its `workshop.toml`; that flow is unchanged.

## Browser opening

The generated config leaves `workshop.open_browser` off. Setting it in a boot config the shell loads opens a browser tab in addition to the desktop window, because the gateway honors the flag wherever it runs; the flag is meant for running the gateway (or the standalone `workshop-server`) without the shell.

## Native runtimes

The desktop build has no native-backend feature flags. At run time the artifact store downloads the pinned whisper.cpp bundle for the host - CUDA on Windows, Metal on Apple Silicon, and CPU on the other supported targets - alongside the managed `llama-server`.

## Updates

The installed desktop app checks the latest GitHub Release after startup. Signed updater bundles are verified with the public key embedded in `tauri.conf.json`; the matching private key exists only in the release workflow secrets. Help > About PromptForge also provides a manual check.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
