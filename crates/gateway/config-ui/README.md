# gateway-config-ui

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The PromptForge gateway config UI: the embedded SPA assets and the esbuild build pipeline, with the asset routes behind the shared loopback wall. The gateway pulls this crate as an optional dependency behind its `config-ui` feature and nests the exported router at `/config`, so the SPA is served on the gateway's own port with no second listener.

## Public surface

- `routes()` - an axum `Router` serving the SPA assets (`/`, `/app.js`, `/app.css`, `/icons/promptforge-icon.png`, `/icons/promptforge-icon@2x.png`) with the loopback wall already applied. The asset routes carry no bearer auth - the SPA shell holds no secrets - and every route answers `403 Forbidden` to a peer that is not loopback.
- `require_loopback` - the loopback middleware, re-exported from the always-on `shared-loopback` crate, which the gateway depends on directly so its admin config endpoints carry the same single check even in headless builds that never compile this crate. It reads the peer address from the `ConnectInfo<SocketAddr>` request extension (present when the server is started with `into_make_service_with_connect_info::<SocketAddr>()`) and fails closed: a request with no peer address is refused as non-loopback.

## UI development

The UI is TypeScript under `ui/src/`, bundled by esbuild. Building the crate requires Node.js 22: run `npm ci` in `ui/` once per checkout. Every `cargo build` runs the UI build through the crate's `build.rs` (via the shared `build-ui` helper), writing the bundle to `$OUT_DIR/ui-dist/` - never into the repository. Debug builds read the bundle from disk on every request; release builds minify and embed it into the binary. `ui/node_modules/` and `ui/dist/` are gitignored.

The workflow: edit the TypeScript, then `cargo build`. The build script re-bundles whenever `ui/src/` or the static UI files change - a build-script-only rerun, no Rust recompile - and debug builds read the bundle from disk on every request. `npm run build` and `npm run watch` in `ui/` still write `ui/dist/` in place, which nothing serves: that tree exists for the jsdom tests, which import the built bundle.

`npm run typecheck` runs `tsc --noEmit`; esbuild strips types without checking them, so the typecheck is advisory. `npm test` runs `node --test` over the colocated `src/**/*.test.mjs` files; the suite includes a jsdom smoke test that imports the built `dist/app.js` and asserts the shell mounts (run `npm run build` first).

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
