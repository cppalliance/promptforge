---
name: Dokuman All Crates
overview: Run the Dokuman documentation pipeline on each of the 8 runvil crates sequentially, placing the final user guide in each crate's directory.
todos:
  - id: crate-core
    content: Run Dokuman on runvil-core -> user-guide-runvil-core.md
    status: completed
  - id: crate-core-tests
    content: Run Dokuman on runvil-core-tests (or skip if test-only)
    status: completed
  - id: crate-cli
    content: Run Dokuman on runvil-cli -> user-guide-runvil-cli.md
    status: completed
  - id: crate-gateway
    content: Run Dokuman on runvil-gateway -> user-guide-runvil-gateway.md
    status: completed
  - id: crate-mcp-server
    content: Run Dokuman on runvil-mcp-server -> user-guide-runvil-mcp-server.md
    status: completed
  - id: crate-tool-picker
    content: Run Dokuman on runvil-tool-picker -> user-guide-runvil-tool-picker.md
    status: completed
  - id: crate-webfetch
    content: Run Dokuman on runvil-webfetch -> user-guide-runvil-webfetch.md
    status: completed
  - id: crate-dev
    content: Run Dokuman on runvil-dev -> user-guide-runvil-dev.md
    status: completed
isProject: false
---

# Dokuman on All Runvil Crates

Run [dokuman.md](tools-public/tools/dokuman.md) on each crate under [runvil/crates/](runvil/crates/), one at a time. Output goes into the crate directory itself as `user-guide-{crate-name}.md` (overriding the default cabinet output routing).

## Crates (processing order)

1. `runvil-core` - parser, error handling, core types
2. `runvil-core-tests` - test-only crate (may be thin; skip if no user-facing capability)
3. `runvil-cli` - CLI binary
4. `runvil-gateway` - gateway/artifact resolution
5. `runvil-mcp-server` - MCP server (largest crate, ~70+ files)
6. `runvil-tool-picker` - tool selection logic
7. `runvil-webfetch` - web fetching
8. `runvil-dev` - dev utilities

## Per-crate execution

Each crate follows the full Dokuman 9-step pipeline:

- **Step 0 (Intake):** Target = `runvil/crates/{crate}/`
- **Steps 1-6:** Subagents for recon, extract, consolidate, tier, verify, evidence prep. All intermediates go to `cabinet/_scratch/dokuman-{crate-name}/`
- **Step 7 (Write):** Single writer subagent produces draft
- **Step 8 (Audit):** Main audits and writes final file to `runvil/crates/{crate-name}/user-guide-{crate-name}.md`

## Routing

Normal cabinet routing throughout the pipeline:

- **scratch** intermediates: `cabinet/_scratch/dokuman-{crate-name}/` (standard)
- **research** if any: `cabinet/_research/` (standard)

Only the final guide departs from default routing. Instead of `cabinet/_output/`, it goes directly into the crate directory as `runvil/crates/{crate-name}/user-guide-{crate-name}.md`.

## Sequencing

Strictly one crate at a time. Complete all 8 steps for crate N before starting crate N+1. Within a single crate's Step 2 (Extract), extraction subagents can run in parallel as the tool allows.

## Edge case: runvil-core-tests

If recon reveals this crate contains only test infrastructure with no standalone user-facing capabilities, skip it and note that in the response.
