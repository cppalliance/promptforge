---
name: promptforge-api firewall
overview: Replace promptforge-api-runtime with a pure re-export facade, package `promptforge` in crates/promptforge/, over private engine crates in crates/promptforge-internal/. The public surface is the minimum hosts need, fully documented by `cargo doc -p promptforge --no-deps` alone, with zero leakage and zero `#[doc(hidden)]`, enforced by user-approved xtask checks.
todos:
  - id: step-1
    content: "Step 1: record the baseline, rename crates/promptforge/ to crates/promptforge-internal/, update build-xtask (ENGINE_CONTAINER, container exception key, family() exact match)"
    status: pending
  - id: step-2
    content: "Step 2: surface audit (non-test vs test-only uses), create the facade package promptforge with auto-trait tests, add TRANSITIONAL_CONTAINER_EXCEPTIONS"
    status: pending
  - id: step-3
    content: "Step 3: migrate every outside crate to the facade; rewrite outside tests that need unjustified items"
    status: pending
  - id: step-4
    content: "Step 4: move the types and runtime crates into the container, move the suite and bench to the facade, delete the transitional exception"
    status: pending
  - id: step-5
    content: "Step 5: drop the types-to-vfs edge, fold shared-vfs into promptforge-vfs, keep its no-dependencies guarantee"
    status: pending
  - id: step-6
    content: "Step 6: facade shape check"
    status: pending
  - id: step-7
    content: "Step 7: cargo xtask api (rustdoc JSON, closure, link targets, doc text, listing) with fixtures"
    status: pending
  - id: step-8
    content: "Step 8: move engine-only operations into detail functions, including ToolId::from_validated"
    status: pending
  - id: step-9
    content: "Step 9: clear all 78 doc(hidden), document the transport codec, gate test helpers, turn on the ban"
    status: pending
  - id: step-10
    content: "Step 10: surface docs name only facade paths (intra-doc links, doc text, doc examples)"
    status: pending
  - id: step-11
    content: "Step 11: minimality pass (non-test uses only), bless public-api.txt, add the CI job"
    status: pending
  - id: step-12
    content: "Step 12: facade lib.md and topic docs, facade docs gate"
    status: pending
  - id: step-13
    content: "Step 13: update AGENTS.md, sweep old names, run the exit gates"
    status: pending
isProject: false
---

# promptforge: pure facade with an absolute firewall

<product-contract>

## Product Requirements

The promptforge repository's engine API is split between `promptforge-api-runtime` and `promptforge-api-types`, and its generated docs are both incomplete and hard to read. This plan replaces the runtime crate with a pure re-export facade, package `promptforge` in `crates/promptforge/`, over private engine crates in `crates/promptforge-internal/`. Visibility is then enforced at the crate boundary, which leaves the facade's module tree free to be organized for readers. The public surface becomes the minimum hosts need, with every item documented by the facade's own docs build, no internal item leaking, and nothing hidden. All paths below are relative to the promptforge repository root.

- Problem and users:
  - Users are host developers reading `cargo doc` output (today the harness and workshop crates, later any outside host) and engine contributors. The user: "I dont want types scattered around different crates. I have discovered it becomes hard to browse the cargo doc that way."
  - `Provenance` (`crates/promptforge-api-types/src/ids.rs` line 208) appears in the public field `Step::Pending::effects` (`crates/promptforge-api-runtime/src/execute/run.rs` line 64) but has no page in the runtime's docs. Its only public path is the whole-crate alias `pub use promptforge_api_types as types;` (`crates/promptforge-api-runtime/src/lib.rs` line 33), and rustdoc renders a whole-crate alias as a single opaque re-export line.
  - The `Provenance` link in that signature depends on build order. Rustdoc links an external crate only if that crate's doc directory already exists when rendering starts, and Cargo doesn't order rustdoc runs between workspace crates under `--no-deps` (https://github.com/rust-lang/cargo/issues/8487, open, needs design). The user: "the type in the signature will still be unlinked".
  - Rustdoc organizes items only by public path, and in Rust a path also controls visibility. The user: "cargo doc shows the physical structure of the crate... Rust's structuring tools are designed to give control over visibility, they do not optimize for legibility by a docs browser." Today's flat root (`crates/promptforge-api-runtime/src/lib.rs` lines 24-33) is an alphabetical list of about 30 items with no grouping by role.
  - The design prose on the private `execute` and `execute::run` modules (`crates/promptforge-api-runtime/src/execute.rs`, `crates/promptforge-api-runtime/src/execute/run.rs` lines 1-28) never renders.
  - Rustdoc has no way to group items by topic independent of paths, on stable or nightly, and no RFC proposes one. Rust nearly got section headers in 2013, but only the unstable `--sort-modules-by-appearance` flag shipped (https://github.com/rust-lang/rust/issues/8552).
- Goals:
  - One public crate, package `promptforge` in `crates/promptforge/`, replacing `promptforge-api-runtime`, so hosts write `promptforge::Run`. The user first named it "the promptforge-api crate (I am renaming it from promptforge-api-runtime)", then shortened it: "can we use "promptforge-impl/" to hold the inner crates and "promptforge/" to hold the facade? would this make the uttered symbols shorter?"
  - As much internal structure as possible. The user: "of course I want as many internal crates as possible and I want maximum internal structure."
  - An absolute firewall. The user: "nothing in the internal crates leaks out except what is re-declared in promptforge-api"; "I want zero leakage"; "absolute privacy"; "keep all the other promptforge crates completely private. no leaks. no #[doc(hidden)], no #cfg(testing), etc". The one `cfg` this plan keeps, the forwarded `test-support` feature, is the open question below.
  - A minimal surface. The user: "I want that surface to be absolutely minimum, and every item in it to be necessary", and "Anything that the user would need to write out or pass in or out of a function would be visible. Nothing would be there which a user would never need."
  - Self-contained docs. The user: "every item in it to be visible in 'cargo doc' with zero additional outside crates required to be built ahead of time (i.e. self-contained)".
  - Exactly one name per item for any consumer. The user: "if we re-export now there are two names for the same thing".
  - Zero public surface growth for tests. No item is exported because a test needs it. The user: "I want ZERO public API surface growth just for tests." The one exception, until the deferred work lands, is the forwarded `test-support` feature, which is off by default and excluded from the surface listing.
  - Zero `#[doc(hidden)]` in the engine and the facade. The user: "I hate #[doc(hidden)] I want it minimized". Once the firewall is in place, the minimum is zero.
- Non-goals:
  - Hiding internal paths from compiler diagnostics, `Debug` output, panic messages, or `std::any::type_name`. No host can name those paths, so this is cosmetic only.
  - Changing engine behavior, run-log or replay formats, or the harness and workshop architecture.
  - Publishing rustdoc anywhere. The Pages workflow deploys only the mdBook guide (`.github/workflows/guide.yml` lines 29-34), and the `documentation` URL in the manifests points at that guide.
- Success criteria:
  - No crate outside `crates/promptforge-internal/` and `crates/promptforge/` depends on any promptforge crate other than `promptforge`, under any dependency kind.
  - `cargo xtask api --check` reports zero closure violations, and its surface listing matches the committed `crates/promptforge/public-api.txt`.
  - `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps` renders a page for every surface item, and every surface type in a signature links to a page inside the facade's docs or to std. That build alone does not prove doc-link integrity, because rustdoc does not report broken links in inlined foreign docs; link integrity is proven by the workspace docs gate together with `cargo xtask api` (see Technical Design).
  - Zero `#[doc(hidden)]` under `crates/promptforge-internal/` and `crates/promptforge/`.
  - Every re-exported item is used by the non-test code of at least one crate outside the engine, is required for the surface to be closed, or belongs to a declared host extension point (the `vfs` module). A use that appears only in an outside crate's tests, test helpers, or doc examples does not justify a re-export.
  - The workspace test count is no lower than the recorded baseline, and every repository gate passes.
- Constraints:
  - Engine crates and the facade have no Cargo features except `test-support`, which is internal to tests. The user: "No feature macros (except maybe testing I guess, but this is internal only)". This matches the repository rule that a feature gates a real constraint, not product shape (`AGENTS.md`, Engineering).
  - The public surface may mention std/core/alloc, serde (with its defining crate serde_core), serde_json, and serde_yaml_ng. This allowlist governs only what the surface's signatures, fields, and impls may mention; internal crates may depend on any external crate the repository's dependency rules allow. The user: "I don't mind depending on serde or whatever. basic rust shit is ok.", and "promptforge internal crates can use a lot of external crates. but the promptforge-api would be restricted to some basic ones".
  - Structural enforcement needs explicit user approval (`AGENTS.md`, Engineering, lines 39-41). The approval is recorded in the Decision Record.
  - Existing repository rules still bind:
    - dependency rules apply to every dependency kind
    - source directories stay flat by default, with kebab sibling files
    - comments state only non-obvious constraints
    - behavior tests are preserved through the refactor
    - JSON round-trip rules apply
    
    All of these are in `AGENTS.md`.
  - Builds stay on the stable toolchain (`rust-toolchain.toml`). Nightly is used only to generate rustdoc JSON for the closure check.
- Open questions:
  - How can the need for `promptforge` to offer testing facilities to outside crates be eliminated, so the facade has no `test-support` feature at all? This does not block this plan, which keeps the forwarded `test-support` feature as the one temporary exception to zero surface growth for tests; the work is deferred (see Deferred and Out of Scope). Today the only outside user is `harness-capabilities`, whose integration tests enable `test-support` as a dev-dependency (`crates/harness/capabilities/Cargo.toml` line 27) and import `Performers` and `drive_tokio` (`crates/harness/capabilities/tests/it/support.rs` lines 14 and 78).

## Functional Specification

Hosts depend on `promptforge` alone and read a module tree grouped by role, where each module's docs are a curated topic page. The dependency rules make the internal crates unreachable, and nothing internal appears in the surface's signatures, impls, or docs. Contributors get checks that fail when the facade's shape, hidden items, the surface's closure, or the surface snapshot drift.

- Actors and workflows:
  - A host developer adds the `promptforge` dependency, imports only `promptforge::` paths, and reads the docs from the `lib.md` table of contents down to module topic pages and then items.
  - An engine contributor uses internal paths freely inside `crates/promptforge-internal/`. To expose something to hosts, they add a single-item `pub use` to the facade. That changes `public-api.txt`, so the change gets reviewed.
  - A transport implementer (today `crates/harness/models/src/transport.rs`) performs the I/O for a model round using `promptforge::transport`.
  - A VFS backend author implements the `Vfs`, `VfsAccess`, and optionally `Policy` traits from `promptforge::vfs`, or reuses `MemoryBackend` and `HostBackend`, and mounts the result with `VfsRef::overlay` or `VfsRefBuilder`. Planned authors: the Workshop, mapping each open window, each running prompt, and each agent's database to a virtual file and serving a `/_promptforge/docs/` directory; and optional harness capabilities, such as one that injects an `AGENTS.md` file. Today only the harness names VFS items (`VfsRef` and `Access` in `crates/harness/runner`, `crates/harness/capabilities`, and `crates/harness/sessions`; `Origin` in the capabilities tests), and no host implements a backend yet.
  - Test code outside the engine that needs the engine's test drivers enables `promptforge`'s `test-support` feature through a dev-dependency. Today that is only the integration tests of `harness-capabilities`.
- Inputs and outputs:
  - The facade's rendered docs: `target/doc/promptforge/`.
  - `crates/promptforge/public-api.txt`: one line per surface item, including the methods, fields, variants, and trait impls of every re-exported type.
  - `cargo xtask api` reads rustdoc JSON for the facade and every internal crate. It reports closure violations and any internal crate name that appears in a surface item's doc text, and it writes the listing. `--check` fails on violations or on any difference from the committed listing. `--bless` rewrites the listing. The listing describes the facade's default build, with `test-support` off, because that is the surface hosts get. The closure check runs twice, once on the default build and once with `test-support` on, so the test drivers cannot expose internal types either.
- States and validation:
  - The surface is every item a host can name from `promptforge`, plus everything those items mention: signatures, fields, enum variants, generic bounds, supertraits, both sides of every trait impl, associated types, and the targets of intra-doc links.
  - Closed: every item the surface mentions is a facade re-export, comes from std/core/alloc, or comes from an allowlisted crate (`serde`, `serde_core`, `serde_json`, `serde_yaml_ng`).
  - Minimal: every re-export is used by the non-test code of a crate outside the engine, is needed to keep the surface closed, or belongs to the `vfs` extension point, which is exported as a unit. Uses only in tests never count. When an outside test needs an item nothing else justifies, the test changes, not the surface. The forwarded `test-support` drivers are the only temporary exception.
  - Self-contained: rustdoc copies every item re-exported by name from another crate into the facade's docs, reading it from compiled crate metadata. The facade's docs build therefore needs the internal crates compiled, as in `cargo check`, but not their docs.
  - Nothing hidden: there is no `#[doc(hidden)]` to make something reachable but undocumented.
- Errors and recovery:
  - Every check failure names the offending item and states required versus actual, following the repository's error style (`AGENTS.md`, Principles).
  - To fix a closure violation, re-export the item from the facade, move the operation into a detail function, or remove the mention from the surface.
  - A snapshot difference is fixed by reviewing it and running `cargo xtask api --bless`.
- Security and privacy behavior:
  - Engine-only operations can't be reached from a host, because detail functions sit in modules the facade never re-exports.
  - Hosts can no longer bypass validation through `ModelId::from_validated` (`crates/promptforge-api-types/src/models.rs` line 73), `CapabilityId::from_validated` (`crates/promptforge-api-types/src/capabilities.rs` line 74), or `ToolId::from_validated` (`crates/promptforge-api-types/src/tools/ids.rs` line 59), which today are public and only hidden from the docs.
  - The transport's credential and body-cap behavior is unchanged.
- Acceptance criteria:
  - Every success criterion in Product Requirements holds.
  - `harness-models` builds against `promptforge::transport` with no hidden items involved.
  - The facade docs show `Provenance`, `Event`, and every other type from the types crate that hosts use, each as its own page under a role module, with working links from `Step::Pending`.
  - The design prose from the private `execute` and `execute::run` module docs appears in the facade's topic docs.

</product-contract>
<implementation-contract>

## Technical Design

The engine becomes a set of private crates under `crates/promptforge-internal/`, with the facade, package `promptforge` in `crates/promptforge/`, at the crates root as their only public face. The existing xtask container-privacy check provides the dependency firewall. The facade holds nothing but single-item re-exports grouped into role modules with curated docs. Operations that only the engine may perform move from inherent methods and public fields on host-visible types into free functions in modules the facade never re-exports, and doc links on surface items are rewritten so the inlined docs stay closed. New xtask checks enforce the facade's shape, the hidden-item ban, and the surface's closure and snapshot.

- Architecture:

  ```mermaid
  flowchart TD
      harness[harness crates] --> api[promptforge]
      workshop[workshop crates] --> api
      api --> engine[engine]
      api --> types[types]
      api --> client
      api --> vfs
      api --> lua
      api --> parser
      api --> store
      engine --> lua
      engine --> parser
      engine --> client[model-client]
      engine --> store
      engine --> vfs
      lua --> types
      lua --> client
      lua --> store
      parser --> lua
      parser --> types
      client --> types
      store --> vfs
  ```

  - The container directory `crates/promptforge/` is renamed to `crates/promptforge-internal/`, and the facade takes `crates/promptforge/`, so each directory is named after what it holds. Inner package names don't change (`promptforge-lua`, `promptforge-parser`, and so on); `-internal` is a directory name only and never appears in code. `build-xtask` finds containers by scanning for directories without a manifest, so the new name is recognized without new logic.
  - `crates/promptforge-api-types` becomes `crates/promptforge-internal/types` (package `promptforge-types`).
  - The code in `crates/promptforge-api-runtime/src` becomes `crates/promptforge-internal/engine` (package `promptforge-engine`).
  - `shared-vfs` is folded into `crates/promptforge-internal/vfs` (package `promptforge-vfs`). It is not a types-only crate: it holds claim and conflict checking, path canonicalization, memory and host backends, and routing (`crates/shared-vfs/src/handle.rs`, `path.rs`, `memory.rs`, `host.rs`, `router.rs`), and it has no dependencies beyond std (`crates/shared-vfs/Cargo.toml` lines 11-15). A test enforces that today: `the_manifest_declares_no_dependencies` (`crates/shared-vfs/src/lib.rs` lines 48-73) fails if any dependency table in the manifest has an entry. `shared-vfs` is also the one member kept off `workspace-hack`, through `[final-excludes] workspace-members = ["shared-vfs"]` in `.config/hakari.toml` (lines 23-26). After the fold, that entry names `promptforge-vfs` instead, the merged crate declares no dependencies at all (it drops `workspace-hack` and `shared-vfs`), and the test moves to `promptforge-vfs` unchanged. A std-only crate gains nothing from `workspace-hack`, since it has no dependencies to unify.
  - The types crate and the vfs crate are independent leaves of the dependency tree, with no edge between them. Today `promptforge-api-types` declares `shared-vfs` (`crates/promptforge-api-types/Cargo.toml` line 18) but uses nothing from it in code; the only references are prose in `crates/promptforge-api-types/src/lib.rs` (lines 26-28) and an intra-doc link to `shared_vfs::Origin` in `crates/promptforge-api-types/src/ids.rs` (lines 191-192), which is why the dependency exists. The doc text is reworded so it no longer links into the vfs crate, and the dependency is removed. The vfs crate gains no dependency on the types crate unless it later needs a types item.
  - The new `crates/promptforge` (package `promptforge`) holds `src/lib.rs`, `src/lib.md`, the integration suite and bench moved from the runtime, and `public-api.txt`.
  - The facade depends directly on every internal crate that defines an item it re-exports, and each `pub use` names the defining crate's path. The diagram shows the edges known today: the engine; the types crate; `promptforge-model-client` for the transport codec and `CompletionError` and `CompletionErrorKind` (`crates/promptforge-api-runtime/src/model.rs` lines 37-40); `promptforge-vfs`; `promptforge-lua` for `StoreOp` and `StoreOutcome` (`crates/promptforge-api-runtime/src/execute.rs` line 126); `promptforge-store` for `StoreError` (line 127); and `promptforge-parser` for `Prompt`, `ParseError`, `ParseErrorKind`, and `promptforge_version` (`crates/promptforge-api-runtime/src/parser.rs` lines 23-27). The runtime's `parser` and `model` modules define nothing of their own; they only re-export. The surface audit adds any other defining crate it finds.
  - A plain merge of the runtime and types crates is impossible. `promptforge-lua`, `promptforge-parser`, and `promptforge-model-client` depend on the types crate (`crates/promptforge/lua/Cargo.toml` line 17, `crates/promptforge/parser/Cargo.toml` line 16, `crates/promptforge/model-client/Cargo.toml` line 19), and the runtime depends on all three (`crates/promptforge-api-runtime/Cargo.toml` lines 16-21), so a merge creates a cycle.
  - Every crate that runs prompts already compiles the whole engine, so depending on the facade adds no weight:
    - `workshop-server` through `workshop-workspace` (`crates/workshop/workspace/Cargo.toml` line 22)
    - `harness-web`, `harness-webfetch`, and `harness-web-search` through `harness-capabilities` (`crates/harness/capabilities/Cargo.toml` line 17)
    
    Of the crates that depend on `promptforge-api-types` without the runtime today, `harness-web`, `harness-webfetch`, `harness-web-search`, and `workshop-server` already compile the engine through another crate. Only three newly compile it: `workshop-protocol` (`crates/workshop/protocol/Cargo.toml` line 12) and `workshop-gateway` (`crates/workshop/gateway/Cargo.toml` line 20), both linked only into `workshop-server`, and `harness-log`, whose use is a dev-dependency (`crates/harness/log/Cargo.toml` line 23), so only its tests pay. The user: "why not pull in the whole engine? You fucking need LUA to run prompts! It cannot be avoided!"
  - The only outside users of `shared-vfs` are harness crates (sessions, capabilities, models, runner, and web as a dev-dependency). All of them already depend on the engine. The user: "harness needs the promptforge engine anyway, duh".
- Modules and interfaces:
  - Facade shape: `crates/promptforge/src/*.rs` contains only these:
    - grouping `pub mod` blocks with doc comments
    - single-item `pub use internal_crate::path::Item;` lines
    - doc attributes
    - `#[cfg(feature = "test-support")]`

    It defines no items, and has no glob re-exports, module re-exports, or crate re-exports.
  - Facade module layout: role modules whose docs are curated topic pages. The proposal, to be adjusted by the surface audit:
    - the crate root: `Prompt`, `Run`, `Step`, `RunContext`, `Environment`, `RunResult`, and the errors those raise, such as `ParseError` and `RunError`
    - modules `effect`, `event` (including `ReplyOrigin`), `ids`, `model`, `transport`, `tools`, `capabilities`, `vfs`, `cancel` (`CancelHandle`), `timestamp` (`Timestamp`), `metrics` (`CallMetrics`, `ClientTiming`, `LlamaTimings`, `ToolCallEvent`, `Usage`, `VllmMetrics`), and `input` (`InputError`, `InputOutcome`)
    - `StreamDelta` is imported today from both `promptforge_api_types::wire` and `promptforge_api_runtime::model`; the audit gives it one home. The parser front-matter items hosts import through the runtime's `parser` module (`Frontmatter`, declarations, roles) also get a home from the audit.
    - Not in the surface, because nothing outside the engine uses them: the `lifecycle` module (hidden today at `crates/promptforge-api-types/src/event.rs` line 57), `MaxToolIterations`, and `LuaProgram`.

    The minimal surface is close to the part of today's API that hosts actually use, across about thirteen role modules; minimality removes what hosts don't use, it doesn't make the API small. Each item has exactly one facade path. There is no `error` module: each error type is re-exported in the module of the role that raises it, next to that role's types, for example `StoreError` in `vfs` and `ClientError` in `transport`.
  - `transport`: the sans-IO model-round codec becomes documented public API. It covers `ChunkSource`, `build_request_body`, `read_body_capped`, `read_completion_stream`, `escape_controls`, `ClientError`, and `ClientTimeout`. Today these are hidden re-exports (`crates/promptforge-api-runtime/src/model.rs` lines 29-42) that `crates/harness/models/src/transport.rs` lines 14-17 imports.
  - Detail modules: any operation on a host-visible type that only the engine may perform becomes a free function in a `detail` module inside the defining internal crate, for example `promptforge_model_client::detail::message_from_validated_parts`. The facade never re-exports that module. Fields hosts must not touch become private, reached through detail functions. Host-facing accessors such as `ToolCall::id`, `ToolCall::name`, and `ToolCall::arguments` stay.
  - Traits hosts call but mustn't implement: use concrete types or enums instead. If a trait is unavoidable, seal it with a supertrait from a module that isn't re-exported. That bound renders as an unlinked name, which is the one visible trace this design allows.
  - `vfs`: a host extension point, exported as a unit rather than trimmed to today's uses. Hosts implement its traits, so `Vfs`, `VfsAccess`, and `Policy` are open traits and exempt from the sealing rule above. The module carries the contract that closure already requires (`VfsRef`, `Access`, `Vfs`, `VfsAccess`, `Policy`, `AllowAll`, `VfsRefBuilder`, `Origin`, `ExecId`, `VfsError`, `Entry`, `FileType`, `Stat`, `GrepQuery`, `GrepResults`, `GrepMatch`, `Op`, `Verdict`, `VfsPath`, `VfsPathBuf`) plus the built-in backends and the operation observer that no signature mentions (`MemoryBackend`, `HostBackend`, `OpEvent`, `OpSink`), all from `crates/shared-vfs/src/lib.rs` lines 21-30. The audit decides the `promptforge-vfs` items (`STORE_MOUNT`, `empty`, `Mode`, `ModePolicy`, `ModeHandle`, `crates/promptforge/vfs/src/lib.rs` lines 15-53) by whether a host assembles a run's VFS itself or gates its own mounts by mode. Engine-only operations on `Access`, such as `spawn` and `id`, move to `detail` functions if the audit finds no host use. The `vfs` topic doc teaches mounting with `overlay` and `VfsRefBuilder`, implementing `VfsAccess` and which of its methods have default bodies, what `Policy` gates, and read-only mounts.
  - Extension traits evolve compatibly: a method added to `Vfs`, `VfsAccess`, or `Policy` after this plan lands must have a default body, because every host backend implements them. `shared-vfs` already follows this pattern (`crates/shared-vfs/src/traits.rs`: `read_only`, `read_range`, `str_replace`, and `grep` have defaults).
  - `test-support`: the engine's test drivers (`crates/promptforge-api-runtime/src/lib.rs` lines 13-14, `crates/promptforge-api-runtime/Cargo.toml` lines 33-40) sit behind the engine's `test-support` feature. The facade declares `test-support = ["promptforge-engine/test-support"]` and re-exports the drivers under `#[cfg(feature = "test-support")]`. Engine-internal test helpers use the same gate.
  - Doc examples in internal crates use `promptforge::` paths, so the rendered facade docs never show internal paths. The facade's own doctest run does not execute the examples of items it re-exports; only the defining crate's doctest run does, and those doctests compile against `promptforge` through a dev-dependency. That is the sole reason engine crates take `promptforge` as a dev-dependency.
  - Intra-doc links on surface items are rewritten, not re-pointed at the facade. Rustdoc inlines a re-exported item's docs into the facade with each link resolved in the defining crate: a link whose target the facade also re-exports is retargeted to the facade's page; a link whose target is not re-exported renders as a dead link; a link that doesn't resolve renders as literal text; and the facade's `--no-deps` docs build reports none of this. Engine crates also can't write `promptforge::` links, because a library's own docs build does not see its dev-dependencies. So every intra-doc link in a surface item's docs must either target an item the facade re-exports, or be reworded as plain text. Re-exporting an item only to satisfy a link is not allowed, because it grows the surface for a doc reason. Today there are about 21 `crate::`, `super::`, or `self::` links on items the runtime exports at its root (about 28 counting `test_support`), including about 11 `super::Environment` links in `crates/promptforge-api-runtime/src/execute/config.rs` (lines 22-301) and two in `crates/promptforge-api-runtime/src/execute/error.rs` (lines 72-73), and about 12 in the types crate, including `crates/promptforge-api-types/src/replay.rs` line 98, `crates/promptforge-api-types/src/tools/descriptor.rs` line 16, and `crates/promptforge-api-types/src/tools/ids.rs` line 178. Links on the runtime's crate-private `Error` (`crates/promptforge-api-runtime/src/error.rs` lines 4-5, 210, 263) are not on the surface and need no change.
  - Link integrity is proven by two gates together, not by the facade's docs build: the workspace docs gate builds every internal crate with warnings denied, so a link that doesn't resolve fails in its defining crate; and `cargo xtask api` reads each surface item's resolved links from rustdoc JSON (its `links` table) and reports any target the facade doesn't re-export.
- File and public API changes:
  - `crates/build-xtask/src/product.rs`:
    - `PUBLIC_PROMPTFORGE` (line 126, declared `[&str; 2]`) ends as `["promptforge"]`. While `promptforge-api-runtime` and `promptforge-api-types` still sit at the crates root, before the move into the container, it lists them beside `promptforge`, so its declared type changes with its length
    - `container_named_exception` (lines 244-250) names one crate per container (`"promptforge" => Some("promptforge-api-runtime")`) and stays single-valued: it maps the `promptforge-internal` container to `promptforge`. Between creating the facade and moving the runtime into the container, the old runtime (which depends on lua, the parser, and model-client) also depends on container crates, so a separate, clearly temporary constant `TRANSITIONAL_CONTAINER_EXCEPTIONS` allows `promptforge-api-runtime` into `promptforge-internal` for that window. The constant and its handling are deleted in the move, which restores the permanent rule that exactly one named outside crate may enter a container
    - `family()` (around lines 54-69) assigns families by package-name prefix, and today matches `"promptforge-"` only. It gains an exact match for `"promptforge"`, as it already has for `gateway` and `workshop`; without it the facade would be unaffiliated, the way a bare `harness` package is today
    - the module docs (lines 18-27), which cite `crates/promptforge/`, and the `crates/build-xtask/src/product-tests.rs` and `crates/build-xtask/src/product-container-tests.rs` fixtures follow
    
    The check already covers normal, dev, build, and target-specific dependencies (line 38) and resolves `package` renames (lines 376-385). Crates named `build-*` stay exempt (lines 154-156).
  - `crates/build-xtask/src/engine_guards.rs`: `ENGINE_ROOT_CRATES` (line 16, declared `[&str; 2]`) ends naming only `promptforge`, listing the two old root crates beside it until they move (so its declared type changes with its length), and `ENGINE_CONTAINER` (around line 19) becomes `"promptforge-internal"`. The leak guard's exemption for a root crate forwarding its sibling's `test-support` feature (`crates/build-xtask/src/test_support_leak.rs` line 114) covers the facade once it is listed in `ENGINE_ROOT_CRATES`. Crates inside the container are discovered automatically. The fixtures in `crates/build-xtask/src/engine_guards-tests.rs` and the test-support leak guard fixtures (`crates/build-xtask/src/test_support_leak-tests.rs`) follow.
  - Nothing in `build-xtask` requires a directory name to equal its package name, so `crates/promptforge/` holding package `promptforge` needs no special handling.
  - The workspace `Cargo.toml` members and `[workspace.dependencies]` follow the moves, and workspace-hack is regenerated with `cargo hakari generate`.
  - Every outside manifest and every `use` moves from `promptforge-api-runtime` or `promptforge-api-types` to `promptforge`. That's about 60 source files across harness and workshop crates, plus `crates/harness/web/Cargo.toml`'s `shared-vfs` dev-dependency.
  - Today's `pub use promptforge_api_types as types;` and the rule it states (`crates/promptforge-api-runtime/src/lib.md` line 3, "re-exported here as [`types`]") are removed. Nothing in code uses the `types::` path. Outside crates import `promptforge_api_types` directly, and the only uses of `types::` are one doc example in `lib.md` and the README.
  - `#[doc(hidden)]` inventory to clear, 78 attributes in all:
    - promptforge-lua (30): `crates/promptforge/lua/src/lib.rs` (18, lines 58-160), `error-value.rs` (4), `vm.rs` (3), `error.rs` (3), `handles.rs` (2)
    - promptforge-model-client (38): `crates/promptforge/model-client/src/client/wire.rs` (22), `client/stream.rs` (4, lines 41, 82, 117, 450), `client.rs` (4, lines 30, 32, 34, 36), `client/read.rs` (3), `error.rs` (3), `client/request.rs` (1), `lib.rs` (1, line 38)
    - promptforge-parser (2): `crates/promptforge/parser/src/error.rs`
    - promptforge-store (2): `crates/promptforge/store/src/error.rs` (lines 300, 314); the store crate's `StoreError` is on the surface
    - promptforge-api-types (4): `crates/promptforge-api-types/src/event.rs` (line 57, the `lifecycle` module), `models.rs` (line 72), `capabilities.rs` (line 72), `tools/ids.rs` (line 59, `ToolId::from_validated`)
    - promptforge-api-runtime (2): `crates/promptforge-api-runtime/src/model.rs` (lines 29, 41)
  - Detail targets already known:
    - `Message::from_validated_parts`, `assistant_tool_calls`, `content_value`, and `raw_tool_calls`
    - `ToolSchema::new`
    - the public fields of `ToolCall` and of the completion result (`result` through `response_body`)
    
    All of these are in `crates/promptforge/model-client/src/client/wire.rs`. Also `ModelId::from_validated`, `CapabilityId::from_validated`, `ToolId::from_validated` (`crates/promptforge-api-types/src/tools/ids.rs` line 59), `Error::http` in `crates/promptforge/model-client/src/error.rs`, and `SharedSource::new` in `crates/promptforge/lua/src/error.rs`.
  - Test helpers to gate: `for_test` (`crates/promptforge/lua/src/handles.rs` lines 85, 163), and `run_chunk`, `tool_bag_handles`, and `model_bag_handles` (`crates/promptforge/lua/src/vm.rs` lines 755, 900, 921).
  - Trait impls to check with the closure check: `impl From<Error> for RunError` and its reverse (`crates/promptforge-api-runtime/src/execute/error.rs` lines 209, 215), where `Error` is crate-private (`crates/promptforge-api-runtime/src/lib.rs` line 18). If the closure check flags them, convert them to detail functions.
  - `crates/build-xtask/Cargo.toml` gains `syn`, `rustdoc-types`, and `serde_json`. Today it depends on `anyhow`, `toml`, and `workspace-hack`, with `tempfile` as a dev-dependency (lines 9-15).
  - Every file in `build-xtask` stays under 500 lines, per the invariants in `crates/build-xtask/src/main.rs` (lines 3-8), so the facade shape check, the `doc(hidden)` ban, and `cargo xtask api` (rustdoc JSON loading, cross-crate item matching, closure, link targets, and the listing) are split across files from the start.
- Data, persistence, failure, security, and privacy constraints:
  - Run-log, replay, and wire JSON formats don't change. Serde derives on public types remain part of the surface, as they are today.
  - Trait impls of traits anyone can name (`Clone`, `Debug`, `Display`, `Error`, `Serialize`, `Deserialize`, `From`) on public types are surface by nature. Keep them only where they're needed.
  - Auto traits (`Send`, `Sync`, `Unpin`, `UnwindSafe`) follow from private fields. The facade's tests pin the promised ones with compile-time assertions. `Run` is documented as `Send` (`crates/promptforge-api-runtime/src/execute/run.rs` lines 82-84).
  - The closure check runs on a pinned nightly. The nightly date and the matching `rustdoc-types` version live together in one constant in build-xtask. The CI job installs exactly that nightly, and `cargo xtask api` run on any other toolchain fails at once with a message naming the required nightly. The constant lives in `crates/build-xtask/src/api/toolchain.rs`; wherever a step says `cargo +<pinned nightly> xtask api`, it means the nightly named there.

</implementation-contract>
<verification-contract>

## Testing Plan

The refactor must not change behavior: existing behavior tests stay unchanged, and the runtime's integration suite moves to the facade so it runs only through the public surface. Each new xtask check gets fixture tests in the style of the existing product-boundary tests. The exit gates are the repository's full gate set, plus the closure and snapshot check and a docs build of the facade on its own. Doc-link integrity rests on the workspace docs gate and the closure check together, never on the facade's docs build alone.

- Unit:
  - Fixture tests for the facade shape check. They accept grouping `pub mod`, single-item `pub use`, doc attributes, and the `test-support` cfg. They reject globs, module re-exports, crate re-exports, and item definitions.
  - Fixture tests for the `doc(hidden)` ban, covering items, fields, methods, and re-exports.
  - Fixture tests for `cargo xtask api`, using small generated workspaces. They must cover:
    - a type in a signature that isn't re-exported
    - an internal trait in a bound or supertrait
    - a trait impl that mentions an internal type
    - an intra-doc link to an internal item, including a link written as `crate::` or `super::` in the defining crate whose target the facade doesn't re-export
    - an intra-doc link whose target the facade does re-export, which passes
    - an internal crate name in doc text
    - running on a toolchain other than the pinned nightly, which fails with a message naming the required nightly
    - a type from an allowlisted crate
    - a snapshot difference
  - Updated `crates/build-xtask/src/product-tests.rs` cases for the single public crate `promptforge`, the `promptforge-internal` container exception, the temporary `TRANSITIONAL_CONTAINER_EXCEPTIONS` entry while it exists, and a `family()` case showing that the package `promptforge` belongs to the PromptForge family.
  - `the_manifest_declares_no_dependencies`, moved unchanged from `shared-vfs` to `promptforge-vfs`, passes on the merged crate's manifest, which declares no dependencies, and `cargo hakari verify` passes with `promptforge-vfs` in `[final-excludes]`.
  - Compile-time assertions in the facade's tests for the auto traits the API promises.
- Integration and end-to-end:
  - The runtime's integration suite (`crates/promptforge-api-runtime/Cargo.toml` lines 61-63, `required-features = ["test-support"]`) and the `models_loop` bench (lines 65-68) move to `crates/promptforge` unchanged, so they compile against `promptforge::` paths only.
  - The harness and workshop suites pass unchanged, apart from import paths.
  - Doctests pass in every internal crate, with examples written against `promptforge::` paths. These runs are what validate re-exported items' examples; the facade's own doctest run executes none of them.
- Regression, security, and performance:
  - Every behavior test is preserved, and the workspace test count is no lower than the baseline.
  - Detail conversions keep host-visible accessor behavior identical.
  - No change to run-log JSON round-trip tests.
- Exit criteria:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - both clippy partitions with `-D warnings`
  - `cargo fmt --all --check`
  - `cargo check -p gateway --no-default-features`
  - the workspace docs gate
  - `mdbook build guide`
  - `cargo test -p build-xtask`
  
  These commands come from `AGENTS.md`, Verification (lines 49-55). The plan adds:
  - `cargo deny check`, which CI already runs (`.github/workflows/ci.yml` line 341) and which covers the three dependencies `build-xtask` gains
  - `cargo hakari verify`, because workspace-hack is regenerated after the moves; neither `AGENTS.md` nor CI runs it today
  - `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`
  - `cargo xtask api --check` on the pinned nightly, locally and in a new job in `.github/workflows/ci.yml`, comparing the listing against the default build and running the closure check on both the default build and the `test-support` build

</verification-contract>
<decision-record>

## Decision Record

The design is a pure facade, package `promptforge`, over private crates in `crates/promptforge-internal/`: visibility is enforced by the existing container check, so the facade's modules can be organized for readers. The facade has one name per item, no hidden items, and the smallest surface hosts need, except the VFS, which is a host extension point exported whole. Engine-only operations move to `detail` functions, and new checks prove the surface closed and snapshot it. The names were chosen for short host paths and a tree that is easy to browse, and a reviewer's point about dependency direction made the types and vfs crates independent leaves.

- Decisions:
  - **Pure facade over internal crates.** The facade, package `promptforge`, re-exports and defines nothing, and all code lives in `crates/promptforge-internal/`. Rationale: visibility is enforced at the crate level by the existing container check, so the facade's module paths can serve only how the docs read. This is the only design discussed that separates what can see an item from how the docs are organized. Precedent: bevy over `bevy_internal` (https://docs.rs/bevy_internal), clap over `clap_builder` (https://docs.rs/clap_builder). The user proposed it: "what if we move the types into interior crates and have them depend on that and then promptforge-api-runtime becomes a PURE re-exports crate".
  - **The facade's package is named `promptforge`, not `promptforge-api`.** Rationale: only the package name appears in Rust paths, so hosts write `promptforge::Run` rather than `promptforge_api::Run`, and the facade reads as the product. The user first planned "renaming it from promptforge-api-runtime" to `promptforge-api`, then asked "would this make the uttered symbols shorter?" and adopted the shorter name.
  - **The facade lives in `crates/promptforge/` and the container moves to `crates/promptforge-internal/`.** Rationale: a directory named after the package it holds is easier for a person browsing the tree. Offered this layout (option A) against keeping the directories and naming only the package `promptforge` (option B), the user said: "A is nicer to browse by a human". `-internal` follows the one real precedent for code behind a facade, `bevy_internal` behind `bevy` (https://docs.rs/bevy_internal). The user chose "crates/promptforge-internal/".
  - **The name `promptforge` only has to be unique inside this workspace.** Every crate sets `publish = false` (for example `crates/promptforge-api-runtime/Cargo.toml` line 7), and the user does not publish to crates.io, where the name `promptforge` is already taken. Nobody should rename the facade to avoid that collision. The user: "someone already took promptforge on crates.io. And I am boycotting crates.io", and, on recording this: "sure, why not".
  - **Internal crate names `promptforge-types` and `promptforge-engine`.** These were proposed during planning, and the user didn't object.
  - **Maximum internal structure.** Internal crates are kept and split further when useful, never merged for convenience. Rationale: crate boundaries are what stop, for example, `promptforge-lua` from reaching into the scheduler. The user: "of course I want as many internal crates as possible and I want maximum internal structure."
  - **Every consumer depends on the full facade, with no engine feature flag.** Rationale: every crate that runs prompts already compiles the engine; of the three that newly would, `workshop-protocol` and `workshop-gateway` are linked only into `workshop-server`, and `harness-log` pays only in its tests (see Technical Design). The user: "why not pull in the whole engine? You fucking need LUA to run prompts!"
  - **No Cargo features except internal `test-support`.** The user: "No feature macros (except maybe testing I guess, but this is internal only)".
  - **Tests never justify public API, starting with this plan.** Minimality counts only uses in outside crates' non-test code; an outside test that needs something else is rewritten. The forwarded `test-support` feature stays until the deferred work removes it, off by default and excluded from the listing. The user: "I want ZERO public API surface growth just for tests." Asked whether that replaced the earlier deferral of `test-support` ("no. we will deal with it later."), the user chose: "Rule now, feature later: minimality counts only non-test uses starting now, but the forwarded test-support feature stays until the deferred work lands".
  - **One name per item, and single-item re-exports only.** Rationale: a module or crate re-export exposes everything added to that module later, and a second path to an item makes the API ambiguous. The user: "if we re-export now there are two names for the same thing".
  - **Role-grouped facade modules whose docs are curated topic pages, with a curated table of contents in `lib.md`.** Rationale: rustdoc has no grouping independent of paths. The only equivalents are hand-written crate docs with intra-doc links, as in wasmtime's "Core Concepts" (https://docs.rs/wasmtime/latest/wasmtime/), and module docs. The facade's `pub use` list plays the role of Haddock's export list, where Haskell allows section headings.
  - **The surface listing describes the default build; the closure check covers both builds.** Rationale: the `test-support` feature changes what the facade exports, so the listing must name one build to be reproducible, and the host surface is the default build; a second closure run keeps the test drivers from exposing internal types. The user: "This sounds reasonable".
  - **The `vfs` module is a host extension point, exported as a unit with its built-in backends.** Rationale: hosts will implement the VFS traits and mount their own backends, so the traits stay open, and trimming the module to today's three used items would strip the very contract hosts need. The user, on the Workshop: "Workshop will want to make heavy use of adding virtual directories mapping to various things like mapping each open window to a file in the vfs, mapping each running prompt into a file in the vfs, running each sqlite database which represents an agent into a file in the vfs,. there will be a /_promptforge/docs/ virtual directory which the model can read"; on the harness: "harness will have optional capabilities that will want to do some or all of those things too. Such as injecting AGENTS.md. That requires vfs manipulation"; "hosts have to implement the traits"; and "promptforge-api has to include the vfs types and functions, because hosts will need them", because "without the vfs, a host can't get data in or out of the prompt". The user's example: a Papergate prompt reads its input with `var.paper = untrusted(store.read("paper.md"))`, where `paper.md` exists only as a virtual file the host provides, which keeps the prompt sandboxed.
  - **The types crate and the vfs crate are independent leaves.** Rationale: the types crate's dependency on the vfs crate exists only to resolve one doc link, and the vfs crate needs nothing from the types crate, so neither depends on the other and the types sit at the foundation of the tree. The facade keeps its direct edges to both. This adopts the point of Marcel, an outside reviewer of this plan, that "the types are the foundation and vfs depends on them", adjusted to drop the edge rather than reverse it, since the vfs crate needs nothing from the types. The user: "adopt Marcel's adjusted changes".
  - **Errors live beside the roles that raise them, with no `error` module.** Rationale: a reader on a role's page finds its error there, and the paths are shorter, such as `promptforge::RunError`. The user: "the error change is good. shorter path and this makes sense".
  - **Zero `#[doc(hidden)]`.** Rationale: with the firewall, internal items are unreachable anyway, and hiding them from the engine's own docs only hurts contributors. Engine-only operations on public types go through detail functions, and test helpers through `test-support`. The user: "I hate #[doc(hidden)] I want it minimized".
  - **Detail free functions for engine-only operations.** Rationale: free functions in a module that isn't re-exported leave no trace on the type's docs page, while a hidden trait implemented on a public type can render as an impl linking into an internal crate. The user asked for "whatever you said the sealed/hidden traits, whatever. absolute privacy".
  - **The modules holding them are named `detail`.** Rationale: the Boost and C++ convention for implementation details, which names what the functions are. "Seam" was rejected because in Michael Feathers' established sense (*Working Effectively with Legacy Code*, 2004) it means a substitution point for changing behavior without editing code, which these are not; `internal` would be redundant inside internal crates; the `__private` spelling with `#[doc(hidden)]` exists only for modules in public crates. The user: "it seems reasonable and the blast radius is limited to internal".
  - **The transport codec becomes documented public API in `promptforge::transport`.** Rationale: the engine performs no I/O, and the harness is its host and performs the round, so the pure request-building and stream-reading functions are real host API. The user chose "documented".
  - **`shared-vfs` moves inside the container and is folded into `promptforge-vfs`.** Rationale: only harness crates use it outside the engine, and all of them depend on the engine, so its types become facade items with their own pages. Having both `shared-vfs` and `promptforge-vfs` inside the engine would be confusing. The user: "shared-vfs is also ok although we might move that to the runtime", then "harness needs the promptforge engine anyway, duh", and chose "fold"; later: "promptforge can't be used without it, so whats the point of keeping it separate. it just makes cargo doc produce worse results".
  - **Third-party allowlist for the public surface: std/core/alloc, `serde`, `serde_core`, `serde_json`, `serde_yaml_ng`.** Adding a crate is an explicit, reviewed change to one constant. The allowlist restricts what the surface mentions, not what internal crates depend on. The user: "I don't mind depending on serde or whatever. basic rust shit is ok.", and "promptforge internal crates can use a lot of external crates. but the promptforge-api would be restricted to some basic ones". `serde_core` and `serde_yaml_ng` were added during Step 7, when rustdoc reported serde's traits under their defining crate `serde_core` and the surface exposed `serde_yaml_ng::Value`. The user, on 2026-09-24: "serde_yaml_ng sounds fine and so does serde_core, these are foundational libs".
  - **Structural enforcement approved.** Approved: the facade shape check (a source parser), the closure check on rustdoc JSON with the crate allowlist, the public surface snapshot, and the `doc(hidden)` ban in `crates/promptforge-internal/` and `crates/promptforge/`. The user, on 2026-09-23: "everything you recommend, based on the constrains and direcetion I want to go implied in our chat".
  - **Doc examples in internal crates use `promptforge::` paths.** Rationale: rendered facade docs must show only public paths.
  - **Intra-doc links on surface items target facade re-exports or become plain text, and no item is re-exported just to satisfy a link.** Rationale: rustdoc resolves an inlined item's links in its defining crate and silently renders unresolved or unexported targets as literal text or dead links, and engine crates can't link through the facade because a library's docs build has no dev-dependencies; re-exporting targets for docs would grow the surface. The rewrite is its own work item. This and the other corrections below came from an outside review of the plan that tested the rustdoc behavior directly; the user chose "Consolidate everything as evaluated above: accept all findings with corrected counts, add a doc-link work item, and decline the rendered-HTML assertion with the two-gate rationale".
  - **The container exception stays single-valued; the transition uses a temporary constant.** `TRANSITIONAL_CONTAINER_EXCEPTIONS` admits `promptforge-api-runtime` into `promptforge-internal` only while it still sits at the crates root, and is deleted in the move. Rationale: making `container_named_exception` set-valued would permanently loosen the rule that one named outside crate may enter a container, for a reason that lasts one step.
  - **The VFS keeps its no-dependencies guarantee after the fold, exactly.** The `[final-excludes]` entry in `.config/hakari.toml` moves from `shared-vfs` to `promptforge-vfs`, the merged crate declares no dependencies, and `the_manifest_declares_no_dependencies` moves unchanged. Rationale: the fold shouldn't silently drop an enforced property of the crate, and the repository already uses `[final-excludes]` for exactly this, at no cost for a crate with nothing to unify. This replaces an earlier version that admitted `workspace-hack`. The user chose: "Move the [final-excludes] entry from shared-vfs to promptforge-vfs: the merged crate declares no dependencies at all, and the_manifest_declares_no_dependencies moves unchanged".
  - **Design prose on private modules moves into the facade's topic docs.** Rationale: prose on private modules never renders, and the `execute` and `execute::run` module docs are some of the crate's best explanations.
  - **The facade's own integration suite never justifies a re-export.** The runtime's `suite` tests and `models_loop` bench move to `crates/promptforge` and compile against `promptforge::` paths only. A test there that needs an item outside the host surface moves into the engine's unit tests instead of adding a re-export. Rationale: the suite moves to prove the public surface works, and a re-export added for a test grows a surface that is hard to shrink later, because removing a public item breaks hosts. Added during step decomposition, and confirmed by the broader rule that tests never justify public API (see that decision above).
- Rejected alternatives:
  - **Merging `promptforge-api-runtime` and `promptforge-api-types` into one crate.** Rejected because the merge forms a dependency cycle through lua, parser, and model-client. Revisit only if the whole engine is merged into one crate, which conflicts with maximum internal structure.
  - **Keeping the container at `crates/promptforge/` and putting the facade in `crates/promptforge-api/` with package name `promptforge`.** This moves no directories and mirrors the `harness/` and `harness-api/` pair, and the repository already has directories named differently from their packages (`crates/workshop/shell` is package `workshop` and `crates/gateway/app` is package `gateway`, per the project survey in `vibe/2026-09-18-2-remove-tool-picker.md`). Rejected by the user in favor of a directory named after its package, which reads better when browsing. No revisit.
  - **Naming the container `crates/promptforge-impl/`.** Rejected because in Rust a crate suffixed `-impl` usually means the proc-macro backend behind a crate (for example `thiserror-impl`, https://docs.rs/thiserror-impl). No revisit.
  - **Re-exporting `Provenance` at the root next to the `types` alias.** Rejected because it creates two names. No revisit.
  - **`#[doc(inline)] pub use promptforge_api_types as types;`.** Rustdoc does support inlining a whole crate this way (https://github.com/rust-lang/rust/pull/55804), and it would fix the `Provenance` page with one line. Rejected because a crate re-export exposes everything the crate gains later, and it makes minimality impossible. Revisit never; the facade replaces it.
  - **Docs-only fixes, such as linking `types::ids::Provenance` from prose.** Rejected because the signature link still depends on build order. No revisit.
  - **An `engine` feature on the facade so lightweight consumers skip the engine.** Rejected by the user; the weight is already paid (see Decisions). Revisit if a crate outside the engine that never runs prompts, and isn't linked into a binary that does, needs the vocabulary.
  - **Preludes, and grouping modules that re-export items already exported elsewhere.** Rejected because they create second paths. No revisit.
  - **Reversing the edge so the vfs crate depends on the types crate.** Rejected because the vfs crate uses no types item; an edge with no use is dead weight in either direction. Revisit if the vfs crate needs a types item.
  - **Reaching the types through the vfs crate instead of a direct facade edge.** Rejected because the facade may depend directly on any number of internal crates without becoming more than one public crate, and routing through an intermediate crate adds second paths inside it (see routing re-exports through the engine). No revisit.
  - **A single `error` module holding every error type.** Rejected because it separates each error from the role that raises it and lengthens its path. No revisit.
  - **Routing re-exports through the engine** (the engine `pub use`s items from `model-client`, `promptforge-vfs`, and other crates, and the facade re-exports them from the engine, so the facade depends only on the engine and the types crate). Rejected because it adds a second path for each such item inside the engine, and makes the closure check resolve re-export chains instead of matching each item to its defining crate in one step. Direct edges add no build weight, since the engine already compiles those crates. No revisit.
  - **Hidden or detail traits implemented on public types.** Rejected in favor of free functions (see Decisions). Revisit if a detail operation needs dynamic dispatch.
  - **`cargo-public-api` for the snapshot.** Rejected because it doesn't list the items inside cross-crate re-exports (https://github.com/cargo-public-api/cargo-public-api/issues/103). The xtask builds the listing from combined rustdoc JSON instead. Revisit if that issue is fixed.
  - **A permanent necessity check** (an import walker over outside crates). Rejected; the one-time audit sets the minimal surface, and the snapshot makes every later addition a reviewed change. Revisit if the surface grows without review.
  - **An assertion over the facade's rendered HTML** (no literal `[crate::` or `[super::`, no `href="crate::`). Rejected as redundant: a link that doesn't resolve already fails the workspace docs gate in its defining crate, and a link that resolves to an item the facade doesn't re-export is reported by `cargo xtask api` from rustdoc JSON's `links` table. Revisit if a broken link ever reaches the facade's HTML past both gates.
  - **Re-exporting a doc link's target so the link works in the facade.** Rejected because it grows the surface for a documentation reason and conflicts with minimality; the link is reworded instead. No revisit.
  - **Making `container_named_exception` set-valued for the transition.** Rejected in favor of a temporary constant, because a set would permanently weaken the one-outside-crate rule for a one-step need. No revisit.
  - **Alternative doc generators** (sphinxcontrib-rust, mdBook plugins, custom renderers). Rejected because the facade layout meets the goal with rustdoc alone. Revisit if rustdoc output still reads poorly after the facade lands.
  - **A separate internal docs build with `--document-private-items`.** Not needed, because the design prose moves into public topic docs. Revisit if contributors ask for internal API docs.
- Assumptions, risks, and notes:
  - Assumption: rustdoc copies named re-exports of items from other crates into the re-exporting crate's docs by default, with pages built from compiled metadata. This is why `StoreOp` and `StoreError`, which are re-exported by name from other crates, render today (`crates/promptforge-api-runtime/src/execute.rs` lines 126-127).
  - Risk (medium): engine crates taking `promptforge` as a dev-dependency forms a cycle. Cargo allows it, but unit tests compile the engine twice, and only doctests may use the facade. The dependency exists only so the defining crates' doctests can compile examples written against `promptforge`, since the facade's doctest run never executes re-exported items' examples. Fallback if it proves unworkable: move those examples into the facade's module docs, where they compile against `promptforge`, and keep internal item docs free of examples.
  - Assumption: rustdoc JSON records each item's resolved intra-doc links (its `links` table), which is what lets `cargo xtask api` check link targets. An outside review confirmed experimentally that a re-exported target is retargeted to the facade page and an unexported one renders as a dead link.
  - Risk (medium): the doc-link rewrite touches about 40 links across the runtime and types crates (see Technical Design), and each needs a judgment call between retargeting to a facade re-export and rewording as plain text.
  - Risk (medium): rustdoc JSON is nightly-only, and its format changes roughly every release (https://github.com/rust-lang/rust/issues/76578). Updating the pinned nightly means updating `rustdoc-types` at the same time.
  - Risk: one crate's rustdoc JSON doesn't include the items it re-exports from another crate, so the xtask has to build JSON for every internal crate and match items across crates by crate and path. Comparing path strings alone isn't enough, because rustdoc reports the original internal path for re-exported items.
  - Risk: the migration touches about 60 source files in outside crates plus every engine manifest. The tree must build after each change, and keeping the old crates in place while consumers move to the facade keeps the physical move separate from the consumer migration.
  - Risk: the firewall covers workspace members only. `build-xtask` walks this workspace's crates, and Cargo places no limit on path dependencies, so a crate in another repository could still path-depend on a crate inside `crates/promptforge-internal/`. The one known outside consumer, Papergate, is to depend on `harness-api` alone (see Deferred and Out of Scope), which keeps it behind the harness's public surface.
  - Note: things that can't be firewalled are trait impls of traits anyone can name, auto traits, and internal paths in diagnostics and `Debug` output. The first two are handled by keeping impls deliberate and pinning auto traits; the last is cosmetic.
  - Risk: the `vfs` traits are a compatibility commitment. Once hosts implement them, a required method added later breaks every host backend, so additions need default bodies (see Technical Design), and removals or signature changes are breaking changes to the facade.
  - Note: the `vfs` module is the one place where the facade exports items no outside crate uses today (`MemoryBackend`, `HostBackend`, `OpEvent`, `OpSink`). The minimality criterion admits them as a declared extension point; the snapshot still makes each a reviewed line.
  - Note: source citations such as `crates/promptforge/lua/src/lib.rs` or `crates/promptforge/vfs/src/lib.rs` give each file's location today. Once the container is renamed, the same files live under `crates/promptforge-internal/`, and `crates/promptforge/` holds only the facade.
  - Note: while the internal crates still render in the workspace docs, their items appear twice across the crate list. That's acceptable for contributors.
  - Note: rustdoc gives headings in crate and module docs a sidebar table of contents (https://github.com/rust-lang/rust/pull/120736), so the topic docs are navigable.

### Deferred and Out of Scope

- Deferred: normalizing the harness and gateway families. The user: "I plan to do this to gateway and harness. Not now. But I will normalize the repo and then xtask will be consistent for all crates". Families come in two kinds, with different shapes; the user: "promptforge-internal/ and promptforge/, and harness-internal/ and harness/ are the right shapes for those species, the executables are a different story. I dont think xtask can have a single rule."
  - Library families (PromptForge and harness): a public crate named after the family at the crates root, wrapping a private `<family>-internal/` container. The harness becomes `crates/harness/` over `crates/harness-internal/`, replacing `harness-api`. This plan's `build-xtask` changes (`PUBLIC_PROMPTFORGE`, the `promptforge-internal` container exception, the exact `"promptforge"` match in `family()`) are the first instance.
  - Application families (gateway and workshop): the product is an executable inside a private container. The gateway becomes `crates/gateway`, a single real crate holding the public API (the wire vocabulary of today's `gateway-api-types` plus the discovery client of today's `gateway-api-discovery`), and `crates/gateway-service/`, a private container whose crates no outsider can reach, one of which builds the executable. The user: "crates/gateway # a single crate, public API, types plus discovery" and "crates/gateway-service/ # a directory with completely private crates which no one outside can access, period, one of them builds the executable". The gateway's internal crates then depend on the public crate for its types, the reverse of PromptForge's facade, which is correct for a public API made of plain data. Merging the two public crates can't form a cycle, because neither may depend on gateway container crates today (`container_named_exception` has no gateway entry). The workshop family has no public crate. The user: "workshop has no api".
  - For `build-xtask`: no single layout rule covers both kinds. What stays uniform is the privacy rule (outsiders name only a family's root public crates, and nothing outside a family reaches its container). A suggested direction, not decided: replace today's scattered family knowledge (`PUBLIC_*` constants, the `container_named_exception` match, the prefix matches in `family()`, `ENGINE_CONTAINER`) with one table holding, per family, its public crates, its container directory, and its kind; the facade shape check would apply only to library families whose public crate wraps internals.
  - Note for later: the package name `gateway` belongs to the gateway binary today (`crates/gateway/app`, per `vibe/2026-09-18-2-remove-tool-picker.md`). Freeing it for the public crate means renaming the binary's package, which retargets every `-p gateway`: `default-members`, CI, release workflows, and the `cargo check -p gateway --no-default-features` gate, which checks the binary's features and must follow it.
  
  Revisit after this plan lands.
- Deferred (pinned by the user): collapsing the engine into a single crate, suggested by the reviewer Marcel. One crate would replace `detail` modules with `pub(crate)` fields and functions, most of the closure check with compiler lints (`private_interfaces`, `private_bounds`, and `unnameable_types`, whose stability on the pinned toolchain is unverified), and the custom snapshot with an off-the-shelf tool; it would give up crate-enforced internal boundaries and build parallelism, and it conflicts with the maximum-internal-structure decision. The user: "Let's put a pin in Marcel's advice". Revisit if the custom tooling, chiefly the nightly rustdoc JSON closure check, proves too costly to maintain.
- Deferred: eliminating the need for `promptforge` to offer testing facilities to outside crates, so the facade drops its `test-support` feature and the second closure run. The one current user is the `harness-capabilities` integration suite (`crates/harness/capabilities/tests/it/support.rs` lines 14 and 78, importing `Performers` and `drive_tokio`); how to replace that use is the open question in Product Requirements. Offered the option of closing it in this plan by giving that suite a harness-side driver, the user kept it deferred: "no. we will deal with it later." Revisit after this plan lands, or sooner if another outside crate starts needing the engine's test drivers.
- Out of scope: Papergate, which lives in the separate `wg21-paperflow` repository (paths in this item are relative to that repository's root). Its manifest path-depends on crates that no longer exist (`promptforge-core` and `promptforge-tool-picker`, `crates/papergate/Cargo.toml` lines 19-20), and it calls APIs that have been removed (`promptforge_core::execute::run`, `crates/papergate/src/app.rs` line 156). When migrated it will depend on `harness-api`, which exposes no engine names today (`crates/harness-api/Cargo.toml` lines 18-22 list only `harness-runner` and `harness-sessions`), so it adds nothing to this plan's surface audit. The user: "papergate will use harness-api".
- Out of scope: changes to `harness-api`. It is treated as complete as written, and a capability is added only when a host such as Papergate needs it. The user: "assume that harness-api is correct as written, and that it is not missing a capability. when papergate comes knocking and needs something, only then will we add it."

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace default member; plain `cargo build` builds only this). Desktop app: `cargo build --locked -p workshop`. Workspace-wide compilation is covered by the clippy runs, so never run a standalone `cargo check --workspace` beside them; the one extra shape check is `cargo check -p gateway --no-default-features`. Toolchain is `stable` via `rust-toolchain.toml`; on Windows MSVC `.cargo/config.toml` links with rust-lld and the static CRT.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-substring>`; add `--test it` (or `--test suite` for `promptforge-api-runtime`) to target one integration binary. The `suite` binary and the `models_loop` bench require the `test-support` feature, which `--all-features` supplies. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. cargo-nextest 0.9.128 is installed locally.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test --locked -p <package> --all-features --doc` (nextest skips doctests). Structural and boundary harness: `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`. Workshop partition: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. UI suites: `npm test` in `crates/workshop/ui` and `crates/gateway/config-ui/ui` (after `npm ci`).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop partition: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; plus `cargo check -p gateway --no-default-features`. Supply chain: `cargo deny check` (cargo-deny 0.20.2 installed locally) and `cargo audit` in CI. UI typecheck: `npm run typecheck` in each UI directory.
- Formatter check command: `cargo fmt --all --check` (also the pre-commit hook).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS = "-D warnings"`); user guide: `mdbook build guide` (mdbook 0.4.44 installed locally). Rustdoc lints are not covered by clippy, so the docs gate is never skipped. `promptforge-api-runtime` crate docs come from `src/lib.md` via `#![doc = include_str!("lib.md")]`.
- Test placement and naming conventions:
  - Unit tests sit beside their module in a sibling `<module>-tests.rs`, wired at the bottom of the module as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (for example `execute/run.rs` and `execute/run-tests.rs`). Large groups use a `tests/` directory inside `src` (`promptforge-api-runtime/src/execute/tests/`, `promptforge/lua/src/protocol/tests/`, `harness/models/src/transport/tests/`).
  - Integration tests compile as one binary per crate: `tests/it/main.rs` with one module per topic (`tests/it/gateway.rs`, `tests/it/boot.rs`), shared helpers in `support.rs` or `tests/common/`. `promptforge-api-runtime` uses `tests/suite/` (declared `[[test]] name = "suite"`, `required-features = ["test-support"]`) plus Markdown prompt fixtures under `tests/prompts/valid/`, `tests/prompts/invalid/`, and `tests/prompts/execution/`.
  - Test functions are sentence-style snake_case naming the behavior, for example `a_gateway_binding_never_prints_its_key` and `set_gateway_called_twice_leaves_the_latest_generation`.
  - Test-only drivers and recorders live behind a `test-support` feature (`promptforge-api-runtime`, `promptforge-lua`, `promptforge-parser`) enabled only from dev-dependencies; benches live in `benches/` on criterion with `harness = false`.
  - `.config/nextest.toml` sets a 60s slow timeout with termination after three periods and throttles the STT crates into a `heavy` test group.
- Directory map:
  - `crates/`: every Rust crate. Root crates are the public layer: `promptforge-api-runtime`, `promptforge-api-types`, `gateway-api-types`, `gateway-api-discovery`, `harness-api`, `shared-vfs`, `shared-loopback`, `shared-error-source`, `workspace-hack` (cargo-hakari), and the `build-*` tooling crates (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-llama-cuda`).
  - `crates/promptforge/`: manifestless container of private engine crates `lua`, `parser`, `store`, `vfs`, `model-client`.
  - `crates/gateway/`: manifestless container of private gateway crates `app` (binary `promptforge-gateway`), `cloud-providers`, `config`, `config-ui` (with its own `ui/` TypeScript app), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api` as package `gateway-stt`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/harness/`: manifestless container of `runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, `web-search`.
  - `crates/workshop/`: manifestless container of `shell` (package `workshop`, Tauri binary `promptforge-workshop`), `server` (binary `workshop-server`), `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, and the `ui/` TypeScript SPA.
  - `crates/shared-ui`: shared TypeScript and CSS package consumed by both UIs, not a Rust crate. Workspace members are enumerated explicitly in the root `Cargo.toml` and the container directories are listed in `exclude`.
  - `guide/`: mdBook user guide (`book.toml`, `src/` chapters for language, agent, gateway, workshop) plus four single-file guides.
  - `prompts/`: example PromptForge Markdown prompts.
  - `tools/`: Node scripts (gateway sidecar staging, live TTS check) with `.test.mjs` tests.
  - `vibe/`: `archdoc.md` and dated work notes.
  - `.github/workflows/`: `ci.yml` (fmt, clippy, test, docs, workshop on Windows and Linux, ui, supply-chain, aggregated by `ci-green`), nightly, release, installer smoke, STT miri, CUDA and whisper builds.
  - `.githooks/`: pre-commit runs fmt; pre-push runs the headless gateway check, clippy, and cargo deny.
  - `.cargo/config.toml`: `cargo xtask` (runs `build-xtask`, subcommands `new-crate` and `tidy`) and `cargo workshop` aliases. `.config/`: `hakari.toml` and `nextest.toml`.
  - Root config: `clippy.toml`, `rustfmt.toml`, `deny.toml`, `dist-workspace.toml` (cargo-dist), `rust-toolchain.toml`, `gateway.local.example.toml`, `AGENTS.md`.
  - Untracked, local only: `local/` (operator gateway configs, prompts, fixtures), `target/`, `target-msrv/`, `guide/book/`.
- Component boundaries:
  - Tier rule: shell to features to services to vocabulary, never upward. Dependency rules bind normal, dev, build, and target-specific dependencies. A crate in a family container may depend only on root crates and its own siblings.
  - PromptForge engine: `promptforge-api-runtime` (parser plus the sans-I/O `Run` state machine) is the only outside crate allowed into `crates/promptforge/`, and with `promptforge-api-types` forms the family's public API. It depends on `promptforge-api-types`, `promptforge-lua`, `promptforge-parser`, `promptforge-store`, `promptforge-vfs`, `promptforge-model-client`, and `shared-vfs`. Internally `promptforge-parser` depends on `promptforge-lua` and `promptforge-api-types`; `promptforge-lua` on `promptforge-api-types`, `promptforge-model-client`, `promptforge-store`; `promptforge-store` on `promptforge-vfs` and `shared-vfs`; `promptforge-vfs` on `shared-vfs`; `promptforge-model-client` on `promptforge-api-types`. `promptforge-api-types` is wire vocabulary only and depends on `shared-vfs`. The family never depends on gateway, harness, or workshop crates.
  - Harness: `harness-api` is the only public crate and depends on `harness-runner` and `harness-sessions`. `harness-sessions` depends on runner, models, capabilities, log, and web; `harness-runner`, `harness-models`, and `harness-capabilities` depend on `promptforge-api-runtime`, `promptforge-api-types`, and `shared-vfs`. Harness may name the gateway public pair and shared crates, never workshop or private gateway crates.
  - Gateway: public pair `gateway-api-types` (no internal dependencies) and `gateway-api-discovery` (depends on `shared-error-source`). Private crates stack upward, for example `gateway-protocol` on `gateway-config` and `gateway-api-types`, `gateway-routing` on config and protocol, with the `gateway` app on top. Gateway never depends on promptforge, harness, or workshop crates.
  - Workshop: `workshop` shell depends on `workshop-server-api` (never `workshop-server` directly) and `gateway-api-discovery`; `workshop-server-api` re-exports `workshop-server`; `workshop-server` depends on `harness-api`, `promptforge-api-types`, `gateway-api-discovery`, `shared-loopback`, and its workshop-* siblings. Workshop never names private gateway crates or harness internals.
  - Shared: `shared-vfs`, `shared-loopback`, and `shared-error-source` depend on no product crate.
  - `cargo test -p build-xtask` enforces the product-boundary matrix, container privacy, single-public-crate rules for promptforge and harness, the tier graph, the `## Invariants` marker, lint inheritance, and the 500-line ceiling.
  - The archdoc names a CLI component, but the tree has no CLI crate or binary; the binaries are `promptforge-gateway`, `promptforge-workshop`, `workshop-server`, `shared-cloud-providers`, and the `build-*` tools.
- Conventions summary:
  - Rust 2024 edition, resolver 3, workspace version 0.3.0, BSL-1.0. Every member inherits `[workspace.lints]`: `unsafe_code = "forbid"`, `missing_docs`, `unreachable_pub`, and `missing_debug_implementations` warn, clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, rustdoc broken and private intra-doc links deny.
  - Dependencies are declared once in `[workspace.dependencies]` and inherited with `.workspace = true`; every member depends on `workspace-hack` except `shared-vfs`, which keeps a zero-dependency rule and is listed under `[final-excludes]` in `.config/hakari.toml`. Manifest dependency lines carry rationale comments.
  - Crate names are family-prefixed (`promptforge-*`, `gateway-*`, `harness-*`, `workshop-*`, `shared-*`, `build-*`); container directories use short names (`crates/harness/runner` is `harness-runner`).
  - Source directories are flat: one or two submodule files become kebab siblings `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`, three or more become a `foo/` directory. Every workshop-* and harness-* `lib.rs` opens with a `//!` doc containing `## Invariants`, and marker crates keep files under 500 lines.
  - Errors use `thiserror`; error and status messages are written for model consumption, naming what is missing with required versus actual.
  - Comments explain non-obvious constraints; workarounds cite upstream issue URLs.
  - Cargo features gate real constraints only (`test-support`, `headless`, `test-fixtures`), never product shape.
  - JSON that reaches the run log or replay round-trips exactly: `serde_json` with `float_roundtrip`, sorted keys, never `preserve_order`.
  - Behavior changes ship with tests in the same change; new structural checks need explicit user approval.
  - Verification commands pass `--locked`, and CI fails if a build dirties the tree.
  - SPA: CSS beside its TypeScript, `--ws-*` tokens only, no `localStorage`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Record the baseline and rename the container [completed]

- Component: container-rename
- Component order: first of five (container-rename, facade-firewall, surface-checks, surface-closure, facade-docs). It comes first because the facade takes the directory `crates/promptforge/` that the container holds today. The rename changes paths only, so the tree builds before and after it.
- Piece: rename, the only piece in this component, built as one step.
- Baseline, taken before any edit. Write it to the scratch file `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/baseline.md`, creating the directory if needed, with each command beside its result. Nothing from it is committed; Steps 3, 4, and 13 compare against it:
  - Count tests per partition. `default-members` in the root `Cargo.toml` is only `crates/gateway/app`, so a bare `cargo nextest list` counts only the gateway. Run `cargo nextest list --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` and `cargo nextest list --locked -p workshop -p workshop-server -p workshop-server-api`, and record the doctest counts from `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
  - Run the standard gates below, plus `cargo deny check` and `cargo hakari verify`, and record pass or fail for each. `cargo xtask api --check` and the facade docs build don't exist yet.
- Standard gates, which every commit in this plan must pass (from `AGENTS.md`, Verification):
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - both clippy partitions with `-D warnings`
  - `cargo fmt --all --check`
  - `cargo check -p gateway --no-default-features`
  - the workspace docs gate
  - `mdbook build guide`
  - `cargo test -p build-xtask`
  - after any manifest change, `cargo hakari generate` then `cargo hakari verify`; after any new external dependency, `cargo deny check`
- Rename with `git mv crates/promptforge crates/promptforge-internal`. Change no file contents inside it.
- Root `Cargo.toml`:
  - the explicit `members` entries `crates/promptforge/<name>` become `crates/promptforge-internal/<name>`
  - the `exclude` entry `crates/promptforge` becomes `crates/promptforge-internal`. Otherwise the `crates/*` members glob matches the manifestless directory.
  - the comment above `exclude` that names `crates/promptforge`
  - the `[workspace.dependencies]` paths
- `crates/build-xtask/src/engine_guards.rs`: `ENGINE_CONTAINER` (line 19) becomes `"promptforge-internal"`, and the other mentions of the container directory (lines 7 and 45) follow.
- `crates/build-xtask/src/engine_deps.rs`: the module docs naming the container (line 5).
- `crates/build-xtask/src/product.rs`:
  - the `container_named_exception` key (line 246) becomes `"promptforge-internal"` and still maps to `Some("promptforge-api-runtime")`
  - the module docs (lines 18-27)
  - `family()` gains an exact `"promptforge"` match beside its `gateway` and `workshop` matches
- Fixtures: the path fixtures in `product-container-tests.rs`, `product-test-support.rs` (line 14), `engine_guards-tests.rs`, and `test_support_leak-tests.rs`, and a new case showing that package `promptforge` belongs to the PromptForge family. `product-tests.rs` is at 441 of the 500-line ceiling, so put the new case in whichever sibling test file keeps every file under 500 lines.
- Every other build, CI, or config file that names a `crates/promptforge/` path, so the tree still builds. Search `.github/`, `.config/`, `.cargo/`, `deny.toml`, `dist-workspace.toml`, and source `include_str!` or `#[path]` strings. Prose in `AGENTS.md`, READMEs, and `guide/` waits for Step 13.
- Tests: the new `family()` case and the updated fixtures in `cargo test -p build-xtask`, and the standard gates.
- Commit: the rename, the build-xtask updates, and the `family()` test.

</step-1>

<step-2>

### Step 2: Audit the surface and create the facade [completed]

- Component: facade-firewall
- Component order: second. It needs the directory that Step 1 freed. It also has to come before the checks, which read the facade and the internal crates at their final paths.
- Piece: facade crate. The three pieces of this component (facade crate, consumer migration, engine relocation) are sequential. Hosts can't migrate to a crate that doesn't exist, and container privacy blocks the relocation until no outside crate depends on the old crates.
- Surface audit. Write it to the scratch file `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/surface-audit.md`. It is not committed; Steps 3, 4, and 8 read it:
  - List every `promptforge_api_runtime::`, `promptforge_api_types::`, and `shared_vfs::` path used by crates outside the engine, including their tests, benches, doc examples, and manifests with each dependency kind. Group the paths by role, and mark each use as non-test code or test-only (tests, benches, test helpers, doc examples). Only non-test uses justify a re-export. Each test-only use of an item nothing else justifies is recorded as an outside test to rewrite in Step 3.
  - List the imports of the runtime's `tests/suite/` (`main.rs`, `support.rs`, and nine topic modules) and `benches/models_loop`, which move to the facade in Step 4. Mark every test that needs an item outside the host surface; per the Decision Record, those tests move into the engine instead.
  - Record the defining crate of every item, because each facade `pub use` names the defining crate's path.
  - Decide the one home of `StreamDelta`, and the homes of the parser front-matter items (`Frontmatter`, declarations, roles).
  - Decide which `promptforge-vfs` items (`STORE_MOUNT`, `empty`, `Mode`, `ModePolicy`, `ModeHandle`) join `vfs`, based on whether a host assembles a run's VFS itself or gates its own mounts by mode. The rest of `vfs` is the whole contract listed in Technical Design, regardless of use counts.
  - Record which `Access` operations (`spawn`, `id`) hosts use, and which fields of `ToolCall` and of the completion result hosts read. Step 8 needs both.
  - Record, for every trait hosts can reach today other than the open `vfs` traits (`Vfs`, `VfsAccess`, `Policy`), whether hosts call it and whether any crate outside the engine implements it. Known today: `ChunkSource`, which `harness-models` implements (`crates/harness/models/src/transport.rs` line 75), and `ModelView` (re-exported at `crates/promptforge-api-runtime/src/model.rs` lines 37-39), which no host implements. Step 8 needs this.
  - Record the auto traits the API promises, starting with `Run: Send` (documented at `execute/run.rs` lines 82-84).
- `crates/promptforge/Cargo.toml`:
  - package `promptforge`, `publish = false`, workspace lints, and `workspace-hack`
  - a normal dependency on each defining crate the audit found. Today these are `promptforge-api-runtime`, `promptforge-api-types`, `shared-vfs`, `promptforge-lua`, `promptforge-parser`, `promptforge-store`, `promptforge-model-client`, and `promptforge-vfs`.
  - `test-support = ["promptforge-api-runtime/test-support"]`, so `harness-capabilities` can migrate in Step 3. Step 4 retargets it to the engine.
  - root `Cargo.toml`: add `promptforge` to `[workspace.dependencies]`; the `crates/*` glob picks up the member
- `crates/promptforge/src/lib.rs`:
  - `#![doc = include_str!("lib.md")]`
  - the root re-exports (`Prompt`, `Run`, `Step`, `RunContext`, `Environment`, `RunResult`, and the errors they raise, such as `ParseError` and `RunError`)
  - one `pub mod` block per role, each with a one-line doc comment: `effect`, `event`, `ids`, `model`, `transport`, `tools`, `capabilities`, `vfs`, `cancel`, `timestamp`, `metrics`, `input`, adjusted by the audit
  - every audited item re-exported once, as a single-item `pub use`
  - the test drivers the audit found, such as `Performers` and `drive_tokio`, under `#[cfg(feature = "test-support")]`
  - no item definitions, glob re-exports, module re-exports, or crate re-exports. The transport items keep the `#[doc(hidden)]` they have at their definitions until Step 9.
- `crates/promptforge/src/lib.md`: a short crate summary. The curated table of contents comes in Step 12.
- `crates/promptforge/tests/suite/main.rs` and `tests/suite/auto_traits.rs`: compile-time assertions for each promised auto trait. Declare them as `[[test]] name = "suite"` with `required-features = ["test-support"]`, so that Step 4 merges the runtime suite into this binary.
- `crates/build-xtask/src/product.rs`:
  - `container_named_exception` maps `"promptforge-internal"` to `Some("promptforge")`
  - new temporary constant `TRANSITIONAL_CONTAINER_EXCEPTIONS` admits `promptforge-api-runtime` into `promptforge-internal`. The container check consults it only as an extra allowance.
  - `PUBLIC_PROMPTFORGE` becomes `[&str; 3]`: `promptforge`, `promptforge-api-runtime`, `promptforge-api-types`
  - register the facade anywhere else build-xtask assigns a crate a family or tier
- `crates/build-xtask/src/engine_guards.rs`: `ENGINE_ROOT_CRATES` becomes `[&str; 3]` and adds `promptforge`.
- Tests in `product-tests.rs` and `product-container-tests.rs`, placed so every file stays under 500 lines (`product-tests.rs` starts near the ceiling, so container cases go in `product-container-tests.rs`):
  - `promptforge` may depend on container crates
  - `promptforge-api-runtime` is admitted only through the transitional entry
  - any other root crate is still rejected
- Tests in `engine_guards-tests.rs` and `test_support_leak-tests.rs`: the facade forwarding `test-support` is exempt from the leak guard.
- Run `cargo hakari generate`.
- Tests:
  - the new build-xtask cases
  - `cargo nextest run --locked -p promptforge --all-features`, which covers the auto-trait assertions
  - the standard gates
- Commit: the facade crate, its auto-trait tests, and the transitional build-xtask state.

</step-2>

<step-3>

### Step 3: Migrate every outside crate to the facade [completed]

- Component: facade-firewall
- Piece: consumer migration. It comes after the facade crate, whose re-exports it imports, and before the relocation. It is one step because the two test partitions are the one test set that covers it.
- Manifests: every dependency of every kind (normal, dev, build, target) on `promptforge-api-runtime`, `promptforge-api-types`, or `shared-vfs` becomes a dependency on `promptforge`, with its rationale comment updated. The crates, per the surface audit that Step 2 wrote to `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/surface-audit.md`:
  - `harness-runner`, `harness-models`, `harness-sessions`, `harness-webfetch`, `harness-web-search`
  - `harness-capabilities`: the normal dependency, plus a dev-dependency on `promptforge` with `features = ["test-support"]` that replaces today's runtime dev-dependency (`Cargo.toml` line 27)
  - `harness-log`, which uses it as a dev-dependency
  - `harness-web`: its `shared-vfs` dev-dependency
  - `workshop-server`, `workshop-workspace`, `workshop-protocol`, `workshop-gateway`
  - any other crate the audit lists
- Sources: every `use` and path in those crates' sources, tests, benches, and doc examples becomes the `promptforge::` path the audit assigned (about 60 files). This includes `crates/harness/models/src/transport.rs` lines 14-17, which move to `promptforge::transport`, and `crates/harness/capabilities/tests/it/support.rs` lines 14 and 78.
- Outside tests the audit recorded as using an item nothing else justifies are rewritten to use only the facade's surface or their own crate's helpers, never by adding a re-export. The one exception is the forwarded `test-support` drivers (`Performers`, `drive_tokio`), which `harness-capabilities` keeps using until the deferred work removes them.
- Run `cargo hakari generate`.
- Tests:
  - the standard gates, with both test counts at or above the baseline in `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/baseline.md`
  - `rg` for the six old crate names (kebab and snake spellings) over the manifests and `.rs` files under `crates/harness/` and `crates/workshop/` returns nothing, comments included. Other places still name the old crates on purpose: root `Cargo.toml`, `Cargo.lock`, `crates/build-xtask/`, and `.config/hakari.toml` until Steps 4 and 5, and prose (`AGENTS.md`, READMEs, `guide/`, `tools/document.md`, and comments in `crates/harness-api/src/lib.rs` and `crates/gateway/app/src/dialect.rs`) until Step 13
- Commit: the host migration.

</step-3>

<step-4>

### Step 4: Move the types and runtime crates into the container [completed]

- Component: facade-firewall
- Piece: engine relocation. It comes after the migration, because container privacy blocks outside crates from the moved crates. Its two steps are sequential: this step deletes the transitional exception as soon as the runtime is inside the container, and Step 5 then changes only edges inside the container and the facade's paths.
- Types crate: `git mv crates/promptforge-api-types crates/promptforge-internal/types`, and rename the package to `promptforge-types`.
- Runtime crate: `git mv crates/promptforge-api-runtime crates/promptforge-internal/engine`, and rename the package to `promptforge-engine`. It keeps `src/`, its unit tests, and its `test-support` feature (`Cargo.toml` lines 33-40).
- Suite and bench:
  - move `tests/suite/`, the `[[test]]` declaration (lines 61-63), `benches/models_loop`, and its `[[bench]]` declaration (lines 65-68) to `crates/promptforge/`, and merge the suite's `main.rs` with the facade's `auto_traits` module
  - rewrite their imports to `promptforge::` paths only
  - tests that the surface audit (Step 2's `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/surface-audit.md`) marked as needing a non-surface item move into the engine's unit tests under `crates/promptforge-internal/engine/src/execute/tests/`, changing only their paths
  - the prompt fixtures under `tests/prompts/` move with the suite. If an engine unit test also reads them, they stay in the engine and the suite points at them by relative path.
  - the suite's and bench's dev-dependencies move to the facade manifest
- Facade:
  - `test-support` becomes `["promptforge-engine/test-support"]`
  - re-export paths change from `promptforge_api_runtime::` to `promptforge_engine::`, and from `promptforge_api_types::` to `promptforge_types::`
  - drop the old dependencies, and add `promptforge-engine` and `promptforge-types`
- Container crates that depend on the types crate (`promptforge-lua`, `promptforge-parser`, `promptforge-model-client`, and any others) rename that dependency and their imports to `promptforge-types`.
- Remove `pub use promptforge_api_types as types;` (the runtime's `lib.rs` line 33). Also remove the rule it states in the engine's `lib.md` (line 3), the `types::` doc example in `lib.md`, and the README example that uses `types::`.
- Root `Cargo.toml`: add explicit `members` entries for `crates/promptforge-internal/types` and `crates/promptforge-internal/engine`, and rename their `[workspace.dependencies]` entries.
- build-xtask:
  - delete `TRANSITIONAL_CONTAINER_EXCEPTIONS`, its handling, and its tests
  - `PUBLIC_PROMPTFORGE` becomes `[&str; 1]` holding `"promptforge"`
  - `ENGINE_ROOT_CRATES` becomes `[&str; 1]` holding `"promptforge"`
  - confirm that the leak guard's forwarding exemption (`test_support_leak.rs` line 114) accepts the facade forwarding a container crate's `test-support`
  - update the fixtures in `product-tests.rs` (single public crate `promptforge`), `engine_guards-tests.rs`, and `test_support_leak-tests.rs`
- Run `cargo hakari generate`.
- Tests:
  - `cargo nextest run --locked -p promptforge --all-features --test suite`, with the suite's test count unchanged apart from tests moved into the engine
  - `cargo bench --locked -p promptforge --all-features --no-run`
  - `cargo test -p build-xtask`, which now proves no outside crate depends on any promptforge crate but `promptforge`
  - the standard gates, with test counts at or above the baseline in `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/baseline.md`
- Commit: the relocation, the moved suite and bench, and the final public-crate lists.

</step-4>

<step-5>

### Step 5: Fold shared-vfs into promptforge-vfs [completed]

- Component: facade-firewall
- Piece: engine relocation, continued from Step 4.
- Drop the types crate's vfs edge first:
  - reword the `shared_vfs::Origin` intra-doc link in `crates/promptforge-internal/types/src/ids.rs` (lines 191-192) and the vfs prose in `src/lib.rs` (lines 26-28) as plain text that names no crate
  - remove the `shared-vfs` entry from its `Cargo.toml` (line 18)
- Move every source file of `crates/shared-vfs/src/` except `lib.rs` (`error.rs`, `glob.rs`, `grep.rs`, `handle.rs`, `host.rs`, `memory.rs`, `observe.rs`, `path.rs`, `router.rs`, `stat.rs`, `traits.rs`) into `crates/promptforge-internal/vfs/src/` with `git mv`, and merge the module declarations and exports of its `lib.rs` (lines 21-30) into the `promptforge-vfs` `lib.rs`, which is today that crate's only source file. They become modules beside the existing `STORE_MOUNT`, `empty`, and mode code, and every public name stays the same. The existing `shared_vfs::` imports in `promptforge-vfs` become `crate::` paths, and unit tests move with their modules.
- Fold `crates/shared-vfs/AGENTS.md` into `crates/promptforge-internal/vfs/AGENTS.md`, creating it if absent, keeping every rule it states and naming the merged crate.
- Move `the_manifest_declares_no_dependencies` (`crates/shared-vfs/src/lib.rs` lines 48-73) into `promptforge-vfs` unchanged. Drop both `shared-vfs` and `workspace-hack` from the `promptforge-vfs` manifest, so it declares no dependencies.
- `.config/hakari.toml`: change `[final-excludes] workspace-members = ["shared-vfs"]` (lines 23-26) to `["promptforge-vfs"]`, and update the comment above it to name `promptforge-vfs`.
- `promptforge-store` and `promptforge-engine` drop `shared-vfs` and use `promptforge_vfs::` paths.
- Facade: the `vfs` re-exports change from `shared_vfs::` to `promptforge_vfs::`, and its `shared-vfs` dependency goes away.
- Retire `crates/shared-vfs/Cargo.toml` and any remaining files with `git rm`, so the `crates/*` glob no longer finds the crate. Remove `shared-vfs` from root `[workspace.dependencies]`.
- build-xtask: update the `product-tests.rs` cases that name `shared-vfs` (eight references).
- Run `cargo hakari generate`.
- Tests:
  - `the_manifest_declares_no_dependencies` passes on the merged crate, which declares no dependencies
  - `cargo hakari generate` leaves `promptforge-vfs` without `workspace-hack`, and `cargo hakari verify` passes
  - the moved vfs unit tests pass
  - the standard gates pass
- Commit: the fold and the dropped types-to-vfs edge.

</step-5>

<step-6>

### Step 6: Add the facade shape check [completed]

- Component: surface-checks
- Component order: third. The checks read the facade and the internal crates at their final paths, so they come after the firewall. They come before the closure fixes, because `cargo xtask api` produces the list of leaks to fix and proves the fixes complete.
- Piece: source shape check. It is joint with the rustdoc check in Step 7; neither uses the other, so they can be built in either order or in parallel. The `doc(hidden)` ban lives in Step 9, because a gate can't turn on before its fixes.
- `crates/build-xtask/Cargo.toml` gains `syn` with full parsing, declared in root `[workspace.dependencies]` with a rationale comment.
- `crates/build-xtask/src/facade_shape.rs` and `facade_shape-tests.rs`, wired from `main.rs` the way `engine_guards` is:
  - parse every `.rs` file under `crates/promptforge/src/`
  - accept grouping `pub mod` blocks with doc comments, single-item `pub use krate::path::Item;` lines, doc attributes, and `#[cfg(feature = "test-support")]`
  - reject glob re-exports, grouped `{A, B}` use lists, crate re-exports (a single-segment path, with or without `as`), and every item definition (`fn`, `struct`, `enum`, `trait`, `impl`, `const`, `static`, `type`, `macro_rules!`)
  - each error names the file and line, the rule broken, and what was required versus found
- Module re-exports: syntax alone can't tell a module from a function, since both are snake_case. So `cargo xtask api` rejects a facade `use` whose target is a module (Step 7), using the item kinds in rustdoc JSON. That is where the Testing Plan's module re-export fixture lives.
- Keep every file under 500 lines.
- Tests:
  - fixtures that accept each allowed form and reject each forbidden form
  - the check passes on the real facade inside `cargo test -p build-xtask`
  - `cargo deny check`
  - the standard gates
- Commit: the shape check and its fixtures.

</step-6>

<step-7>

### Step 7: Implement cargo xtask api [completed]

- Component: surface-checks
- Piece: rustdoc check, joint with Step 6.
- `crates/build-xtask/Cargo.toml` gains `rustdoc-types`, pinned to the version that matches the nightly, and `serde_json`, both declared in root `[workspace.dependencies]`.
- `crates/build-xtask/src/api.rs` is the subcommand entry for `cargo xtask api`, with `--check` and `--bless`, dispatched from `main.rs` beside `new-crate` and `tidy`. Its submodules live in `crates/build-xtask/src/api/`, per the flat-directory rule for three or more submodules:
  - `toolchain.rs`: one constant pairing the pinned nightly date with the `rustdoc-types` version. The command is invoked as `cargo +<pinned nightly> xtask api`, and on any other active toolchain it fails at once with a message naming the required nightly. Pin a nightly whose rustdoc JSON `format_version` matches a published `rustdoc-types` release, preferring one already installed locally (`nightly-2026-09-05` is) so no toolchain download is needed.
  - `load.rs`: builds rustdoc JSON for the facade and every crate under `crates/promptforge-internal/`, into `target/xtask-api/`. It runs once with the default features and once with the facade's `test-support` on, building each internal crate with the feature set that facade build activates.
  - `items.rs`: matches items across crates by crate name and path, because rustdoc reports the internal path of a re-exported item. It resolves each facade `use` to its defining item and rejects a `use` whose target is a module.
  - `closure.rs`: walks the surface, meaning signatures, fields, variants, generic bounds, supertraits, both sides of every trait impl, associated types, and each surface item's `links` table. Every mention must be a facade re-export, std, core, or alloc, or come from a crate in one allowlist constant (`serde`, `serde_core`, `serde_json`, `serde_yaml_ng`).
  - `doc_text.rs`: reports any internal crate name, in kebab or snake spelling, found in a surface item's doc text.
  - `listing.rs`: builds the listing from the default build only, with one sorted line per surface item, including the methods, fields, variants, and trait impls of every re-exported type.
- Behavior:
  - a plain run prints every violation from both builds and the listing's difference from `crates/promptforge/public-api.txt`
  - `--check` fails on any violation or any difference
  - `--bless` rewrites the listing, and refuses while any violation remains
  - each violation names the item, what it mentions, and what was required versus found
- Fixture tests live in `api/*-tests.rs` siblings. Each builds a small generated workspace, a facade crate plus one internal crate, in a temp directory. They cover:
  - a type in a signature that isn't re-exported
  - an internal trait in a bound or supertrait
  - a trait impl that mentions an internal type
  - an intra-doc link to an internal item, including one written as `crate::` or `super::` whose target the facade doesn't re-export
  - a link whose target the facade re-exports, which passes
  - an internal crate name in doc text
  - a type from an allowlisted crate, which passes
  - a module re-export
  - a snapshot difference
  - a toolchain other than the pinned nightly, which fails with a message naming it
- `cargo test -p build-xtask` runs on stable, so every fixture that needs rustdoc JSON carries `#[ignore = "needs the pinned nightly"]`. The toolchain-mismatch fixture runs in the stable suite. Locally, run the ignored fixtures with `cargo +<pinned nightly> nextest run --locked -p build-xtask --run-ignored only`. The CI job in Step 11 runs them.
- Keep every file under 500 lines.
- Don't commit `public-api.txt` yet. Run `cargo +<pinned nightly> xtask api` once over the repository to confirm it runs end to end and reports the known leaks. Steps 8 through 11 each rerun it for their current findings.
- Tests:
  - every api fixture, with the ignored ones run on the pinned nightly
  - `cargo test -p build-xtask`
  - `cargo deny check`
  - the standard gates
- Commit: `cargo xtask api` and its fixtures.

</step-7>

<step-8>

### Step 8: Move engine-only operations into detail functions [completed]

- Component: surface-closure
- Component order: fourth. The list of fixes comes from `cargo xtask api`, and so does the proof that they're done.
- Piece: engine-only operations. The four pieces of this component (engine-only operations, hidden items, doc closure, snapshot) run in this order, except that doc closure is independent. This piece comes first because removing `#[doc(hidden)]` from these items while they still sit on host-visible types would publish them.
- Inputs: the surface audit that Step 2 wrote to `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/surface-audit.md` (host use of `Access` operations, the `ToolCall` and completion-result fields hosts read, and which reachable traits hosts implement), and a fresh `cargo +<pinned nightly> xtask api` run for the current findings.
- New `detail` modules, which the facade never re-exports. Each is declared `pub mod detail;` with a module doc stating that constraint:
  - `crates/promptforge-internal/model-client/src/detail.rs`
  - `crates/promptforge-internal/types/src/detail.rs`
  - `crates/promptforge-internal/lua/src/detail.rs`
  - `crates/promptforge-internal/vfs/src/detail.rs`, only if the audit moves `Access` operations
- Conversions. Each keeps behavior identical and updates every engine caller:
  - model-client `client/wire.rs`:
    - `Message::from_validated_parts`, `assistant_tool_calls`, `content_value`, `raw_tool_calls`, and `ToolSchema::new` become free functions such as `detail::message_from_validated_parts`
    - `ToolCall` fields become private, with `detail` constructors and field access for the engine. The host accessors `ToolCall::id`, `name`, and `arguments` stay unchanged.
    - completion result fields (`result` through `response_body`) become private with `detail` access, except fields the audit shows hosts read, which keep a host accessor
  - types: `ModelId::from_validated` (`models.rs` line 73), `CapabilityId::from_validated` (`capabilities.rs` line 74), and `ToolId::from_validated` (`tools/ids.rs` line 59) become `detail::model_id_from_validated`, `detail::capability_id_from_validated`, and `detail::tool_id_from_validated`
  - model-client `error.rs`: `Error::http` becomes a `detail` function
  - lua `error.rs`: `SharedSource::new` becomes a `detail` function
  - vfs: `Access::spawn` and `Access::id` become `detail` functions, if the audit found no host use
  - traits: a reachable trait that hosts call but never implement, such as `ModelView` (model-client), becomes a concrete type or enum if it stays on the surface, or, if a trait is unavoidable, is sealed with a supertrait from a module the facade never re-exports. A trait hosts don't use at all is left to Step 11's minimality pass. `ChunkSource` stays open, because `harness-models` implements it.
  - engine `execute/error.rs` lines 209 and 215: if `cargo xtask api` flags `impl From<Error> for RunError` or its reverse, replace them with crate-private conversion functions beside them, since `Error` is crate-private. `?` call sites become explicit `map_err`.
  - the `#[doc(hidden)]` attributes on the converted items go with them
- Tests:
  - existing unit and integration tests pass with only their call paths changed, and host accessors behave identically
  - `cargo xtask api` reports no finding on any converted item
  - the standard gates
- Commit: the detail conversions.

</step-8>

<step-9>

### Step 9: Clear every doc(hidden) and turn on the ban

- Component: surface-closure
- Piece: hidden items, after Step 8.
- Test helpers go behind `test-support` in their defining crate: `for_test` (lua `handles.rs` lines 85 and 163), `run_chunk`, `tool_bag_handles`, and `model_bag_handles` (lua `vm.rs` lines 755, 900, and 921), and any other engine-internal test helper. Crates whose tests use them enable the feature only through dev-dependencies, which the leak guard enforces.
- Transport codec:
  - remove `#[doc(hidden)]` from `ChunkSource`, `build_request_body`, `read_body_capped`, `read_completion_stream`, `escape_controls`, `ClientError`, and `ClientTimeout` at their definitions in model-client: `client.rs` lines 30-36, `client/stream.rs` lines 41, 82, 117, and 450, `client/read.rs`, `client/request.rs`, `error.rs`, and `lib.rs` line 38
  - write item docs that a transport implementer can follow
  - delete the engine's hidden re-exports of these items (`engine/src/model.rs` lines 29-42), replacing any engine use of those paths with a direct import
- Every other `#[doc(hidden)]` under `crates/promptforge-internal/` and `crates/promptforge/`, from the 78-attribute inventory in Technical Design minus the ones Step 8 removed:
  - an internal-only item, such as the `lifecycle` module (types `event.rs` line 57), just loses the attribute
  - a member of a surface type, such as the two on `StoreError` (store `error.rs` lines 300 and 314), becomes private or a `detail` function, as in Step 8
- The ban lives in `crates/build-xtask/src/doc_hidden.rs` and `doc_hidden-tests.rs`. It parses every `.rs` file under both directories with `syn` and rejects `doc(hidden)` in outer or inner attributes on items, fields, variants, methods, and re-exports, naming the file and line. It runs over the real tree in `cargo test -p build-xtask`.
- Keep every file under 500 lines.
- Tests:
  - ban fixtures covering items, fields, methods, and re-exports
  - the ban passes on the tree
  - `cargo xtask api` reports no test-helper finding on the `test-support` build
  - `harness-models` builds against `promptforge::transport` with no hidden items involved
  - the standard gates
- Commit: the cleared attributes, the transport docs, and the ban.

</step-9>

<step-10>

### Step 10: Make surface docs name only facade paths

- Component: surface-closure
- Piece: doc closure. It is joint with Steps 8 and 9, because it edits only doc comments and needs nothing from them. It is placed after them so that one `cargo xtask api` run confirms the link and doc-text findings are gone.
- Intra-doc links on surface items. Every `crate::`, `super::`, or `self::` link in the docs of an item the facade re-exports must either target another facade re-export, which rustdoc retargets to the facade page, or become plain text. Never re-export an item only to make a link work.
  - Start from the known sites: about 21 on the engine's root exports, including about 11 `super::Environment` links in `engine/src/execute/config.rs` (lines 22-301) and two in `execute/error.rs` (lines 72-73); about 7 in `test_support`; and about 12 in the types crate, including `replay.rs` line 98, `tools/descriptor.rs` line 16, and `tools/ids.rs` line 178.
  - `cargo xtask api` finds the rest.
  - Links on the engine's crate-private `Error` (`error.rs` lines 4-5, 210, and 263) are not on the surface and stay.
- Doc text: reword every surface doc that names an internal crate.
- Doc examples:
  - rewrite the examples on surface items in internal crates to use `promptforge::` paths
  - add `promptforge` as a dev-dependency of each crate that has such examples, with a rationale comment: only the defining crate's doctest run executes them
  - `promptforge-vfs` can't take that dev-dependency, because `the_manifest_declares_no_dependencies` admits no dependency of any kind. Its `MemoryBackend` example (from the old `memory.rs`) moves into the doc comment of the facade's `vfs` module.
  - if build-xtask's dependency rules reject a dev-dependency from a container crate to the facade, use the Decision Record's fallback (move the examples into facade module docs) rather than loosening a check
- Tests:
  - the workspace docs gate
  - `cargo test --locked -p <crate> --all-features --doc` for every internal crate and the facade
  - `cargo xtask api` reports zero link and doc-text findings on both builds
  - the standard gates
- Commit: the doc-link, doc-text, and doc-example rewrites.

</step-10>

<step-11>

### Step 11: Bless the surface listing and gate it in CI

- Component: surface-closure
- Piece: snapshot, last in this component, because `public-api.txt` is blessed only after zero violations.
- Fix every finding `cargo xtask api` still reports on either build, by re-exporting an item closure requires, moving an operation into a `detail` function, or removing the mention.
- Minimality pass: every re-export is either used by the non-test code of a crate outside the engine, needed to keep the surface closed, or part of `vfs`. Uses in tests, benches, test helpers, or doc examples never count, including the facade's own suite, per the Decision Record. Remove any re-export that is none of these. A facade suite test that loses an item this way moves into the engine's unit tests, changing only its paths. The forwarded `test-support` drivers are the one temporary exception, and the default-build listing excludes them.
- Run `cargo +<pinned nightly> xtask api --bless` and commit `crates/promptforge/public-api.txt`.
- `.github/workflows/ci.yml`: a new job that installs the nightly named by the constant in `api/toolchain.rs`, runs `cargo xtask api --check` and the nightly-only fixtures (`cargo nextest run --locked -p build-xtask --run-ignored only`), and joins the `ci-green` aggregation. The job reads the nightly from that constant instead of repeating the date, so the two cannot drift.
- Tests:
  - `cargo xtask api --check` passes locally on the pinned nightly
  - the ignored fixtures pass
  - the standard gates
- Commit: the last closure fixes, `public-api.txt`, and the CI job.

</step-11>

<step-12>

### Step 12: Write the facade topic docs and gate them

- Component: facade-docs
- Component order: fifth and last. The topic docs describe the final surface, and the old-name sweep needs every rename done.
- Piece: facade docs, then repository docs. The two are sequential because the Verification section of `AGENTS.md` names the facade docs gate that this step adds.
- `crates/promptforge/src/lib.md`: a curated table of contents grouped by role that links every module and its key items. It replaces the Step 2 summary.
- A topic doc for each role module, written as `crates/promptforge/src/<module>.md` and attached with `#[doc = include_str!("<module>.md")]` on its `pub mod` block. Each doc explains its role under headings, which rustdoc turns into a sidebar table of contents, and links only to facade items.
  - The design prose from the engine's `execute.rs` and `execute/run.rs` (lines 1-28) moves into the root docs and the relevant topic docs, and leaves the private modules.
  - `vfs.md` covers mounting with `VfsRef::overlay` and `VfsRefBuilder`, implementing `VfsAccess` and which of its methods have default bodies, what `Policy` gates, and read-only mounts. It also holds the `MemoryBackend` example from Step 10 and the rule that any method added later to `Vfs`, `VfsAccess`, or `Policy` needs a default body.
  - `transport.md` explains how a host performs one model round with the codec.
- `.github/workflows/ci.yml` docs job: add `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`.
- Check `target/doc/promptforge/` by hand:
  - every surface item has a page
  - `Provenance`, `Event`, and every other types-crate item hosts use has its own page under a role module
  - `Step::Pending` links to `Provenance`
- Tests:
  - the facade docs build with warnings denied
  - `cargo test --locked -p promptforge --all-features --doc` for the topic doc examples
  - `cargo xtask api --check`, which scans module doc text
  - the standard gates
- Commit: `lib.md`, the topic docs, the moved design prose, and the docs gate.

</step-12>

<step-13>

### Step 13: Update AGENTS.md, sweep old names, and run the exit gates

- Component: facade-docs
- Piece: repository docs, after Step 12.
- `AGENTS.md`:
  - Structure: the one public crate `promptforge` at `crates/promptforge/`, and the private container `crates/promptforge-internal/` holding the types, the engine, and the vfs crate, with no `shared-vfs`
  - Verification: add `cargo +<pinned nightly> xtask api --check` and `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`
- Sweep READMEs, crate `AGENTS.md` files, `guide/`, `tools/document.md`, `clippy.toml` comments, and comments in source files outside the engine (known: `crates/harness-api/src/lib.rs` and `crates/gateway/app/src/dialect.rs`) for `promptforge-api-runtime`, `promptforge_api_runtime`, `promptforge-api-types`, `promptforge_api_types`, `shared-vfs`, and `shared_vfs`, plus container paths `crates/promptforge/<engine crate>` left stale by Step 1. Update or reword each hit. Leave the Lua chunk-name strings `@crates/promptforge/lua/...` (lua `coro.rs` lines 31 and 39, `messages.rs` line 28) unchanged: they are labels, not file paths, and tests or recorded error text may depend on them.
- Exit gates: every exit criterion in the Testing Plan:
  - the standard gates
  - `cargo deny check` and `cargo hakari verify`
  - the facade docs build
  - `cargo xtask api --check` on the pinned nightly
  - both partitions' test counts and both doctest counts at or above the baseline in `C:/Users/Vinnie/cursor/cabinet/_scratch/vibe-coder-2026-09-23-1-promptforge-api-firewall/baseline.md`
- Tests:
  - the exit gates above
  - `rg` for the six old names over code, manifests, CI, and the swept docs returns nothing, excluding `vibe/`, whose dated notes are history
- Commit: the documentation sweep.

</step-13>

</execution-plan>
