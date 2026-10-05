---
name: Plugin terminology rename
overview: Rename the activation unit from "capability" to Plugin, a capitalized defined term, across the promptforge repository (code, wire format, frontmatter, prose, and every plan's contents) and the promptforge-docs guide. Add Plugin as the fourth term in the root AGENTS.md, and extend the existing docs-claims guard so rulebooks cannot call a Plugin a capability again.
todos:
  - id: step-1
    content: "Step 1: rename the code to Plugin (crate move, name map, message texts, frontmatter key, Workshop JSON, service-id check, public-api.txt, Cargo.lock)"
    status: pending
  - id: step-2
    content: "Step 2: extend docs-claims.mjs, define Plugin in the root AGENTS.md, and sweep the repository's prose (rulebooks remove-first)"
    status: pending
  - id: step-3
    content: "Step 3: sweep the contents of every vibe/ plan and the promptforge-docs guide"
    status: pending
isProject: false
---

# Plugin Terminology Rename

<product-contract>

## Product Requirements

Today the activation unit is called a capability: the unit a prompt declares, which adds tools and an optional Lua prelude to a run. The same word also names the VFS `Access`, a model's capabilities, and Tauri's window permissions. The unit itself also goes by "pack" and, in older plans, "addon". This work renames the unit to Plugin, a capitalized defined term like Engine, Harness, and Host, everywhere in the `promptforge` repository and in the `promptforge-docs` guide. It changes terminology only: behavior stays the same, apart from the renamed frontmatter key, JSON field, and message texts.

- Problem and users:
  - Code, rules, and plans give one word four meanings. Examples: "Capabilities are delivered in packs" in `crates/harness-internal/capabilities/src/capability.rs` lines 4-8; "the RAII capability" in `crates/promptforge-internal/vfs/src/handle.rs` line 6; the role keywords in `crates/promptforge-internal/model-client/src/model/options.rs` lines 97-142; Tauri's window capability in `crates/workshop/desktop/src/main.rs` lines 280-286. The main readers are coding agents, then engineers.
  - Prompt authors write the key in frontmatter, and models read the refusal messages that name it.
- Goals:
  - The root `AGENTS.md` defines Plugin as its fourth term.
  - Every name for the activation unit says Plugin: types, modules, functions, fields, the crate, files, the frontmatter key, Workshop JSON, messages, comments, docs, the guide, and every plan's contents.
  - Other programs' plugins are named with their owner: Tauri plugin, ProseMirror plugin, esbuild plugin, NSIS plugin.
  - "capability" remains only in its other senses.
  - A guard stops rulebooks from calling a Plugin a capability or writing a lowercase "plugin".
- Non-goals:
  - Changing what a Plugin does or how activation works.
  - Renaming the other senses of "capability".
  - Changing any repository other than `promptforge` and `promptforge-docs`.
- Success criteria:
  - Every gate in the Project Survey passes after each step.
  - Searching for `capabilit` finds only these: senses kept by design (Technical Design), verbatim quotations, plan file names and references to them, and the repository copy of this plan.
  - The extended guard passes on the repository, and fails when a rulebook reintroduces "capability" or a lowercase "plugin".
- Constraints:
  - Commit directly to `master` in both repositories, with no feature branch. Nothing is pushed; Vinnie pushes.
  - Plan files are never renamed or moved, because commit messages carry their names in `Plan:` trailers. Only their contents change, and every reference to a plan file name stays exactly as written.
  - This plan governs over every rulebook it conflicts with: any `AGENTS.md`, `.cursor/rules/*.mdc`, crate `## Invariants` docs, type- or trait-level `# Invariants` sections, and module docs that state rules. When a rulebook conflicts with the plan, or the rename makes it wrong, fix it in this order of preference:
    1. Remove the material, when it is wrong after the rename or only restates what code, a test, or the root Definitions say.
    2. Edit it in place, when the rule must still hold.
    3. Add text only when nothing can be removed or edited. The only planned addition is the Plugin bullet in the root `AGENTS.md`.

    Every removal is listed in that step's residual report, with file, line, and removed text.
  - Prose edits in code files change comment text in place and never add a line. A code file shrinks only by its reported rulebook removals. The new checks in `crates/workshop/ui/test/docs-claims.mjs` are the one exception.
  - Engine docs call the code that steps a run "the caller" and never name the Harness, including when they describe Plugin activation.
  - Verbatim quotations of people stay exactly as written.
  - `vibe/scratch/` stays untouched; `vibe/.gitignore` ignores it.
- Open questions: None

## Functional Specification

Prompt authors declare Plugins under `plugins:`. The Host registers Plugins in a `PluginRegistry`, the Harness activates the ones a prompt declares, and Workshop lists a prompt's declared Plugins. Every text a user or model sees that named a capability now names a Plugin, in the exact words below.

- Actors and workflows:
  - A prompt author lists the Plugins a prompt requires under `plugins:`. Each entry is an id string, or a map with `ref`, `optional`, and `config`. This is the same shape `capabilities:` has today (`crates/promptforge-internal/parser/src/contract.rs` lines 163-182).
  - The Host builds a `PluginRegistry` and hands it to `Harness::new` (`crates/harness-internal/runner/src/harness.rs` lines 133-139). The Harness activates each declared Plugin when the run starts.
  - Workshop's prompts route returns the declared Plugins under `plugins` (`crates/workshop/server/src/routes/prompts.rs` lines 55-98). The run panel parses them and renders one row per Plugin (`crates/workshop/ui/src/services/run-api.ts` lines 20-251; `crates/workshop/ui/src/parts/run/run-rows.ts` lines 7 and 133-141).
- Inputs and outputs:
  - The frontmatter key `plugins:` replaces `capabilities:`, with no alias. A prompt that still says `capabilities:` fails to parse with serde's unknown-field error, which lists `plugins` among the expected keys, because the frontmatter denies unknown fields (`crates/promptforge-internal/parser/src/build-frontmatter.rs` lines 40-41).
  - The Workshop JSON field `plugins` replaces `capabilities`, in the server and the UI together.
  - Id text does not change: `promptforge/web` stays `promptforge/web`, so tool ids and run records are unchanged. Docs describe the grammar as `namespace/plugin` for a Plugin and `namespace/plugin/name` for a tool.
- States and validation:
  - Parsing, activation, conflict checks, and service checks behave exactly as they do today.
  - A Host service id is valid when it parses as a global name with exactly two segments. Its check no longer goes through the Plugin id parser (Technical Design).
- Errors and recovery:
  - Every user-facing and model-facing message that named a capability names a Plugin. The exact texts are listed under "Message texts" in Technical Design.
  - Recovery for a prompt that still uses the old key is to rename the key. The error message names the expected keys.
- Security and privacy behavior:
  - None changed.
- Acceptance criteria:
  - A prompt using `plugins:` parses and runs exactly as the same prompt with `capabilities:` did before.
  - A prompt using `capabilities:` fails to parse, and the error lists `plugins`.
  - Workshop shows a prompt's declared Plugins from the `plugins` field.
  - Every message in Technical Design's "Message texts" appears with its new text, and the tests that pin those texts assert the new text.
  - The root `AGENTS.md` defines Engine, Harness, Host, and Plugin, and the extended guard passes.

</product-contract>
<implementation-contract>

## Technical Design

This is a rename: the module structure and dependency directions stay as they are. One crate directory moves and the crate gets a new name, two facade modules get new names, and every public item that names the unit is renamed. The Engine's public surface listing is regenerated. Senses of "capability" other than the activation unit keep their names.

- Architecture:
  - Unchanged. `harness-plugins`, at `crates/harness-internal/plugins/`, takes the place of `harness-capabilities` in the dependency graph. The Harness facade and `harness-runner` depend on it, and it depends on the `promptforge` facade.
- Modules and interfaces (the name map):
  - Engine (`crates/promptforge/`, `crates/promptforge-internal/{types,parser,engine,lua}`):
    - Module `promptforge_types::capabilities` and facade module `promptforge::capabilities` become `plugins` (`crates/promptforge-internal/types/src/lib.rs` line 39; `crates/promptforge/src/lib.rs` lines 113-124).
    - `CapabilityId`, `CapabilityIdError`, and `CapabilityIdErrorKind` become `PluginId`, `PluginIdError`, and `PluginIdErrorKind`.
    - In `crates/promptforge-internal/types/src/names.rs`, `GlobalName::pack()` becomes `GlobalName::plugin()` and `GlobalName::capability_prefix()` becomes `plugin_prefix()`. `CapabilityId::pack()` becomes `PluginId::name()`, and `ToolId::capability()` becomes `ToolId::plugin()` (`types/src/tools/ids.rs` line 59).
    - `Prelude::capability()` and its field become `plugin`.
    - `CapabilityDecl`, and its visitor, become `PluginDecl`. `Frontmatter::capabilities()`, and its serde field, become `plugins`. `parse_capability_id`, `check_distinct_capabilities`, and `check_slot_capabilities` become `parse_plugin_id`, `check_distinct_plugins`, and `check_slot_plugins`.
    - `CapabilityConflict` becomes `PluginConflict`, and `MissingService::capability` becomes `MissingService::plugin` (`crates/promptforge-internal/engine/src/execute/requirements.rs`).
  - Harness (`crates/harness/`, `crates/harness-internal/{capabilities,runner}/`, `crates/harness-web/`):
    - `Capability`, `CapabilityError`, `CapabilityErrorKind`, and `CapabilityRegistry` become `Plugin`, `PluginError`, `PluginErrorKind`, and `PluginRegistry`.
    - `ServiceGap::capability` becomes `plugin`.
    - The facade module `harness::capability` becomes `harness::plugin` (`crates/harness/src/lib.rs` lines 25-50).
    - The `capabilities` parameter and field of `Harness` become `plugins`.
    - The test helpers `StubCapability` and `HoldCapability` become `StubPlugin` and `HoldPlugin`.
  - Workshop:
    - `fn capabilities()` and its field in `crates/workshop/server/src/agents.rs` become `plugins`.
    - `CapabilityDto` becomes `PluginDto` (`routes/prompts.rs`). `RunContractCapability` becomes `RunContractPlugin` (`ui/src/services/run-api.ts`).
  - Test functions whose names contain "capability" in this sense are renamed to say "plugin": about 32 in the Engine crates and about 38 elsewhere.
  - Names that stay:
    - `Contribution`, `Activation`, `activate`, `RunServices`, `HostServices`, `ServiceId`, and `ServiceKey`.
    - Every other sense of "capability":
      - the whole `crates/promptforge-internal/vfs/` crate, and "access capability" in comments elsewhere
      - `ModelBinding::capabilities` and `with_capabilities`, and Lua `model.capabilities` (`crates/promptforge-internal/lua/src/models-userdata.rs` lines 50 and 111)
      - "thinking capability" and "frontier-capability"
      - everything under `crates/gateway/`
      - Tauri's `CapabilityBuilder`, `window_capability`, and the generated schemas in `crates/workshop/desktop/`, plus `crates/workshop/ui/src/parts/chrome/window-chrome.ts` line 33
      - the gateway connection "capability" in `crates/workshop/gateway/src/{binding.rs,resolve.rs,test_gateway.rs}`, `crates/workshop/desktop/src/gateway/identity.rs`, and `crates/workshop/server/src/fixtures.rs`
    - The Workshop workspace sibling folder name `"plugins"` in `crates/workshop/workspace/src/workspace_file/siblings.rs` line 14, which already reads in the Plugin sense.
  - Ambiguous uses, decided by sense and reported rather than guessed:
    - `crates/promptforge-internal/types/src/cancel.rs` line 11
    - "capability description" in `crates/promptforge-internal/lua/src/handles.rs` and `lua/src/tools/userdata.rs`
    - `crates/promptforge-internal/engine/src/execute/error.rs` line 21
    - the "echo capability" fixture strings
- File and public API changes:
  - Crate:
    - `crates/harness-internal/capabilities/` moves to `crates/harness-internal/plugins/`, and the package `harness-capabilities` becomes `harness-plugins`.
    - The root `Cargo.toml` changes on line 3 (the member) and line 48 (the workspace dependency).
    - Dependents change in `crates/harness/Cargo.toml` line 13 and `crates/harness-internal/runner/Cargo.toml` line 15.
    - `Cargo.lock` is rewritten.
    - These `crates/build-xtask/` tests name the crate or its path and change too: `src/harness_bans-tests.rs`, `src/product/tests.rs`, `src/product/harness_tests.rs`, `src/product/container_tests.rs`, and `src/test_support_leak-tests.rs`.
  - Files:
    - `crates/promptforge-internal/types/src/capabilities.rs` and `capabilities-tests.rs` become `plugins.rs` and `plugins-tests.rs`.
    - `crates/promptforge-internal/parser/src/contract/tests-capabilities.rs` becomes `tests-plugins.rs`.
    - `crates/promptforge/tests/suite/capabilities.rs` becomes `plugins.rs`.
    - `crates/harness-internal/plugins/src/capability.rs` and `capability-tests.rs` become `plugin.rs` and `plugin-tests.rs`.
    - Every `mod` and `#[path]` line that names these files is updated.
  - Engine facade surface: `crates/promptforge/public-api.txt` is regenerated. About 67 of its 69 matching lines change; `ModelBinding::capabilities` and `with_capabilities` stay.
  - Service ids: `HostServices::provide` stops validating with `CapabilityId::parse` (`crates/harness-internal/capabilities/src/service.rs` line 161). It instead accepts a literal that parses as a `GlobalName` with exactly one `/`. `ServiceError::InvalidId` keeps only its `id` field: the `source` field goes, so no error calls a service id a Plugin id. Its Display text stays `service id {id} is not a namespace/name id`.
  - Frontmatter in `prompts/research-person.md`, `crates/workshop/agents/agents/chat.md`, and every Rust and `.mjs` test fixture changes from `capabilities:` to `plugins:`.
- Data, persistence, failure, security, and privacy constraints:
  - Run records and events are unchanged. No serialized field is named after capabilities. A `PluginId` serializes as the same `namespace/name` string a `CapabilityId` did. `ToolDescriptor`'s field is named `conflicts` (`crates/promptforge-internal/types/src/tools/descriptor.rs` lines 22-43).
  - The Workshop JSON field changes in the server and the UI in the same commit.
  - Message texts. These are model-facing, and each test that pins one changes in the same commit as the string. `{...}` marks an interpolated value.
    - `- missing required Plugin: {id}` (refusal notice; was "missing required capability")
    - `- conflicting Plugins: {first} and {second} cannot be activated together; declare one or the other`
    - `invalid frontmatter: Plugin {id} is declared more than once under plugins`
    - `invalid frontmatter: tool alias '{alias}' names {path}, whose Plugin {id} is declared optional; a tool slot requires its Plugin`
    - ``invalid Plugin id `{text}`: a Plugin id has exactly 2 segments (namespace/plugin)``
    - `invalid Plugin id: {reason}`, whose segment-count reason is `a Plugin id must have exactly 2 segments (namespace/plugin)`
    - the global-name segment error: `must have exactly 2 segments (namespace/plugin) or 3 (namespace/plugin/name)`
    - the frontmatter entry type error: ``a Plugin id string or a map with `ref`, `optional`, and `config` ``
    - prelude errors: ``Plugin `{id}`: its prelude defines ...`` and `... is read-only inside a Plugin prelude; cannot set`. The prelude chunk name in tracebacks becomes `@plugin:{id}`.
    - registry errors: `a Plugin with id {id} is already registered`, and `Plugin id {id} was rejected: it differs from the registered id {existing} only by '-', '_' or '.' punctuation`
    - ``tool `{alias}` ({tool}) is not among the run's activated Plugins``
    - Unchanged: `- {plugin} needs {service}, and this host provides none`, and `User input is unavailable in this host; continue without it.`
    - The operator log messages in `crates/harness-internal/capabilities/src/activation.rs` say "Plugin", and their structured field `capability` becomes `plugin`.
    - Any other message that says "capability" in the activation-unit sense says "Plugin", capitalized, with the rest of its wording unchanged.

</implementation-contract>
<verification-contract>

## Testing Plan

Existing tests carry most of the proof: they are renamed, and their pinned texts move to the new wording, so the whole suite proves behavior is unchanged. A few new unit tests pin the renamed key and the service-id check. The extended guard keeps rulebooks clean. Scripted residual searches and line checks cover prose that no test reads.

- Unit:
  - The parser accepts a `plugins:` entry in both the string and map forms. It rejects a `capabilities:` key with an error that lists `plugins`.
  - `HostServices::provide` rejects a three-segment literal and a malformed literal as `ServiceError::InvalidId`, and still accepts a valid two-segment literal.
  - The refusal notice, the parser messages, the prelude errors, and the registry Display text assert the new texts listed in the Functional Specification.
- Integration and end-to-end:
  - The existing suites of `promptforge`, `promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `harness-plugins`, `harness-runner`, `harness`, `harness-web`, `workshop-server`, and `workshop-agents` pass under their renamed tests and fixtures.
  - `crates/promptforge/tests/suite/shipped.rs` parses every prompt under `prompts/`.
  - `crates/workshop/server/src/routes/prompts-tests.rs` asserts the `plugins` JSON field. `crates/workshop/ui/test/run-api.mjs` and `run-panel.mjs` assert the `plugins` contract and its row.
- Regression, security, and performance:
  - `crates/workshop/ui/test/docs-claims.mjs` gains two checks over every rulebook it already reads. Both strip inline code first, as the existing checks do.
    - A lowercase standalone "plugin" or "plugins" fails, except in "Tauri plugin", "ProseMirror plugin", "esbuild plugin", and "NSIS plugin". Hyphenated, underscored, and path-joined names such as `tauri-plugin-dialog` and `harness-plugins` pass through the same lookarounds the "engine" and "harness" check uses.
    - "capabilit" in any case fails, except in "access capability", "Tauri capability", and "model capabilities". Each failure says the Plugin is never called a capability.
    - The test title becomes "rulebooks use Engine, Harness, Host, and Plugin as the root AGENTS.md defines them".
  - Residual reports, written as scratch, list every remaining `capabilit` hit, with its reason, in `crates/`, `prompts/`, the root files, each `vibe/` month folder searched directly, and the guide chapters.
  - Comment-only check: in a code file, every changed prose line is a comment line. The file's line count is unchanged, apart from its reported rulebook removals.
  - Plans check:
    - `git status` shows only modified files under `vibe/`: no renames, additions, or deletions.
    - Nothing under `vibe/scratch/` changes.
    - Every plan-file name inside plan contents is unchanged.
  - Guide check: no link targets a removed heading slug, and every new slug matches a heading.
- Exit criteria:
  - Every command in the Project Survey's build, test, lint, format, and docs entries passes.
  - The guide site builds.
  - Each residual report holds only explained hits.

</verification-contract>
<decision-record>

## Decision Record

The owner chose Plugin over Addon and Extension, and asked for one defined term applied throughout two repositories. The work goes straight onto `master`. Plan files keep their names, and rulebooks yield to this plan, with removal preferred to editing and editing to adding. The agent settled the remaining naming and message choices, recorded below with their reasons.

- Decisions:
  - **Plugin becomes the fourth defined term, capitalized everywhere.** User: "I want a top to bottom rewrite of the entire project to settle on the word Plugin capitalized. I want the root AGENTS.md which lists Engine, Host, and Harness, to add a forth term Plugin".
  - **The word is Plugin.** User: "how about Plugin instead of Addon". Plugin matches the owner's own phrase "the plugins directory" from the design conversation, and Workshop already reserves a `plugins` workspace folder (`crates/workshop/workspace/src/workspace_file/siblings.rs` line 14). Its other uses in the repository are vendor names that are always qualified.
  - **Scope is the `promptforge` repository plus the `promptforge-docs` guide, and every plan in `vibe/`, the dated month folders included.** These were the owner's selections when asked.
  - **Commit straight to `master`.** User: "use master not plugin-rename".
  - **Plan files keep their names.** User: "dont rename plan files, the filenames are baked into commit messages. you can only change the contents of plan files." Code files, test files, and the crate directory are still renamed.
  - **Rulebooks yield to the plan, in the order remove, then edit, then add.** User: "when an AGENTS.md conflicts with the plan, prefer the plan, and prefer removing material from AGENTS.md over editing, and prefer editing over adding. Same for prose rules in source files like INVARIANTS or whatever". The root `AGENTS.md` therefore gets one new bullet and edits elsewhere (Execution Instructions, Step 2).
  - **No alias for `capabilities:`.** Only two prompts in the repository use the key, and both change. The parser's unknown-field error names `plugins`. Confidence: medium-high.
  - **The id's second segment is called `plugin`**, and `GlobalName::pack()` becomes `plugin()`. "pack" is a second name for the same unit, and the rename retires it. Confidence: medium.
  - **Messages write "Plugin" capitalized**, matching the defined term. Confidence: medium.
  - **Service ids get their own two-segment check, and `ServiceError::InvalidId` drops its `source`.** Otherwise the error would call a service id a Plugin id, which breaks the one-meaning rule. Removing the field is smaller than inventing a new error type. Confidence: medium.
  - **Few, light steps.** User: "keep steps light and few".
- Rejected alternatives:
  - **Addon.** It is unused in the code, but it sounds optional, and the owner moved on to Plugin. Revisit: never, unless Plugin is withdrawn.
  - **Extension.** It collides with file extensions in the repository's own prompt discovery (`crates/workshop/agents/src/discovery.rs` line 47), wire-format and HTTP request extensions (`crates/harness-gateway-client/src/wire/parse.rs` line 20; `crates/shared-loopback/src/peer.rs` lines 22-29), CodeMirror and TipTap extensions in Workshop's editor, and MCP protocol extensions. Revisit: if Workshop's stubbed Extensions view (`crates/workshop/ui/src/parts/menu/stubs.contribution.ts` line 79) becomes the main place users manage Plugins.
  - **A feature branch.** The owner chose `master`. Revisit: never for this work.
  - **Renaming the four plan files whose names contain "capabilit".** Commit trailers name them. Revisit: never.
  - **New "Using the terms" bullets for Plugin and for the other senses of "capability".** Editing the existing other-senses bullet, plus the guard's allowlists, covers them with no added rule text. Revisit: if writers keep confusing the senses after the guard lands.
  - **Retired-symbol seeds for the old identifiers** in `crates/build-xtask/src/engine_guards.rs`. The compiler already rejects any use of a removed name, and the scan covers only Engine sources. Revisit: if an old identifier is reintroduced as a new item.
- Assumptions, risks, and notes:
  - **The plan's own copy.** The run copies this plan into `vibe/` and writes `vibe/ACTIVE`. Both are excluded from the plans sweep, because the plan names the old identifiers on purpose.
  - **Hidden folders.** `vibe/.cursorignore` (`**/vibe/*/`) hides `vibe/2026-07/`, `vibe/2026-08/`, and `vibe/2026-09/` from searches that start at the repository root, so a root-level residual search would pass falsely. Every check of the plans searches each folder directly, or runs `rg`, which ignores that file.
  - **Wrong sense calls.** A kept sense could be renamed, or a Plugin could be left behind. The kept and ambiguous lists in Technical Design, plus the residual reports, catch these.
  - **Lockfile.** Every `--locked` command fails until `Cargo.lock` is rewritten after the crate is renamed.
  - **Docs lag.** The guide's quoted messages are stale between the code step and the guide step. No test reads them.
  - **External references.** References to the old names outside the two repositories stay as they are: cabinet reports, the `promptforge-design` repository, `~/.cursor/plans`, and the sibling checkouts `promptforge2/` and `promptforge3/`.
  - **The shell.** If the shell cannot run commands on this machine, edits still proceed, and Vinnie runs the gates.

### Deferred and Out of Scope

- Deferred: **the Plugin API redesign** (a manifest as data, a Plugin source that loads Plugins from configuration, lazy per-run creation, teardown, and configuration tiers). Revisit after this rename lands.
- Out of scope:
  - the `tools-public` repository, which uses "capability" in its plain-English sense
  - the `promptforge-design` repository and cabinet reports
  - the sibling checkouts `promptforge2/` and `promptforge3/`
  - the gateway's model-capability vocabulary

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command:
  - Default build: `cargo build --locked -p gateway`. `gateway` is the workspace's only default member.
  - The crates this plan touches: `cargo build --locked -p promptforge -p promptforge-engine -p harness-capabilities -p harness-runner -p harness -p harness-web -p workshop-server`. After the crate rename, `harness-capabilities` is `harness-plugins`.
  - Headless gateway shape: `cargo check -p gateway --no-default-features`.
- Focused test command pattern:
  - `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`, for example `cargo nextest run --locked -p promptforge-parser --all-features plugin`.
  - For `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features`.
  - A UI test file: `node --test test/<file>.mjs` from `crates/workshop/ui`.
- Component test command pattern:
  - `cargo nextest run --locked -p <crate> --all-features`, with the same `--all-features` exception for the three Workshop crates.
  - Structural checks: `cargo test -p build-xtask`.
  - A UI package: `npm test --workspace ui` from `crates/workshop`.
- Full-suite test command:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - UI: `npm test --workspaces --if-present` from `crates/workshop`, and `npm test` from `crates/gateway/config-ui/ui`.
  - No doctest run is needed, because a `build-xtask` check bans compiled code blocks in doc comments.
- Linter command:
  - With `CARGO_BUILD_WARNINGS=deny` set (PowerShell: `$env:CARGO_BUILD_WARNINGS="deny"`), run both `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`.
  - Run no standalone `cargo check --workspace`. The one extra check is `cargo check -p gateway --no-default-features`.
  - UI typecheck: `npm run typecheck --workspaces --if-present` from `crates/workshop`.
- Formatter check command: `cargo fmt --all --check`.
- Docs command:
  - With `RUSTDOCFLAGS=-D warnings` set (PowerShell: `$env:RUSTDOCFLAGS="-D warnings"`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`.
  - Facade surface: `cargo +nightly-2026-09-05 xtask api --check`. `cargo +nightly-2026-09-05 xtask api --bless` rewrites `crates/promptforge/public-api.txt`. The pin lives in `crates/build-xtask/src/api/toolchain.rs`.
  - Guide: `cargo xtask site --books-only`, with `PROMPTFORGE_DOCS` set to `c:\Users\Vinnie\cursor\promptforge-docs` (PowerShell: `$env:PROMPTFORGE_DOCS="c:\Users\Vinnie\cursor\promptforge-docs"`).
- Test placement and naming conventions:
  - Unit tests sit in an inline `#[cfg(test)] mod tests`, or in a sibling `foo-tests.rs` wired as `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;` (for example `crates/promptforge-internal/types/src/capabilities-tests.rs`).
  - Each crate has one integration binary: `tests/it/main.rs` (in `harness-runner`, `harness-capabilities`, and `workshop-server`) or `tests/suite/main.rs` (in `harness` and `promptforge`). It holds one module per topic plus a `support.rs`. Prompt fixtures sit under `tests/prompts/`.
  - The Engine's execution tests live in `crates/promptforge-internal/engine/src/execute/tests/`, each declared in `execute/tests.rs`.
  - Test names are snake_case behavior sentences, such as `activation_receives_the_runs_own_services`.
  - UI tests are `node --test` `.mjs` files in `crates/workshop/ui/test/`. `docs-claims.mjs` checks the Definitions vocabulary in every `AGENTS.md`, every `.cursor/rules/*.mdc`, and the `//!` crate doc of every `lib.rs` or `main.rs` that carries `## Invariants`. It strips fenced and inline code first, and skips the root `AGENTS.md` Definitions section.
- Directory map:
  - `crates/`: every crate. The top level holds the `promptforge` and `harness` facades, the Host-side crates `harness-web` (which provides the `promptforge/web` Plugin) and `harness-gateway-client`, the shared crates, and the build tooling (`build-xtask`, `build-ceiling`, `build-user-guide`, and others).
  - Manifestless containers hold each family's private crates: `promptforge-internal/` (`types`, `parser`, `engine`, `lua`, `vfs`, `model-client`), `harness-internal/` (`runner`, `capabilities`), `gateway/`, and `workshop/`. `workshop/` also holds the TypeScript packages `ui`, `look`, and `platform`, with one npm install at `crates/workshop`.
  - `prompts/`: example prompts, each of which must parse (`crates/promptforge/tests/suite/shipped.rs`).
  - `guide/`: the docs site's chrome, landing page, and `books/<book>/book.toml`. The chapter text lives in the `promptforge-docs` checkout.
  - `vibe/`: dated plans at the top level, older months in `2026-07/`, `2026-08/`, and `2026-09/`, and the gitignored `scratch/`. `vibe/.cursorignore` hides the subfolders from root-level Cursor search.
  - `.github/workflows/`: CI, release, site, and nightly jobs. `.cursor/rules/`: two Workshop rule files.
  - `promptforge-docs` (`c:\Users\Vinnie\cursor\promptforge-docs`) is its own git repository with default branch `master`. Chapters sit under `src/<book>/`, alongside `src/introduction.md`. Its `SUMMARY.md` and the guide exports are generated at build time into `target/`, never committed.
- Component boundaries:
  - The Engine is sans-I/O, and the Harness reaches it only through the `promptforge` facade.
    - `promptforge-engine` depends on `lua`, `parser`, `model-client`, `types`, and `vfs`.
    - `promptforge-parser` depends on `lua` and `types`.
    - `promptforge-types` depends on nothing.
  - `harness-capabilities` depends on `promptforge`. `harness-runner` depends on it and on `promptforge`, and the `harness` facade re-exports both.
  - `harness-web` and `harness-gateway-client` depend on the `harness` facade.
  - Workshop is a Host. `workshop-server` and `workshop-agents` depend on the Harness crates and `promptforge`.
  - `cargo test -p build-xtask` enforces the product and container boundaries, the Harness tokio ban, the `test-support` leak guard, the retired-symbol scan, the `## Invariants` marker, and the 500-line file ceiling. Several of its tests name the `harness-capabilities` crate and its path.
  - For this plan:
    - Ids and preludes: `crates/promptforge-internal/types/src/capabilities.rs` and `names.rs`.
    - Frontmatter: `crates/promptforge-internal/parser/src/build-frontmatter.rs` and `contract.rs`.
    - The refusal notice: `crates/promptforge-internal/engine/src/execute/requirements.rs`.
    - Prelude errors: `crates/promptforge-internal/lua/src/prelude.rs`.
    - The trait, registry, activation, and services: `crates/harness-internal/capabilities/src/{capability,registry,activation,service}.rs`.
    - The Workshop contract: `crates/workshop/server/src/routes/prompts.rs` and `crates/workshop/ui/src/services/run-api.ts`.
- Conventions summary:
  - Rust 2024 on stable, with dependencies centralized in `[workspace.dependencies]`.
  - Clippy `all` and `pedantic` deny, and so do `unwrap_used` and `expect_used`. Suppressions use `#[expect(..., reason = "...")]`. Rustdoc broken intra-doc links deny, so every doc link to a renamed item must change with it.
  - Source directories are flat, with kebab `foo-bar.rs` siblings wired by `#[path]`. Every file stays under 500 lines.
  - Engine, Harness, and Host are capitalized defined terms. Engine docs call the stepping code "the caller".
  - Error messages are written for a model to read, naming what was required versus what was found.
  - CI commands pass `--locked`, and no build step may dirty the tree.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Rename the code to Plugin [completed]

- Component: Plugin code
- Tests first:
  - Change the tests that pin message texts or JSON fields to the new texts in Technical Design's "Message texts", and to the `plugins` JSON field: `requirements-tests.rs`, `parser/src/contract/tests-capabilities.rs`, `lua/src/prelude-tests.rs`, `prelude-tests-environment.rs`, `capability-tests.rs`, `routes/prompts-tests.rs`, `ui/test/run-api.mjs`, and `ui/test/run-panel.mjs`.
  - Add two unit tests:
    - The parser key test, in `parser/src/contract/tests-capabilities.rs`, which this step renames to `tests-plugins.rs`. It checks that the parser accepts a `plugins:` entry in both the string form and the map form (`ref`, `optional`, `config`). It also checks that a prompt with a `capabilities:` key fails to parse with an error whose text contains `plugins`.
    - The service-id test, in `service-tests.rs`. It checks that `HostServices::provide` rejects a three-segment literal and a malformed literal with `ServiceError::InvalidId`, and still accepts a valid two-segment literal.
  - Run them and confirm they fail.
- Move the crate:
  - `git mv crates/harness-internal/capabilities crates/harness-internal/plugins`, and rename the package to `harness-plugins`.
  - Update the Cargo files and the five `build-xtask` tests listed in Technical Design.
  - Run `cargo update --workspace`.
- Apply the name map and the file renames from Technical Design. Update every `mod`, `#[path]`, `use`, and doc link that names a renamed item, and every frontmatter fixture.
- Split the work across three sub-agents on disjoint files. Each leaves its changes unstaged; the coding agent merges them, builds, and fixes.
  - Engine: `crates/promptforge/` and `crates/promptforge-internal/{types,parser,engine,lua}/`.
  - Harness: `crates/harness/`, `crates/harness-internal/{plugins,runner}/`, `crates/harness-web/`, including `ServiceError::InvalidId`.
  - Workshop: `crates/workshop/{server,agents,ui}/`, `prompts/research-person.md`, and the `build-xtask` tests.
- Prose changes in this step are limited to the words that name a renamed item: doc links, `use` lines, and identifiers in comments. The rest waits for Step 2.
- Run `cargo +nightly-2026-09-05 xtask api --bless`.
- Write a residual report as scratch. Every remaining `Capabilit|capabilit|\.pack\(\)` hit in `crates/` and `prompts/` must be a kept sense, an ambiguous item, or prose left for Step 2.

</step-1>
<step-2>

### Step 2: Define Plugin and sweep the repository's prose [completed]

- Component: Plugin prose
- Tests first:
  - Extend `crates/workshop/ui/test/docs-claims.mjs` (an existing facility) with two checks over every rulebook line it already reads, after its existing stripping of fenced and inline code:
    - A lowercase standalone "plugin" or "plugins" fails, except in "Tauri plugin", "ProseMirror plugin", "esbuild plugin", and "NSIS plugin". Use the same lookarounds as the existing "engine"/"harness" check, so hyphenated, underscored, and path-joined names such as `tauri-plugin-dialog` and `harness-plugins` pass.
    - "capabilit" in any case fails, except in "access capability", "Tauri capability", and "model capabilities". Each failure message says the Plugin is never called a capability.
    - Rename the test to "rulebooks use Engine, Harness, Host, and Plugin as the root AGENTS.md defines them", and update its header comment to match.
  - Run `node --test test/docs-claims.mjs` from `crates/workshop/ui`, and confirm it fails on the root `AGENTS.md` and the crate docs listed below.
- Root `AGENTS.md`: one addition, and edits everywhere else.
  - Edit "Three words have exactly one meaning each" to "Four words".
  - Add this bullet after Host: "**Plugin**: a named unit of tools, and optionally a Lua prelude, that the Host installs for its runs, such as `promptforge/web`. Every tool it adds sits under its two-segment id (`promptforge/web/fetch`). A prompt declares the Plugins it requires under `plugins:` in its frontmatter, and the Harness activates them when the run starts. A Plugin has no other name: never capability, pack, or addon."
  - Edit the "Using the terms" bullet that opens with "Engine" and "Harness" to open: "Engine", "Harness", and "Plugin" mean only the defined terms. Anything else gets a qualified lowercase name: the gateway's speech engine, the database, Rust's built-in test harness, a Tauri or ProseMirror plugin. The rest of that bullet is kept.
  - Edit lines 31 and 33 to say "feature" in place of "capability".
  - Change nothing else in this file.
- Rulebook lines that name the old word: remove first, edit second, and report each removal. The guard reads every crate doc listed here, except the trait-level section.
  - `crates/harness-internal/plugins/src/lib.rs`, including the "depends on no capability provider" invariant
  - the trait-level `# Invariants` on `Plugin` in `crates/harness-internal/plugins/src/plugin.rs`
  - `crates/harness-web/src/lib.rs` lines 1-11
  - `crates/harness-internal/runner/src/lib.rs` line 5
  - `crates/workshop/protocol/src/lib.rs` line 46
  - `crates/promptforge-internal/types/src/lib.rs` lines 15-19
  - `crates/promptforge-internal/lua/src/lib.rs` lines 28-33. Its line 20 says "access capability" and stays.
- In code files, change comment text in place and never add a line. A code file shrinks only by its reported rulebook removals; `docs-claims.mjs` is the one exception. Engine docs call the code that steps a run "the caller", and never name the Harness. Verbatim quotations of people stay as written.
- Sweep the remaining prose with three sub-agents on disjoint files: the Engine crates, the Harness crates, and everything else.
  - What they sweep: comments, `//!` docs, READMEs, Cargo `description` strings, and comments in `.ts`, `.mjs`, `.css`, `.lua`, `.toml`, and `.yml` files.
  - The change: the activation-unit "capability", "capability pack", "pack", and "addon" become "Plugin". For example, "Capabilities are delivered in packs" becomes "Plugins ship in crates now, and as DLLs through an adapter later".
  - Name other programs' plugins:
    - Tauri: the comments in `crates/workshop/desktop/src/main.rs`, `window_state.rs` line 8, `ui/src/parts/chrome/chrome.contribution.ts` line 119, and `workspace-document.contribution.ts` line 10.
    - ProseMirror: `ui/src/parts/chatbox/{chat-box.ts,mention-chip.ts,typeahead-popup.ts,typeahead-popup.css}`, `ui/test/chat-box.mjs`, and `ui/test/typeahead-popup.mjs`.
    - NSIS: `crates/workshop/desktop/installer.nsi`.
    - CI: `.github/workflows/ci.yml` line 355.
  - Leave the kept senses alone, and decide each ambiguous use by its sense, reporting the call.
- Checks:
  - The guard passes.
  - The comment-only and line-count checks pass.
  - Residual reports for `capabilit` and for lowercase `\bplugins?\b` hold only explained hits.

</step-2>
<step-3>

### Step 3: Sweep the plans and the guide [completed]

- Component: Plugin prose
- This step edits prose only and has no failing-test-first shape. The checks below take that role.
- Plans: change contents only, in the top level of `vibe/` and in `vibe/2026-07/`, `vibe/2026-08/`, and `vibe/2026-09/`.
  - Get exact file lists by searching each folder directly.
  - Apply the name map to prose, inline code, and fenced code. "addon DLL" becomes "Plugin DLL", and "addon loader" becomes "Plugin loader".
  - "plug-in" (with a hyphen) becomes "Plugin" where it means the unit. Elsewhere it gives way to the thing's own name. For example, "Host plug-ins" in `vibe/2026-10-04-1-tool-context.md` means the Host-side crates `harness-web` and `harness-gateway-client`.
  - Keep these exactly as written:
    - plan file names and every reference to one, such as `2026-09-28-2-capability-harness-redesign.md`
    - commit hashes, commit subjects, and `Plan:` trailers
    - verbatim quotations of people, mostly the owner's in `2026-09-28-2`, `2026-09-28-5`, and the `2026-09-13-1` decision record
    - other senses, such as model capabilities in the gateway plans and Tauri capabilities in the Tauri migration plan
  - Leave out entirely: `vibe/scratch/`, `vibe/ACTIVE`, and this plan's repository copy.
  - Split the work across three sub-agents: the top level plus `2026-07` and `2026-08`; `2026-09-28-2` plus `2026-09-13-1`; the rest of `2026-09`.
- Guide, in `c:\Users\Vinnie\cursor\promptforge-docs` on `master`:
  - Rename five headings, and update the 18 links that target them:
    - `src/language/04-how-a-prompt-runs.md` line 472, "Capability activation", becomes `#plugin-activation`
    - `05-lua-environment.md` line 723, "Declaring the capability", becomes `#declaring-the-plugin`
    - `12-tools.md` line 85, "Declaring capabilities", becomes `#declaring-plugins`
    - `12-tools.md` line 144, "Capability ids and tool paths", becomes `#plugin-ids-and-tool-paths`
    - `13-web-fetch-and-search.md` line 5, "The web capability", becomes `#the-web-plugin`
  - Sweep the language chapters `01`, `02`, `04`, `05`, `12`, `13`, `16`, and `17` with two sub-agents:
    - Change the 23 fenced `capabilities:` examples to `plugins:`.
    - Change `namespace/pack` to `namespace/plugin`.
    - Copy every quoted message exactly from Technical Design's "Message texts".
  - Leave alone: the gateway chapters, `src/introduction.md`, `10-models.md`, "thinking capability", and `handle.capabilities`.
  - Commit `promptforge-docs` first, as its own commit, before this step's `promptforge` commit.
- Checks:
  - Each `vibe/` folder's residual report holds only explained hits.
  - `git status` in `promptforge` shows only modified files under `vibe/`, and nothing under `vibe/scratch/`.
  - In the guide, no link targets a removed heading slug, and every new slug matches a heading.
  - `cargo xtask site --books-only` passes with `PROMPTFORGE_DOCS` set.

</step-3>

</execution-plan>
