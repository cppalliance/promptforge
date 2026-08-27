---
name: Refresh tree on folder drop
overview: "Fix folder drops doing nothing: the grant succeeds but the Workshop tree is never notified and its roots listing is cached forever. Wire a grant event from the drop handler to the tree panel and invalidate the cached roots."
todos:
  - id: grant-event
    content: Dispatch promptforge:workspace-granted from workspace-drops.ts after successful grants
    status: pending
  - id: tree-refresh
    content: "Listen in WorkshopTreePanel: invalidate roots cache, re-render, remove empty hint, dispose listener"
    status: pending
  - id: tests
    content: Cover grant event dispatch and tree refresh in UI tests
    status: pending
  - id: verify
    content: npm test + typecheck, then manual drop test in the desktop app
    status: pending
isProject: false
---

# Refresh the Workshop tree when a drop grants a root

## Root cause

Dropping a folder works end to end through the shell: wry's drag-drop handler forwards the paths, `dispatch_file_drop` fires `promptforge:file-drop`, and [workspace-drops.ts](promptforge/crates/promptforge-ws-server/ui/src/workspace-drops.ts) POSTs each path to `/workspace/grant` successfully. But the tree panel in [workshop-panel.ts](promptforge/crates/promptforge-ws-server/ui/src/workshop/workshop-panel.ts) fetches the granted-roots listing exactly once in `init()` and caches it in the module-level `listingCache` under `ROOTS_KEY` forever. No event reaches the panel, and even a manual re-render would read the stale cache. Result: the drop grants the root and nothing visible happens.

The server side needs no change: `GET /workspace/tree` with no path already returns the granted-roots listing (`grants_listing` in [workspace.rs](promptforge/crates/promptforge-ws-server/src/workspace.rs)).

## Fix

### 1. `ui/src/workspace-drops.ts` - announce successful grants

After `grantDroppedPaths` completes, collect the paths that granted successfully and dispatch a window event:

```ts
window.dispatchEvent(new CustomEvent("promptforge:workspace-granted", {
  detail: { paths: grantedPaths },
}));
```

Dispatch only when at least one grant succeeded; failed paths already paint the status bar and are excluded. Validate nothing here - the paths came from the shell's validated drop event and the server's 200.

### 2. `ui/src/workshop/workshop-panel.ts` - listen and refresh

- In `init()`: register a `promptforge:workspace-granted` listener. Implement `dispose()` on the renderer to remove it (Dockview calls `dispose` when a panel is destroyed; verify against the dockview 8.2 `IContentRenderer` interface in node_modules).
- On the event: delete `ROOTS_KEY` from `listingCache`, clear the rendered list, remove the empty-state hint paragraph if present, and re-run `loadRoots()`.
- Guard the empty-state paragraph: it is appended on every `loadRoots()` with an empty listing, so remove any existing `.workshop-tree__empty` before appending a new one (both on grant-refresh and on re-init) to avoid duplicates.
- Session expansion state (`expandedPaths`) and cached subdirectory listings are untouched: a refresh of the roots list does not collapse expanded directories.

### 3. Tests - extend `ui/test/workspace-drops.mjs` and `ui/test/workshop-zones.mjs`

- workspace-drops: a successful grant dispatches `promptforge:workspace-granted` with exactly the succeeded paths; a fully failed drop dispatches nothing.
- workshop-zones (or a small new test if the harness fits better): with the tree mounted, firing the event re-fetches the roots listing and renders the new root; the empty-state hint disappears; firing it twice does not duplicate rows; a disposed panel stops listening.

## Verification

- `npm test` and `npm run typecheck` in `crates/promptforge-ws-server/ui`.
- Manual (also proves the wry handler fires on Windows, which no test covers): `cargo run -p promptforge-ws --features cuda`, drag a folder from Explorer onto the window, confirm the root appears in the Workshop tree immediately, expand it, open a file. If the root does not appear, the next suspect is the shell's drag-drop handler itself - check for the `could not forward the dropped paths` stderr line before digging into the UI.

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Refresh tree on folder drop (be1ad63f)

## Provenance caveat

The supplied transcript is not the plan's creation chat. It is a post-implementation verification chat (Aug 28, 2026) whose entire user content is one question: "did this get applied". It contains no design rationale, no why, and no discarded alternatives from the planning session - none were discussed there.

## What the chat does add (post-hoc audit findings, paraphrase)

The audit confirmed the plan was applied as commit `cbdbf57` "Make Explorer drops visible in the Workshop tree" (Aug 27, 2026), working tree clean, with these deviations from the plan text:

- The event was renamed: implemented as `promptforge:workspace-changed` (via a `WORKSPACE_CHANGED_EVENT` constant) with no payload, instead of the plan's `promptforge:workspace-granted` with `detail: { paths }`. The auditor judged this functionally equivalent because the tree only needs a poke, not the paths.
- The plan's tree-side tests were never added: no coverage for event-triggered roots re-fetch, empty-hint removal, duplicate-row prevention, or dispose-stops-listening. Only `workspace-drops.mjs` gained a test ("granting announces one workspace change").
- The plan's "fully failed drop dispatches nothing" test is missing; the existing failure test uses a mixed blocked/allowed batch, so the event still fires.
- Scope grew beyond the plan: the commit also added server-side dunce canonicalization (stripping Windows verbatim path prefixes) and root rows displaying folder names instead of full paths.
- The plan file's todos still read `pending`; the document was never updated after landing.
- The manual desktop drop test (plan step 4) was unverifiable from the chat.

## Note on paths

The audit found the UI sources live under `ui/src/ui/` (an extra `ui` segment), not the `ui/src/` paths the plan cites - relevant to anyone executing or amending the plan later.
