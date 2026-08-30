# promptforge-gateway-config-ui

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The PromptForge gateway config UI: the embedded SPA assets, the esbuild build pipeline, and the shared loopback wall. The gateway pulls this crate as an optional dependency behind its `config-ui` feature and nests the exported router at `/config`, so the SPA is served on the gateway's own port with no second listener.

## Public surface

- `routes()` - an axum `Router` serving the SPA assets (`/`, `/app.js`, `/icons/promptforge-icon-1.png`) with the loopback wall already applied. The asset routes carry no bearer auth - the SPA shell holds no secrets - and every route answers `403 Forbidden` to a peer that is not loopback.
- `require_loopback` - the loopback middleware itself, exported so the gateway applies the same single check to its admin config endpoints. It reads the peer address from the `ConnectInfo<SocketAddr>` request extension (present when the server is started with `into_make_service_with_connect_info::<SocketAddr>()`) and fails closed: a request with no peer address is refused as non-loopback.

## UI development

The UI is TypeScript under `ui/src/`, bundled by esbuild into `ui/dist/app.js`. Node.js >= 22 is required: run `npm ci` in `ui/` once per checkout. After that, debug `cargo build` runs the UI build itself (the crate's `build.rs` prefers `ui/node_modules/.bin/esbuild` and falls back to `npx esbuild`, which may download esbuild on first use). `ui/node_modules/` and `ui/dist/` are gitignored.

Two workflows:

1. **Just cargo:** edit the TypeScript, then `cargo build`. The build script re-bundles whenever `ui/src/` or the static UI files change, and debug builds read `ui/dist/` from disk on every request.
2. **esbuild watch:** run `npm run watch` in `ui/` in one terminal and the gateway in another. Edit, save, refresh the browser - no Rust recompile for UI changes.

`npm run typecheck` runs `tsc --noEmit`; esbuild strips types without checking them, so the typecheck is advisory. `npm test` runs `node --test` over the colocated `src/**/*.test.mjs` files; the suite includes a jsdom smoke test that imports the built `dist/app.js` and asserts the shell mounts (run `npm run build` first; a debug `cargo build` also produces `dist/`).

## Release artifact verification

Release builds embed a verified, minified artifact: `build.rs` checks `ui/dist/manifest.json` (schema version, minified flag, a sha256 over every build input, and the dist file list) and, when the manifest is absent or stale against the current sources, produces the artifact itself by running `node build.mjs --package` in `ui/` (the same command as `npm run package`) before verifying and embedding. A single `cargo build --release` is sufficient, including after UI edits and after a debug build wiped `ui/dist/`; the build fails with instructions only when the artifact cannot be produced (for example Node.js or `ui/node_modules` missing) or still does not verify.

The verifier lives in `build/manifest.rs`, shared with the test build through `#[path]`; its input-hash algorithm is mirrored exactly in `ui/manifest.mjs`, and the two files must change together.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
