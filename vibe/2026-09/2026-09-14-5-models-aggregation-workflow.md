---
name: Weekly models aggregation workflow
overview: "Turn the placeholder models.yml in cppalliance/promptforge-cloud-providers into the real aggregation pipeline: build the sheet binary from cppalliance/promptforge, run it with the repository's secrets env contract, gate on secret leakage, and publish to a rolling GitHub release only when model content actually changed. Make the repository public so users can download the sheet anonymously, and replace the README with user-facing documentation covering badges, the stable asset URL, and why PromptForge periodically downloads the file."
todos:
  - id: make-public
    content: "Make the repository public so release assets download anonymously"
    status: pending
  - id: rewrite-workflow
    content: "Rewrite .github/workflows/models.yml: build, run, secret-scan gate, content-only change detection, conditional rolling-release publish"
    status: pending
  - id: readme
    content: "Rewrite README.md: badges, stable asset URL, what the sheet is, why PromptForge downloads it"
    status: pending
  - id: validate
    content: Validate the workflow YAML and inline scripts locally
    status: pending
  - id: verify-dispatch
    content: "Land the changes, then operator dispatches manually twice: first run publishes the initial sheet, second run skips publish; verify the asset URL downloads anonymously"
    status: pending
isProject: false
---

# Weekly Models Aggregation Workflow

<product-contract>

## Product Requirements

The aggregation repository already holds the operator's secrets and a hand-written workflow skeleton; this plan turns that skeleton into the real pipeline and documents the result for end users. The pipeline publishes the provider model sheet as a rolling GitHub release asset at a stable URL, only when the model content actually changed. The repository becomes public so that PromptForge installations can download the sheet anonymously, and a new README explains to those users what the periodic download is and why it is harmless.

- Problem and users: the sheet binary landed in `cppalliance/promptforge` but nothing builds or publishes it on a schedule; the operator wants the initial file published via a manual trigger and weekly updates thereafter, without republishing when nothing changed. Two user groups: the operator (sole writer, runs the workflow) and PromptForge end users (anonymous readers of the published sheet, who need to understand the periodic download).
- Goals:
  - A weekly scheduled run plus manual dispatch in `cppalliance/promptforge-cloud-providers` that clones `cppalliance/promptforge`, builds the sheet binary, and runs it against the repository's secrets.
  - Publication as a rolling GitHub release named `models` with the asset overwritten in place, so the stable URL `https://github.com/cppalliance/promptforge-cloud-providers/releases/download/models/cloud-provider-models.json` never changes.
  - No publication when the new sheet's model content is identical to the published one.
  - A secret-scan gate that fails the run if any configured secret value appears in the sheet.
  - The repository made public so the asset downloads anonymously.
  - A README with badges, the stable asset URL, and an explanation of why PromptForge periodically downloads the file.
- Non-goals: changes to the sheet binary or the `cppalliance/promptforge` repository (the binary contract is consumed as-is); Gateway sheet consumption (phase 2); provisioning the remaining provider secrets; any use of the dl.cpp.al upload path from the skeleton.
- Success criteria: a manual dispatch with no prior release publishes the initial sheet; an immediate second dispatch publishes nothing; the stable asset URL downloads without authentication; the weekly cron is registered; a sheet containing a secret value fails the run before any publication; unavailable providers never fail the run.
- Constraints:
  - The operator never hands secret values to any tool or agent; missing secrets simply record their providers `unavailable`. Secret values are never printed in logs (GitHub masks them; the scan gate compares without echoing).
  - The workflow file's existing trigger block (cron `0 12 * * 1` plus `workflow_dispatch`) and its fourteen-entry secrets env block carry over unchanged.
  - The published asset is named `cloud-provider-models.json`, matching the local default output name (2026-09-14, `cppalliance/promptforge` commit `9b7ca5fd`); the workflow passes an explicit output path regardless.
  - The binary's previous-sheet contract stands: `MODELS_SHEET_PREVIOUS_URL` points at the stable asset URL, HTTP 404 means first run, and any other download failure is fatal (DEBT-PMS-1, `cppalliance/promptforge` commit `5945b706`).
- Open questions: None

## Functional Specification

One actor drives the pipeline (the operator, via schedule or manual dispatch), and one silent consumer reads its output (PromptForge installations worldwide, via anonymous GET). The workflow is a linear job: clone, build, run, scan, compare, conditionally publish.

- Actors and workflows: the operator dispatches manually to seed the initial file; the weekly cron keeps it current; end users' PromptForge installs download the published asset periodically with no credentials.
- Inputs and outputs: repository secrets and the previously published asset in; one release asset (`cloud-provider-models.json` on the rolling `models` release) out, plus the Actions log as the run report. The binary emits per-provider failure notes on stderr, so a degraded provider is visible in the log without failing the run.
- States and validation: first run (no published asset - the previous-sheet URL 404s and the comparison is forced to "changed"); unchanged run (normalized model content identical - publish step skipped); changed run (content differs - asset overwritten); leaked-secret run (scan gate fails the job before publication); unreachable previous sheet with a non-404 error (fatal before any write, by the binary's standing contract).
- Errors and recovery: a provider with a missing secret or failed fetch records `unavailable` or `stale` and never fails the job; a failed previous-sheet download fails the job rather than publishing a regressed sheet; a failed publication leaves the previous asset in place.
- Security and privacy behavior: secrets exist only as GitHub Actions secrets in the aggregation repository, mapped into the job's environment; the scan gate greps the sheet for each configured secret value fixed-string and fails on any hit; the sheet itself contains no credentials; the end-user download is a plain anonymous GET of a static public file - nothing about the user is transmitted.
- Acceptance criteria: the success criteria above, verified by the double-dispatch and anonymous-download checks in the Testing Plan.

</product-contract>
<implementation-contract>

## Technical Design

The entire change lives in the aggregation repository: one workflow rewrite, one README rewrite, one visibility flip. The externally observable surface is the rolling-release contract - tag `models`, asset `cloud-provider-models.json`, stable download URL - plus the change-detection semantics that decide when the asset is overwritten.

- Architecture:
  - Job shape: clone `cppalliance/promptforge` (master, depth 1, unauthenticated HTTPS as in the existing skeleton), stable Rust toolchain with a build cache, `cargo build --release --locked -p shared-cloud-providers`, then run the binary with `MODELS_SHEET_PREVIOUS_URL` set to the stable asset URL and an explicit output path under `$RUNNER_TEMP`. On the runner no profile-dir secrets file exists, so the binary's loader notes its absence and uses the environment (loader behavior: `crates/shared-cloud-providers/src/main.rs` in `cppalliance/promptforge`).
  - Change detection: download the published asset (404 forces "changed"), normalize both sheets with `jq -S '.providers | with_entries(.value |= .models)'`, and compare with `cmp -s`; the result flows to the publish step through `$GITHUB_OUTPUT`. Comparing only per-provider model arrays makes timestamps and status flaps (ok/stale) invisible to publication, matching the operator's "actually new" semantics.
  - Publication: `gh release view models || gh release create models`, then `gh release upload models <sheet> --clobber`, each with `--repo cppalliance/promptforge-cloud-providers` (the job never checks out the aggregation repo, so `gh` has no git context to infer it from - the first dispatch failed on exactly this), authenticated with the job's `GITHUB_TOKEN` (`permissions: contents: write`). Overwriting the asset in place is what keeps the URL stable.
  - Secret-scan gate: a bash loop over every key variable in the env block runs `grep -Fq -- "$value" <sheet>` and fails the job on a hit, before the publish step; values are never echoed.
- Modules and interfaces: no code modules. The interfaces are the workflow's env contract (the fourteen secret names already configured, consumed by the binary's provider descriptors), the `MODELS_SHEET_PREVIOUS_URL` variable, the release tag and asset name, and the README's documented stable URL.
- File and public API changes: `.github/workflows/models.yml` (jobs section rewritten; triggers and env block carried over verbatim) and `README.md` (full rewrite) in the aggregation repository; repository visibility flipped to public. No changes in `cppalliance/promptforge`.
- Data, persistence, failure, security, and privacy constraints: the only persisted artifact is the release asset; the previous asset survives until a changed run overwrites it; the binary's failure-isolation and 404-versus-fatal contracts are unchanged and govern the run step.

</implementation-contract>
<verification-contract>

## Testing Plan

A GitHub Actions workflow has no local unit-test harness; verification is static review locally, then the operator's controlled double dispatch against the real service, which exercises every state the design defines.

- Unit: none - no code units are added.
- Integration and end-to-end: local validation parses the workflow YAML and reviews each inline script under `set -euo pipefail` semantics. End-to-end, the operator dispatches manually twice: the first run (no existing release) must create the `models` release and upload the initial sheet; the second run, with no upstream change, must detect identical content and skip the publish step. Both outcomes are verified in the Actions log.
- Regression, security, and performance: the anonymous-download check (`curl -fsSL` of the stable asset URL without credentials, or a private browser window) proves the exact request a user's installation will make; the scan gate is exercised by inspection of the job log (each configured secret compared, none printed); the binary's unavailable-never-fails and 404-versus-fatal behavior is already covered by its integration suite in `cppalliance/promptforge` (`crates/shared-cloud-providers/tests/sheet_binary.rs`).
- Exit criteria: both dispatches behave as specified, the asset downloads anonymously, and the README renders with working badges and links once the repository is public and the first release exists.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Publish target is a rolling GitHub release, not the skeleton's scp upload to dl.cpp.al: the operator chose the rolling-release design when offered both (2026-09-14). The skeleton's timestamped filenames could never have provided a stable URL regardless.
  - "Actually new" means model content only: the operator selected content-only comparison (2026-09-14), after asking for no publication "if nothing is changed from the previous file" and "only... when there's something that's actually new". Status flaps and timestamps never trigger an upload.
  - The repository becomes public: the operator approved (2026-09-14) after the constraint surfaced - release assets on private repositories require authentication, so end users could never download the sheet. Safety is preserved: GitHub never exposes Actions secrets, write access stays with the operator alone, and the scan gate catches a secret reaching the sheet. The operator's words on the repository's purpose: "This repository has all the secrets on GitHub. It's separate repository for safety, so I'm the only one who has write access."
  - Weekly schedule plus manual dispatch: the operator's words - "I want a weekly action... I also wanna be able to trigger the action manually, so this way I can get the initial file up there" (2026-09-14). The cron expression `0 12 * * 1` carries over from the operator's existing skeleton (`.github/workflows/models.yml`).
  - The published asset is named `cloud-provider-models.json` at the stable URL: the operator's correction - "Why is it called models.json everywhere when I asked for cloud-provider-models.json?" (2026-09-14) - superseding the morning's deferred-design name. The release tag `models` stays, so the URL path segment `/download/models/` remains.
  - The README documents the download as a feature, not a bug: the operator's words - "so users understand why PromptForge is 'phoning home' every so often to check for a new file" (2026-09-14).
- Rejected alternatives:
  - The dl.cpp.al scp upload with timestamped filenames: rejected with the choice of the GitHub rolling release; the `UPLOAD_SSH_KEY` secret stays configured but unused. Revisit if GitHub release hosting proves unsuitable.
  - Comparing everything except timestamps (so status changes trigger publication): rejected in favor of content-only comparison. Revisit if consumers ever need status transitions pushed to them.
  - Keeping the published asset name `models.json` for URL stability: rejected by the operator's rename correction (2026-09-14); the URL stays stable under the new name because the asset is overwritten in place. Revisit never.
- Assumptions, risks, and notes:
  - `DEEPGRAM_API_KEY` appears in the skeleton's env block but is not among the repository's configured secrets; deepgram records `unavailable` on every run until the operator adds it through the GitHub UI.
  - The twelve Subprime keys and the AWS/Azure configuration variables are not configured; those providers record `unavailable` until the operator provisions them. This matches the design's missing-key semantics.
  - `NVIDIA_API_KEY` and `OPENROUTER_API_KEY` are configured but unused - both providers are keyless. Harmless.
  - GitHub-hosted `ubuntu-24.04` images carry `jq`, `gh`, and `curl` preinstalled; if a future image drops one, the workflow gains an install step.
  - A stale vim swap file `.models.yml.swp` sits beside the workflow; left untouched.
  - Badge and download links in the README render only after the repository is public and the first release exists; until then they show as broken images. This is expected and self-healing.

### Deferred and Out of Scope

- Deferred: Gateway sheet consumption and config-UI integration (phase 2). Revisit when reliable published sheets exist.
- Deferred: provisioning the remaining provider secrets (Deepgram, the twelve Subprime keys, AWS/Azure configuration). Revisit when the operator creates the consoles' keys.
- Out of scope: changes to `cppalliance/promptforge`; the dl.cpp.al upload path; Niche static lists; SiliconFlow.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: None
- Focused test command pattern: None
- Component test command pattern: None
- Full-suite test command: None
- Linter command: None
- Formatter check command: None
- Docs command: None
- Test placement and naming conventions: None; the repository contains no tests.
- Directory map: `README.md` (project description) and `.github/workflows/models.yml` (the sole GitHub Actions workflow). No source, test, or docs directories exist.
- Component boundaries: One component only: the `models` workflow. It runs weekly (cron, Mondays 12:00 UTC) and on `workflow_dispatch`, clones `cppalliance/promptforge` at master, runs bash steps inside the clone to build a timestamped model-list file, and uploads it via scp to `dl.cpp.al:/var/www/downloads/promptforge/` using an SSH key from secrets. Provider API keys are injected as environment variables from GitHub secrets.
- Conventions summary: Workflow shell steps use `set -euo pipefail`; YAML is two-space indented with explanatory comments; secrets follow the `<PROVIDER>_API_KEY` naming pattern; the build step currently holds an interim placeholder marked `RUN SCRIPTS HERE!!`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Rewrite workflow and README [completed]

- Component: `none`

Rewrite the `jobs:` section of `.github/workflows/models.yml` per Technical Design: clone `cppalliance/promptforge` (master, depth 1, unauthenticated HTTPS), stable Rust toolchain with a build cache, `cargo build --release --locked -p shared-cloud-providers`, then run the binary with `MODELS_SHEET_PREVIOUS_URL` set to the stable asset URL and an explicit output path under `$RUNNER_TEMP`. Add the secret-scan gate (bash loop over every key variable in the env block, `grep -Fq -- "$value"` against the sheet, fail on any hit, values never echoed), content-only change detection (download the published asset, 404 forces "changed", normalize both sheets with `jq -S '.providers | with_entries(.value |= .models)'`, compare with `cmp -s`, pass the result through `$GITHUB_OUTPUT`), and the conditional rolling-release publish (`gh release view models || gh release create models`, then `gh release upload models <sheet> --clobber`, with `permissions: contents: write`). Carry the trigger block (cron `0 12 * * 1` plus `workflow_dispatch`) and the fourteen-entry secrets env block over verbatim. Rewrite `README.md`: badges (workflow status, latest release, download link), the stable asset URL `https://github.com/cppalliance/promptforge-cloud-providers/releases/download/models/cloud-provider-models.json`, what the sheet is (a 23-provider model catalog envelope with `schema_version`, `generated_at`, and a `providers` map), why PromptForge periodically downloads it (an anonymous GET of a static public file; nothing is uploaded; the file contains no keys; users who never use cloud providers can ignore or disable the check), a one-paragraph pipeline summary, and a link to `cppalliance/promptforge`. Validate the workflow YAML and review each inline script locally under `set -euo pipefail` semantics. One commit on `master` containing both files.

</step-1>

<step-2>

### Step 2: Publish and verify end-to-end [completed]

- Component: `none`

Make `cppalliance/promptforge-cloud-providers` public (`gh repo edit --visibility public`) and push the Step 1 commit to the aggregation repository's `master`. The operator then dispatches the workflow manually twice: the first run (no existing release) must create the `models` release and upload the initial sheet; the second run, with no upstream change, must detect identical content and skip the publish step. Verify both outcomes in the Actions log, confirm the stable asset URL downloads without authentication (`curl -fsSL` or a private browser window), and confirm the README badges and links render. Report the Actions log outcomes and the anonymous-download result. Operator note: add `DEEPGRAM_API_KEY` to the repository secrets when convenient; until then deepgram records `unavailable` by design.

</step-2>

</execution-plan>
