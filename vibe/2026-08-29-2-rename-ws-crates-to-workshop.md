---
name: Rename ws crates to workshop
overview: Rename the promptforge-ws and promptforge-ws-server crates (packages, directories, binaries, and all references) to promptforge-workshop and promptforge-workshop-server in one atomic commit, verified by the full workspace gate set.
todos:
  - id: rename-manifests
    content: git mv both crate directories and update all four Cargo.toml manifests + Cargo.lock
    status: completed
  - id: rename-code
    content: Update Rust code references (gateway, both renamed crates, tests, name-pinning test)
    status: completed
  - id: rename-docs-ci
    content: Update CI workflows, .gitignore, READMEs, AGENTS.md files, doc comments, package.json/lock
    status: completed
  - id: verify
    content: Run the full gate set plus the one-command release proof and the grep sweep
    status: completed
  - id: commit
    content: Commit the rename as one atomic commit
    status: completed
isProject: false
---

# Rename promptforge-ws -> promptforge-workshop, promptforge-ws-server -> promptforge-workshop-server

## Rename map

- Directory `crates/promptforge-ws` -> `crates/promptforge-workshop` (via `git mv`)
- Directory `crates/promptforge-ws-server` -> `crates/promptforge-workshop-server` (via `git mv`)
- Package + binary `promptforge-ws` -> `promptforge-workshop`; `promptforge-ws-server` -> `promptforge-workshop-server`
- Rust path `promptforge_ws_server::` -> `promptforge_workshop_server::`
- UI package `promptforge-ws-ui` -> `promptforge-workshop-ui`; CI artifact `promptforge-ws-ui-dist` -> `promptforge-workshop-ui-dist`
- Thread name string `"promptforge-ws-server"` in `serve.rs`

## Edits

**Manifests**
- [crates/promptforge-workshop/Cargo.toml](promptforge/crates/promptforge-workshop/Cargo.toml): `name`, `[[bin]] name`
- [crates/promptforge-workshop-server/Cargo.toml](promptforge/crates/promptforge-workshop-server/Cargo.toml): `name`, `[[bin]] name`, and the self dev-dependency (`promptforge-ws-server = { path = ".", features = ["test-fixtures"] }`)
- [promptforge/Cargo.toml](promptforge/Cargo.toml): workspace member paths and the `promptforge-ws-server` workspace dependency entry
- [crates/promptforge-gateway/Cargo.toml](promptforge/crates/promptforge-gateway/Cargo.toml): the optional dependency and the `workshop` / `workshop-cuda` feature edges (`promptforge-ws-server/voice-cuda` -> `promptforge-workshop-server/voice-cuda`); feature names themselves already say `workshop` and stay
- `Cargo.lock` regenerates via `cargo check` and is committed

**Code**
- [crates/promptforge-gateway/src/workshop.rs](promptforge/crates/promptforge-gateway/src/workshop.rs) (12 refs) and [api_error.rs](promptforge/crates/promptforge-gateway/src/api_error.rs) (2 refs): `promptforge_ws_server::` paths
- ws-server crate itself: `src/main.rs`, `src/config.rs` (doctest), `src/tape.rs` (doctest), `src/serve.rs` (thread name), `tests/common/mod.rs`, `tests/it/voice.rs`
- [crates/promptforge-workshop/src/main.rs](promptforge/crates/promptforge-workshop/src/main.rs): the name-pinning test `crate_is_named_promptforge_ws` becomes `crate_is_named_promptforge_workshop` asserting `"promptforge-workshop"`

**CI**
- [.github/workflows/ci.yml](promptforge/.github/workflows/ci.yml): all `--exclude` pairs, the `crates/promptforge-ws-server/ui` working-directory paths, the `promptforge-ws-ui-dist` artifact name
- [.github/workflows/cuda.yml](promptforge/.github/workflows/cuda.yml): `cargo build -p promptforge-ws`

**Docs and config comments**
- Root [README.md](promptforge/README.md) crate table rows and root [AGENTS.md](promptforge/AGENTS.md)
- Crate AGENTS.md files: workshop, workshop-server, workshop-server/ui, plus the references in [promptforge-desktop-shell/AGENTS.md](promptforge/crates/promptforge-desktop-shell/AGENTS.md) and [promptforge-transcribe/AGENTS.md](promptforge/crates/promptforge-transcribe/AGENTS.md)
- Crate READMEs of both renamed crates; [promptforge-gateway/README.md](promptforge/crates/promptforge-gateway/README.md); [promptforge-desktop-shell/src/lib.rs](promptforge/crates/promptforge-desktop-shell/src/lib.rs) doc comment
- [.gitignore](promptforge/.gitignore): the two `/crates/promptforge-ws-server/ui/...` path entries
- Comment-only references: `workshop.example.toml`, `module-ceilings.toml` header, `build/manifest.rs` header, `ui/src/services/protocol.ts` cross-cite
- `ui/package.json` and `ui/package-lock.json` name fields

**Left as-is (historical record):** `design/design-promptforge-ws-1.md` keeps its name and content (dated design log; existing references to it stay valid). Past commit messages and the extraction run's scratch ledger are not rewritten.

## Verification (single gate run, from repo root)

1. `cargo fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --locked --workspace --all-features` (lockfile regenerated and committed first, so `--locked` passes)
4. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
5. `npm run typecheck` and `npm test` in `crates/promptforge-workshop-server/ui`
6. One-command release proof: `cargo build --release -p promptforge-workshop` (exercises the renamed ws-server build script and UI artifact path)
7. Grep sweep: zero remaining `promptforge[-_]ws` matches outside `design/` and git history

## Execution shape

One atomic commit (half-renamed states do not build). Implement, run the gates above, commit with a plain rename message. Precondition: clean worktree.

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: rename_ws_crates_to_workshop

## Origin

This plan was created inside the crate_extraction execution chat, minutes after a fight over the release build gate that the extraction had installed. Step 8 of the extraction made release builds fail unless a prebuilt, verified UI artifact existed, and any debug build wiped `ui/dist/`. The user hit this trying to build the app and exploded (verbatim): "what in the fuck are you crazy? this is a huge pain in the ass! I want to build with 1 command". The assistant changed `build.rs` to produce the artifact on demand (commit e8b8563), and the user's very next request was the rename (verbatim): "I want to rename promptforge-ws to promptforge-workshop and rename promptforge-ws-server to promptforge-workshop-server".

## The why

(paraphrase) "ws" was a stale abbreviation. The codebase had already converged on "workshop" vocabulary everywhere except the crate names: the gateway features were already named `workshop` / `workshop-cuda`, the config file was already `workshop.toml`, the example `workshop.example.toml`. The rename aligned the last holdouts - package names, binary names, and directories - with vocabulary already in use.

(paraphrase) The key safety finding from the assistant's research: nothing functional keys off the old names. Config and data paths are not derived from the crate or binary name (`DEFAULT_CONFIG_PATH` is the hardcoded string `"workshop.toml"`), so the rename is cosmetic for user data - no config paths, tape paths, or user directories move. The only behavioral pins were the `CARGO_PKG_NAME` name-pinning test and the CI `--exclude` pairs.

## Decisions and discarded alternatives

- **Binary names (the one question asked).** The `[[bin]]` names were explicit, so the packages could have been renamed while keeping the shipped executables `promptforge-ws.exe` / `promptforge-ws-server.exe`. That decoupled option was discarded; the user chose to rename the binaries too. Assistant's reasoning (verbatim): "the crate name and binary name staying in sync is the point; the name-pinning test suggests the user cares about the name being deliberate".
- **Historical design doc.** Renaming `design/design-promptforge-ws-1.md` was considered and rejected: it is a dated decision record, renaming is cosmetic churn, and leaving it keeps existing references valid. Judged not worth a user question; stated in the plan as a default open to veto.
- **UI package and CI artifact names.** `promptforge-ws-ui` and `promptforge-ws-ui-dist` were renamed for consistency; judged cosmetic and not worth asking about.
- **One atomic commit vs. staged rename.** Splitting was discarded because half-renamed states do not build; a single commit is the cleanest atomic unit.
- **Adjacent design override (same chat, preceding commit).** The extraction plan's fail-with-instructions release gate was itself overridden by the user's one-command directive - auto-package on demand, fail only when the artifact genuinely cannot be produced. That override is the immediate context and trigger for the rename request.

## Go/no-go gate

Approval was gated on churn. Before authorizing execution the user asked (verbatim): "how much churn is it". The measured answer: ~93 hand-edited lines across 31 files, 228 tracked files moved unchanged via `git mv` (180 of them the `ui/` tree), the diff ~95% pure renames, risk low. Only after that answer did the user say (verbatim): "run".
