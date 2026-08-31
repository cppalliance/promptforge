---
name: Republish crates to crates.io
overview: Bump versions, flip 7 infrastructure crates to publishable, verify the whole set with dry-runs in dependency order, and hand the user an ordered publish script to execute with their crates.io token.
todos:
  - id: manifests
    content: Bump versions, flip 7 infra crates to publishable, update workspace dep versions
    status: completed
  - id: verify
    content: cargo check workspace and dry-run all 26 crates in level order
    status: completed
  - id: commit
    content: Commit manifest changes as one commit
    status: completed
  - id: script
    content: Generate publish-all.ps1 with ordered publishes and inter-level sleeps
    status: completed
isProject: false
---

# Republish PromptForge crates on crates.io

## Audit findings (research complete)

- 7 crates are on crates.io at 0.1.0 (published 2026-08-12): `promptforge-cli`, `promptforge-core`, `promptforge-dev`, `promptforge-gateway`, `promptforge-mcp-server`, `promptforge-tool-picker`, `promptforge-webfetch`
- Local `promptforge-gateway` is already 0.3.0; the workspace was heavily refactored after the initial publish, extracting 12+ new crates that were never published
- crates.io requires every dependency (including optional and build deps) to exist in the registry, so the `publish = false` infra crates in the dependency closure must be flipped and published first
- User decisions: flip all needed infra crates; agent does bumps + dry-runs, user runs the final `cargo publish` commands

## Step 1: Manifest edits (one commit)

In [Cargo.toml](C:/Users/Vinnie/cursor/promptforge/Cargo.toml) `[workspace.dependencies]` (lines 13-36), update version requirements to match what will be published:

- `promptforge-core` 0.1.0 -> 0.2.0, `promptforge-tool-picker` -> 0.2.0, `promptforge-webfetch` -> 0.2.0 (already-published crates get a minor bump; 0.x semver treats minor as breaking)
- `promptforge-stt` 0.0.0 -> 0.1.0 (0.0.0 is a placeholder, not worth publishing)
- `promptforge-gateway` stays 0.3.0; everything else publishes at its current local version

In per-crate manifests under [crates/](C:/Users/Vinnie/cursor/promptforge/crates):

- Remove `publish = false` from: `promptforge-progress`, `promptforge-gateway-loopback`, `promptforge-gateway-config-ui`, `promptforge-gateway-build`, `promptforge-stt`, `promptforge-workshop-server`, `promptforge-transcribe`
- Bump versions in: `promptforge-cli`, `promptforge-core`, `promptforge-dev`, `promptforge-mcp-server`, `promptforge-tool-picker`, `promptforge-webfetch` (0.1.0 -> 0.2.0); `promptforge-stt` (0.0.0 -> 0.1.0)
- Verify each flipped crate has `description` and `license` (all do, per audit) and that any `readme = "README.md"` field points at an existing file - drop the field if the file is missing

Stays `publish = false`: `promptforge-desktop-shell`, `promptforge-workshop`, `promptforge-core-tests`, `make-user-guide` (nothing in the publish closure depends on them).

## Step 2: Verify

- `cargo check --workspace --all-features` (catches version-requirement mismatches after the bumps)
- `cargo publish --dry-run -p <crate>` for every crate in the publish set, in the level order below (dry-run catches missing readmes, unlisted files, and metadata gaps without touching the registry)

## Step 3: Commit

One commit: "Prepare crates.io republication: bump versions, publish infra crates". Note in the body which existing facility was considered per the repo's do-more-with-less rule - not applicable here, but the body should state the publish=false flip rationale.

## Step 4: User runs the publishes

Prerequisite: `cargo login` with a crates.io token (user's credential, never handled by the agent).

Publish in this exact order, waiting 30-60 seconds between levels for the crates.io index to propagate (a "no matching package" error means wait and retry):

- Level 0: `promptforge-progress`, `promptforge-gateway-loopback`, `promptforge-gateway-build`, `promptforge-core-support`, `promptforge-gateway-config`, `promptforge-store`, `promptforge-tools`
- Level 1: `promptforge-gateway-config-ui`, `promptforge-transcribe`, `promptforge-gateway-protocol`, `promptforge-tool-picker`, `promptforge-web-search`, `promptforge-webfetch`
- Level 2: `promptforge-gateway-routing`, `promptforge-gateway-client`, `promptforge-web-search-service`
- Level 3: `promptforge-lua`, `promptforge-gateway-local`
- Level 4: `promptforge-parser`, `promptforge-workshop-server`
- Level 5: `promptforge-core`, `promptforge-stt`
- Level 6: `promptforge-gateway`
- Level 7: `promptforge-cli`, `promptforge-dev`, `promptforge-mcp-server`

The agent delivers a ready-to-run `publish-all.ps1` script emitting these `cargo publish -p <crate>` commands with inter-level sleeps, so the user runs one script instead of 26 commands.

## Step 5: Tag

After the script succeeds: `git tag v2026-08-31-publish && git push upstream master --tags` (user runs the push).

## Risks

- `cargo publish --dry-run` does not catch registry-side rejection of optional deps on freshly published crates; the inter-level sleep mitigates (medium confidence - based on cargo's resolver behavior, not on a live test)
- `promptforge-gateway` pulls Node 22 for the config-ui build script even in a publish verify build; the user's machine has it (the workshop build uses it), so no action needed
- Publishes are irreversible (yank-only); that is why the user runs Step 4