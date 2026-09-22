---
name: Workshop family container reorg
overview: Move the twelve workshop-* crates into a manifestless, fully private crates/workshop/ container with shortened directory names and unchanged package names, following the established promptforge/gateway family recipe. No crate renames, no identifier sweeps; the work is moves, manifests, path literals, and boundary enforcement.
todos:
  - id: harness
    content: build-xtask product.rs gains crates/workshop/ fully-private container rule + fixtures; sync architecture test matrix
    status: in_progress
  - id: moves
    content: git mv the twelve crates into crates/workshop/ with short dir names
    status: pending
  - id: manifests
    content: Root Cargo.toml members/exclude/workspace.dependencies
    status: pending
  - id: paths
    content: "Path literals: gitignore, package.json + lockfile, chat_gate.rs, gateway build.rs icon, sidecar tool, 6 workflow files, README, document.md"
    status: pending
  - id: docs
    content: AGENTS.md root rule + moved docs sweep
    status: pending
  - id: verify
    content: Cargo.lock regen, full verification battery, one commit
    status: pending
isProject: false
---

# Workshop Family Container Reorganization

## Why this shape

The family is already self-contained: no crate outside the family depends on any `workshop-*` crate (verified by manifest grep), and the tier graph (shell -> features -> services -> vocabulary) is already enforced by build-xtask. What is missing is what the other two families have: tree-level privacy and a tidy `crates/` root. After this, `crates/` reads as: build-*, gateway-api*, gateway/, promptforge-api-*, promptforge/, shared-*, workshop/, workspace-hack.

Package names stay unchanged everywhere, so no `use` statements, `cargo -p` flags, nextest filters, or CI package references change. Only paths move.

## Target shape

`crates/workshop/` is manifestless and fully private (no outside exception - nothing depends in). Directories drop the `workshop-` prefix; package names are unchanged.

The `crates/` root, before -> after:

```
BEFORE (12 workshop crates flat)          AFTER
crates/                                   crates/
  ...                                       ...
  workshop/            <- shell app         workshop/          <- manifestless container
  workshop-gateway/                           shell/           <- package `workshop`
  workshop-menu/                              server/          <- workshop-server
  workshop-protocol/                          server-api/      <- workshop-server-api
  workshop-registry/                          gateway/         <- workshop-gateway
  workshop-server/                            menu/
  workshop-server-api/                        protocol/
  workshop-sessions/                          registry/
  workshop-status/                            sessions/
  workshop-support/                           status/
  workshop-user-state/                        support/
  workshop-workspace/                         user-state/
  ...                                         workspace/
```

The container with key files:

```
crates/workshop/
  shell/                  <- the Tauri desktop app (package `workshop`)
    Cargo.toml
    build.rs              <- sidecar refresh, manifest-dir relative (unaffected)
    tauri.conf.json       <- crate-relative icons/ + binaries/ (unaffected)
    tauri.macos.conf.json, tauri.nightly.conf.json
    installer.nsi, Entitlements.plist, Info.plist
    AGENTS.md, README.md
    icons/                <- incl. icons/AGENTS.md
    binaries/             <- gitignored gateway sidecar staging
    gen/                  <- gitignored tauri-build ACL schemas
    src/
      gateway/supervisor.rs
  server/                 <- workshop-server: the HTTP/WS server
    Cargo.toml
    src/app.rs
    tests/it/             <- chat_gate.rs path fix lands here
    ui/                   <- the TypeScript SPA (package workshop-ui)
      package.json        <- shared-ui file: dep gains one ../
      build.mjs, src/, test/
  server-api/             <- workshop-server-api: the shell's re-export view
    src/lib.rs
    src/lib-tests.rs
  gateway/                <- workshop-gateway: bearer-auth gateway client
    src/observer.rs
  menu/                   <- workshop-menu: Model menu workbench + bus
  protocol/               <- workshop-protocol: wire frames, zero I/O
  registry/               <- workshop-registry: sealed subsystem slots
  sessions/               <- workshop-sessions: /ws and /agents/ws sockets
    agents/chat.md        <- referenced by server/tests/it/chat_gate.rs
  status/                 <- workshop-status: status-bar bus + renderer
  support/                <- workshop-support: vocabulary (atomic writes, backoff)
  user-state/             <- workshop-user-state: account UI state bucket
  workspace/              <- workshop-workspace: jailed /workspace/* fs
```

No nesting: the vocabulary trio (protocol, registry, support) stays flat; the tier graph already enforces layering.

## Execution order

1. **Harness first** (layout-agnostic parts only):
   - [crates/build-xtask/src/product.rs](c:\Users\Vinnie\cursor\promptforge\crates\build-xtask\src\product.rs): the `crates/workshop/` container is covered by the existing recursive walk and container-privacy rule - no new face needed since nothing outside depends in; add a fixture test proving outside-into-`workshop/` fails and sibling edges pass. One carve-out lands with it: `build-*` crates are meta tooling, exempt from container privacy in every container (workshop, promptforge, gateway) - today no build-* crate depends into any container, so the exemption changes no current fact and only prevents false positives against the build layer. NOTE: AGENTS.md requires explicit user approval for new topology checks - approved via this plan, exemption included.
   - Check whether [architecture.rs](c:\Users\Vinnie\cursor\promptforge\crates\gateway\stt\api\tests\it\architecture.rs) encodes container privacy or only family-name rules: its classification is package-name-based (line 45), so likely no change; if it encodes containers, add workshop.
   - The layout-dependent harness edits land AFTER the moves (step 2), because they are wrong against today's flat layout: [tidy.rs](c:\Users\Vinnie\cursor\promptforge\crates\build-xtask\src\tidy.rs):74's flat `root.join("crates").join(name).join("Cargo.toml")` lookup maps workshop packages into the container (`workshop` -> `workshop/shell`, `workshop-X` -> `workshop/X`), and [new_crate.rs](c:\Users\Vinnie\cursor\promptforge\crates\build-xtask\src\new_crate.rs):15 scaffolds at `crates/workshop/<suffix>/`. Only the final tree is verified; intermediate states need not pass.
2. **Moves**: the shell cannot move into its own directory, so: `git mv crates/workshop crates/ws-shell-tmp`, then `mkdir crates/workshop`, `git mv` the ten `workshop-*` crates to their container paths, then `git mv crates/ws-shell-tmp crates/workshop/shell`.
3. **Root manifest** [Cargo.toml](c:\Users\Vinnie\cursor\promptforge\Cargo.toml): `members` gains the twelve explicit container paths (gateway pattern), `exclude` gains `"crates/workshop"`, `[workspace.dependencies]` lines 56-66 repoint to the new paths.
4. **Path literals** (complete inventory from the full-repo sweep):
   - [.gitignore](c:\Users\Vinnie\cursor\promptforge\.gitignore):17,19,22,24-25 -> `/crates/workshop/server/ui/node_modules/`, `/crates/workshop/server/ui/dist/`, `/crates/workshop/shell/gen/`, `/crates/workshop/shell/binaries/`.
   - [.gitattributes](c:\Users\Vinnie\cursor\promptforge\.gitattributes):12,15,18 -> `crates/workshop/server/ui/**`, `crates/workshop/server/ui/**/*.png`, `crates/workshop/server/tests/it/observer/*.jsonl`.
   - [crates/workshop-server/ui/package.json](c:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui\package.json):45: `"shared-ui": "file:../../shared-ui"` -> `file:../../../shared-ui`; regenerate the lockfile (`npm install`).
   - [crates/workshop-server/ui/build.mjs](c:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui\build.mjs):28: repo-root lookup gains one `..`.
   - UI test files (all gain one `..` for the extra depth): `test/stt-stream.mjs`:13, `test/pcm-worklet.mjs`:10-18, `test/agent-stt.mjs`:17-26, `test/realtime-wire-fixtures.mjs`:21-29; `test/docs-claims.mjs`:14 repoRoot gains a level; `test/agent-wire-fixtures.mjs`:43 `"workshop-protocol"` -> `"workshop", "protocol"` segments.
   - [crates/workshop-server/tests/it/chat_gate.rs](c:\Users\Vinnie\cursor\promptforge\crates\workshop-server\tests\it\chat_gate.rs):48: `include_str!("../../../workshop-sessions/agents/chat.md")` -> `../../../sessions/agents/chat.md` (from the file's new home at `crates/workshop/server/tests/it/`, `../../../` is the container).
   - [crates/workshop-server/tests/it/realtime_relay.rs](c:\Users\Vinnie\cursor\promptforge\crates\workshop-server\tests\it\realtime_relay.rs):52-53: `../../../gateway/stt/...` -> `../../../../gateway/stt/...` (the crate sits one level deeper now).
   - [crates/gateway/app/build.rs](c:\Users\Vinnie\cursor\promptforge\crates\gateway\app\build.rs):23 and its :7 comment: `../../workshop/icons/icon.ico` -> `../../workshop/shell/icons/icon.ico`; same fix in [crates/gateway/app/Cargo.toml](c:\Users\Vinnie\cursor\promptforge\crates\gateway\app\Cargo.toml):19 comment and [crates/gateway/app/tests/it/icon.rs](c:\Users\Vinnie\cursor\promptforge\crates\gateway\app\tests\it\icon.rs):2,41; comment-only updates in `crates/gateway/app/src/tray/{windows,macos,linux}.rs`.
   - [tools/stage-gateway-sidecar.mjs](c:\Users\Vinnie\cursor\promptforge\tools\stage-gateway-sidecar.mjs):35-37: `crates/workshop/binaries` -> `crates/workshop/shell/binaries`; same in [tools/stage-gateway-sidecar.test.mjs](c:\Users\Vinnie\cursor\promptforge\tools\stage-gateway-sidecar.test.mjs):112-117.
   - [crates/build-ui/tests/it/main.rs](c:\Users\Vinnie\cursor\promptforge\crates\build-ui\tests\it\main.rs):16-19: `join("workshop-server").join("ui")` -> `join("workshop").join("server").join("ui")`.
   - [crates/build-workshop/tests/interruption.rs](c:\Users\Vinnie\cursor\promptforge\crates\build-workshop\tests\interruption.rs):49-53: sidecar path gains `shell/`.
   - Workflows: `crates/workshop-server/ui` -> `crates/workshop/server/ui` in [ci.yml](c:\Users\Vinnie\cursor\promptforge\.github\workflows\ci.yml) (9 sites), `dist-ci/build-setup.yml`, `llama-cuda-blackwell.yml`, `promptforge-gateway-v-release.yml`, `release-workshop.yml`, `nightly.yml`, `workshop-installer-smoke.yml`; `projectPath: crates/workshop` -> `crates/workshop/shell` and `crates/workshop/binaries` -> `crates/workshop/shell/binaries` in `release-workshop.yml`, `nightly.yml`, and `workshop-installer-smoke.yml` (also its `installer.nsi` and `tauri*.conf.json` trigger paths); every `cache-dependency-path` block gains `crates/workshop/*/ui/package-lock.json` (ci.yml x6, dist-ci/build-setup.yml, release-workshop.yml, nightly.yml x3).
   - [README.md](c:\Users\Vinnie\cursor\promptforge\README.md):74,84 and [tools/document.md](c:\Users\Vinnie\cursor\promptforge\tools\document.md):105,132 path updates (line 132 is already stale today - agents live under sessions - fix to `crates/workshop/sessions/agents/`).
   - LICENSE depth links in five crate READMEs: `../../LICENSE` -> `../../../LICENSE` in workshop/README.md:47, workshop-server/README.md:153, workshop-sessions/README.md:23, workshop-workspace/README.md:92, workshop-user-state/README.md:49.
   - `guide/scratch/` holds ~874 stale path citations: it is pipeline disposable - delete it (it regenerates on the next guide run). `guide/src/` itself has no crate-path references.
5. **Docs**: root [AGENTS.md](c:\Users\Vinnie\cursor\promptforge\AGENTS.md) gains the workshop container rule beside the promptforge/gateway ones, stated as the composed topology rule: "a crate in a family container (`crates/promptforge/`, `crates/gateway/`, `crates/workshop/`) may depend only on crates at the `crates/` root and its own siblings; the root is the public layer - `shared-*` substrate, `gateway-api`/`gateway-api-discovery`, `promptforge-api-runtime`/`promptforge-api-types` - and no outside crate may depend into a container. The `build-*` crates are meta tooling, exempt from container privacy." The shell's AGENTS.md and icons/AGENTS.md move with it; sweep crate docs for `crates/workshop-` path references.
6. **Cargo.lock** regen (paths changed; CI uses `--locked`, so the lockfile must be committed).

## What does NOT change

- Package names, binary names, `tauri.conf.json` (crate-relative icons/binaries), the shell's `build.rs` (manifest-dir relative), the `build-ui` helper (already upward-searches for shared-ui), the `## Invariants` markers, the 500-line ceiling exemptions, CI package-name partitions.

## Verification

- `cargo check --workspace --all-targets`
- `cargo test -p build-xtask` (container coverage + tidy.rs path mapping green), `cargo test -p build-ui`, `cargo test -p build-workshop`, `node tools/stage-gateway-sidecar.test.mjs`
- `cargo test -p gateway-stt --test it architecture` (matrix sync)
- The check-workshop battery locally: `cargo build --locked -p gateway --no-default-features`, sidecar stage/remove via the tool, `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, doctests for the three
- `npm ci` + `npm test` in `crates/workshop/server/ui` (the six fixture-path test files prove the depth fixes)
- `cargo build -p workshop` (shell builds; sidecar refresh lands in the moved `binaries/`)
- `cargo fmt --all --check`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (workshop exclusions as in CI)
- `rg "crates/workshop-"` and `rg "workshop-server/"` outside `vibe/`/`target/`/`guide/scratch/`: only the new paths
- `ls crates/` shows the tidy tree
- One commit.

Stop condition: two consecutive failures on one step stops the run for a re-plan.