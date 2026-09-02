---
name: Evergreen links and version
overview: Enable stable "latest" release links for Workshop, add version-free download URLs, fix the hardcoded Workshop About version, and surface version in the Gateway config UI header.
todos:
  - id: make-latest
    content: "Change `make_latest: false` to `true` in release-workshop.yml"
    status: completed
  - id: evergreen-downloads
    content: Add rename + upload step for version-free installer names on the `promptforge-workshop-latest` rolling release
    status: completed
  - id: workshop-about-version
    content: Replace hardcoded APP_VERSION in about-dialog.ts with __APP_VERSION__ build-time define; ensure workshop-server build.rs passes the define
    status: completed
  - id: config-ui-header-version
    content: Add version label to the Gateway config UI tab bar header in tab-bar.ts + layout.css
    status: completed
isProject: false
---

# Evergreen Release Links and Version Display

Four changes across the release workflow, the Workshop About dialog, and the Gateway config UI.

## 1. Mark Workshop releases as GitHub "latest"

In [release-workshop.yml](promptforge/.github/workflows/release-workshop.yml), line 354, change `make_latest: false` to `make_latest: true`. This gives an evergreen page URL at `/releases/latest`.

## 2. Upload version-free installers to the rolling release

In the existing `Publish the stable updater pointer` step of [release-workshop.yml](promptforge/.github/workflows/release-workshop.yml) (lines 366-379), extend it to also upload the installers with fixed names. Add a step before it that copies and renames:

- `*-setup.exe` -> `PromptForge-setup.exe`
- `*.dmg` (arm) -> `PromptForge-arm64.dmg`, (intel) -> `PromptForge-x64.dmg`
- `*.deb` -> `PromptForge.deb`
- `*.AppImage` -> `PromptForge.AppImage`

Then `gh release upload promptforge-workshop-latest ... --clobber` with the renamed files alongside `latest.json`. This gives stable direct-download URLs like:

```
https://github.com/cppalliance/promptforge/releases/download/promptforge-workshop-latest/PromptForge-setup.exe
```

## 3. Fix Workshop About dialog version

In [about-dialog.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/about-dialog.ts), the version is hardcoded at line 13:

```typescript
const APP_VERSION = "0.2.0";
```

Replace this with the build-time `__APP_VERSION__` define (same pattern the config UI already uses), falling back to `"dev"`:

```typescript
declare const __APP_VERSION__: string | undefined;
const APP_VERSION = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";
```

This requires the Workshop server's esbuild config to define `__APP_VERSION__`. Need to check whether [promptforge-workshop-server/build.rs](promptforge/crates/promptforge-workshop-server/build.rs) already passes this define via `ui-build`; if not, enable it (the config UI crate already does this with `define_app_version: true`).

This eliminates the manual "bump hardcoded version" step forever. The Tauri `getVersion()` path via `UpdateService` subscription remains as a secondary source but is no longer the only way to get the real version.

## 4. Show version in Gateway config UI header

The config UI already shows `Version 0.2.0` in Settings > About ([settings-view.ts](promptforge/crates/promptforge-gateway-config-ui/ui/src/views/settings-view.ts) line 1674). Add it to the tab bar as well for visibility.

In [tab-bar.ts](promptforge/crates/promptforge-gateway-config-ui/ui/src/components/tab-bar.ts) (around line 92-105, the `.tab-actions` area), add a subtle version label element using the same `__APP_VERSION__` define. Style it as a muted `.tab-version` span in [layout.css](promptforge/crates/promptforge-gateway-config-ui/ui/src/styles/layout.css).


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Evergreen links and version (plan 7a3971d3)

Source: creator chat, Sep 2 2026.

## Origin

The plan grew out of an operational question, not a design brief: "how do we make the releases get published?" The answer (push a version-matching tag; the pipelines build, test, and publish with no manual step) led through nightly mechanics to the actual feature request: "how do I make an evergreen link, i.e. always the same name, without putting the version in the title?"

## Alternatives considered for evergreen links

Three approaches were laid out:

1. GitHub's built-in `/releases/latest`, enabled by flipping `make_latest: false` to `true`. Simplest; gives a stable page URL. Catch: asset filenames keep the version baked in, so per-installer direct links still change every release. (Noted in chat: `true` is GitHub's default, so deleting the line would also work; the plan keeps it explicit.)
2. A new dedicated rolling tag (e.g. `latest` or `stable`) with fixed-name assets, using the delete-and-recreate pattern the nightly workflow already uses. Fully stable URLs, but adds a second rolling release to maintain.
3. Extend the existing `promptforge-workshop-latest` rolling release, which already existed solely to carry the Tauri updater manifest `latest.json`, to also carry the renamed installers.

Discarded: option 2 as a standalone mechanism. The chosen design combines 1 and 3: `make_latest: true` for the evergreen page, plus version-free installer copies uploaded with `--clobber` onto the pre-existing updater release for stable direct-download URLs. The motivation for stable direct-download URLs (paraphrase): supporting things like a "Download" button on a website. User approval was verbatim: "I want those two things yes."

## Scope expansion

Plan items 3 and 4 came from one sentence: "I want those two things yes. and I also want the about of worksop and somewhere in the gateway config ui to show the version" (sic; "worksop" = Workshop).

## About dialog design choice

Two ways to source the real version were weighed:

- Tauri's runtime `getVersion()` API, called when the dialog opens (the UpdateService already uses it). Considered cleaner than build-time defines for the Tauri case, but it needs a fallback for browser mode.
- The `__APP_VERSION__` build-time esbuild define, the pattern the Gateway config UI already used via the shared `ui-build` crate (`define_app_version: true`).

Chosen: the build-time define, for consistency with the existing config UI pattern and to eliminate the manual "bump the hardcoded string" step forever. The runtime `getVersion()` path via the UpdateService subscription remains as a secondary source rather than the only one.

## Config UI placement

The version already existed in Settings > About; the user's ask was for visibility, so it was duplicated into the tab bar header as a muted label rather than moved. The tab bar placement was the assistant's choice; the user only specified "somewhere."

## Execution constraints

"do it all in one commit and adopt a light version of @tools-public/rulebooks/vibe-rulebook.md" - light meant a single Bounded step: one Coder subagent for all four changes, then Message, Review-and-Fix (zero findings, no amend), and a final Verify running the full suite (179/179 Rust, 57/57 workshop UI, 108/108 config UI tests). Result: commit `e6fa867a`, "Add evergreen release links and baked version display" (9 files, +91/-12).

One operational note from the chat: the evergreen upload step only takes effect on the next `promptforge-workshop-v*` tag; nothing retroactively fixes already-published releases.
