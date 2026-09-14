---
name: Dokuman All Crates
overview: Run the Dokuman documentation pipeline on each of the 8 promptforge crates sequentially, placing the final user guide in each crate's directory.
todos:
  - id: crate-core
    content: Run Dokuman on promptforge-core -> user-guide-promptforge-core.md
    status: completed
  - id: crate-core-tests
    content: Run Dokuman on promptforge-core-tests (or skip if test-only)
    status: completed
  - id: crate-cli
    content: Run Dokuman on promptforge-cli -> user-guide-promptforge-cli.md
    status: completed
  - id: crate-gateway
    content: Run Dokuman on promptforge-gateway -> user-guide-promptforge-gateway.md
    status: completed
  - id: crate-mcp-server
    content: Run Dokuman on promptforge-mcp-server -> user-guide-promptforge-mcp-server.md
    status: completed
  - id: crate-tool-picker
    content: Run Dokuman on promptforge-tool-picker -> user-guide-promptforge-tool-picker.md
    status: completed
  - id: crate-webfetch
    content: Run Dokuman on promptforge-webfetch -> user-guide-promptforge-webfetch.md
    status: completed
  - id: crate-dev
    content: Run Dokuman on promptforge-dev -> user-guide-promptforge-dev.md
    status: in_progress
isProject: false
---

# Dokuman on All PromptForge Crates

Run [dokuman.md](tools-public/tools/dokuman.md) on each crate under [promptforge/crates/](promptforge/crates/), one at a time. Output goes into the crate directory itself as `user-guide-{crate-name}.md` (overriding the default cabinet output routing).

## Crates (processing order)

1. `promptforge-core` - parser, error handling, core types
2. `promptforge-core-tests` - test-only crate (may be thin; skip if no user-facing capability)
3. `promptforge-cli` - CLI binary
4. `promptforge-gateway` - gateway/artifact resolution
5. `promptforge-mcp-server` - MCP server (largest crate, ~70+ files)
6. `promptforge-tool-picker` - tool selection logic
7. `promptforge-webfetch` - web fetching
8. `promptforge-dev` - dev utilities

## Per-crate execution

Each crate follows the full Dokuman 9-step pipeline:

- **Step 0 (Intake):** Target = `promptforge/crates/{crate}/`
- **Steps 1-6:** Subagents for recon, extract, consolidate, tier, verify, evidence prep. All intermediates go to `cabinet/_scratch/dokuman-{crate-name}/`
- **Step 7 (Write):** Single writer subagent produces draft
- **Step 8 (Audit):** Main audits and writes final file to `promptforge/crates/{crate-name}/user-guide-{crate-name}.md`

## Routing

Normal cabinet routing throughout the pipeline:

- **scratch** intermediates: `cabinet/_scratch/dokuman-{crate-name}/` (standard)
- **research** if any: `cabinet/_research/` (standard)

Only the final guide departs from default routing. Instead of `cabinet/_output/`, it goes directly into the crate directory as `promptforge/crates/{crate-name}/user-guide-{crate-name}.md`.

## Sequencing

Strictly one crate at a time. Complete all 8 steps for crate N before starting crate N+1. Within a single crate's Step 2 (Extract), extraction subagents can run in parallel as the tool allows.

## Edge case: promptforge-core-tests

If recon reveals this crate contains only test infrastructure with no standalone user-facing capabilities, skip it and note that in the response.
