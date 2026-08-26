---
name: model-turn-actions
overview: Correct the model-turn feed states and actions, move model selection into the title bar, remove the obsolete selector sidebar, and make the always-unlocked Workspace-left, Editor-middle, Agent-right workbench support tabbed independent Agent panels.
todos:
  - id: step-1-thinking-states
    content: Replace the dot loader with immediate Planning next moves prefill and a toggleable Thinking reasoning block
    status: completed
  - id: step-2-turn-footer
    content: Create one footer per completed model turn with Copy, inert Fork, and timestamp tooltip
    status: completed
  - id: step-3-model-menu
    content: Replace the selector sidebar with a dynamic top-level Model menu
    status: completed
  - id: step-4-agent-tabs
    content: Add File > New Agent and support multiple independent Agent tabs
    status: completed
  - id: step-5-unlocked-layout
    content: Remove layout locking and make the tabbed three-zone workbench unlocked by design
    status: completed
  - id: step-6-final-verify
    content: Run full UI, TypeScript, Rust formatting, Clippy, and workspace test gates
    status: completed
isProject: false
---

# Workshop model menu, agent tabs, layout, and turn feed

## Execution rules
- Execute under the loaded vibe rulebook: one testable commit per step, a coder subagent followed by one review-and-fix subagent, bounded git in the main session, and scheduled verification.
- Apply the loaded TypeScript rulebook to all touched UI code: strict typing, `import type`, no new `any` or enums, explicit exported return types, no floating promises, and runtime validation at external boundaries.
- Apply the loaded HTML/CSS rulebook to all touched markup and styles: semantic buttons and landmarks, accessible names, keyboard and focus behavior, native controls before ARIA, visible focus states, reduced-motion handling, and no layout-shifting controls.
- Apply the loaded Rust rulebook to any touched Rust code: concrete `thiserror` errors, no new `unsafe`, no silent error swallowing, rustfmt and Clippy-clean changes, and tests beside the behavior they guard.
- Keep the existing TypeScript configuration stable; do not enable repo-wide `noUncheckedIndexedAccess` as part of this feature because the prior audit found unrelated pre-existing failures. New and touched modules must still be written to that standard and checked in isolation.

## Execution steps
1. **Thinking states.** Replace the dot loader and grace-delay path with immediate `Planning next moves`, then the first reasoning token switches to a durable, toggleable `Thinking` block. Test: [thinking-block.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/thinking-block.mjs).
2. **Model-turn footer.** Add one footer per completed model turn with working Copy, intentionally inert Fork, relative time, and absolute-time/duration tooltip. Test: new focused footer test wired into [package.json](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/package.json).
3. **Model menu and sidebar removal.** Move catalog ownership and selection into TypeScript state, add the dynamic Model title-bar menu, and remove the obsolete selector sidebar. Test: [smoke.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/smoke.mjs) and [window-menu.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/window-menu.mjs). Verify after this step.
4. **Tabbed Agent panels.** Add File > New Agent, stable per-agent panel ids, one `ChatUI` per Agent tab, active-agent routing, shared model application, and per-tab cleanup. Test: [workshop-zones.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/workshop-zones.mjs), [window-menu.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/window-menu.mjs), and focused Agent lifecycle coverage.
5. **Always-unlocked workbench.** Remove lock state, controls, persistence, styles, and menu commands; make tabs and dividers permanently interactive while preserving user rearrangements. Test: [workshop-layout.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/workshop-layout.mjs) and [workshop-zones.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/workshop-zones.mjs).
6. **Final verification.** Run the full UI suite, TypeScript typecheck, `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --locked --workspace --all-features`.

## Implementation
### Model menu and sidebar removal
- Remove the entire static model selector sidebar, including `#model-picker`, `#model-description`, and their styles, from [index.html](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/index.html) and [style.css](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/style.css). The Dockview workbench then occupies the full application width, making Workspace the leftmost visible panel.
- Add `Model` between Edit and Window in the custom title bar. Extend [window-menu.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/window-menu.ts) with a dynamic Model popover that lists the current catalog, marks the selected model, disables unavailable states, and uses each model description as supporting tooltip text.
- Replace DOM-owned selection in [main.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/main.ts) with explicit catalog and selected-model state. Boot and pushed catalogs update that state, preserve the current model when still available, otherwise select the first model, rebuild the Model menu, update `ChatEngine` request defaults, and continue blocking submission only when no model exists. Browser-mode tests may still auto-select the first catalog entry even though the desktop-only menu is hidden.

### Tabbed Agent panels
- Add `File > New Agent` to [window-menu.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/window-menu.ts). The command opens a new Agent panel in the right zone and activates its tab; it does not clear or replace the existing Agent conversation.
- Change chat panel identity in [zones.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/workshop/zones.ts) from the singleton id `chat` to stable per-agent ids such as `agent:<uuid>`. Keep the Workspace tree singleton and path-keyed editors. Persist per-agent ids so restored layouts recreate the same tabs.
- Replace the single global `ChatUI` mount in [main.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/main.ts) with an Agent panel controller. The controller creates one `ChatUI` per Agent panel, shares the socket provider and model-selection state across all agents, applies model changes to every live agent, tracks the active agent for File > New Chat and voice submission, and destroys a `ChatUI` when its panel closes. The controller owns plugin lifecycle per agent so each tab gets isolated voice, thinking, and tool state.
- Rename the panel type and default tab title from `Chat` to `Agent` in [panel-types.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/workshop/panel-types.ts). Every Agent group shows Dockview tabs in unlocked mode; opening a second Agent creates a second tab in the right bank.

### Always-unlocked workbench
- Remove [layout-lock.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/workshop/layout-lock.ts), its zone-header padlock control, the Window menu Lock/Unlock command, `.dock--locked` and `.layout-lock-toggle` styles, and every import or callback dedicated to lock state.
- Create Dockview with `locked: false` in [main.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/main.ts), leaving dividers, tabs, and panel drag targets permanently interactive. Keep floating groups disabled.
- Simplify [layout-persistence.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/workshop/layout-persistence.ts) by removing the persisted `locked` field and lock-change subscription, then bump the layout schema so stale locked snapshots cannot reapply obsolete state. Preserve users' moved panel positions and their Agent tab identities across launches.
- Keep the affinity mapping in [panel-types.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/workshop/panel-types.ts): Workspace tree to `left`, document editors to `main`, and Agent panels to `right`. Make the known-good default boot order explicit as Workspace-left, empty Editor-middle until a document opens, and one Agent-right.

### Thinking and model-turn actions
- Correct [thinking-plugin.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/plugins/thinking/thinking-plugin.ts) so generation starts immediately with only `Planning next moves` (no trailing ellipsis), with no three-dot loader and no 500 ms delay. On the first reasoning token, replace that prefill row with the normal `Thinking` toggle and stream the auditable reasoning preview beneath it. Clicking `Thinking` expands the preserved reasoning, and clicking it again rolls the block back up. Keep that toggle available after completion.
- Give the thinking plugin explicit ownership of the empty assistant loading state instead of allowing [message-node.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/components/message-node.ts) to render its generic three-dot fallback at the same time. Update [thinking.css](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/plugins/thinking/thinking.css) so shimmer applies only to prefill, not to the `Thinking` label used while reasoning tokens stream.
- Add a reusable turn-footer component beside [feed-node.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/components/feed-node.ts). Mount it once per completed model turn for both grouped agent runs and ordinary assistant responses, keeping it outside collapsible thinking and tool activity so it never disappears.
- Reuse `extractPlainText` from [msg-utils.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/core/msg-utils.ts) for Copy, preserving the existing checkmark feedback behavior. Add the fork glyph as an accessible button with tooltip, but intentionally give it no conversation mutation or click behavior yet.
- Derive the footer clock from the model response's completion timestamp and the existing user-to-final-response duration calculation in [feed-items.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/components/feed-items.ts). Render compact relative text such as `2m ago`, refresh it at time-unit boundaries, and show a styled hover and keyboard-focus tooltip containing the localized absolute timestamp plus `Worked for 1m 50s` when duration is available.
- Style the muted icon row and dark rounded tooltips in [feed.css](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/styles/feed.css), matching the supplied Cursor reference while retaining visible focus states, semantic `<time>` markup, reduced-motion behavior, and ARIA labels.
- Consolidate the currently dormant [copy-plugin.ts](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/src/chat/plugins/copy/copy-plugin.ts) behavior into the turn footer or share its helper, avoiding duplicate copy controls when plugins are later registered.

## Data flow and verification
- The model catalog has one TypeScript owner. Both boot fetches and WebSocket catalog pushes update it, the Model menu reads it, selection writes request defaults, and chat submission reads the selected ID without depending on removed DOM.
- New Agent allocates a fresh panel id and `ChatUI`; the shared model owner broadcasts selection to all live agents. Closing a tab destroys only that agent. Restored Agent tabs receive fresh in-memory sessions; persisted layout restores tab identity and placement, not chat history. Dockview owns all resizing and movement in permanently unlocked mode, while layout persistence records tabs and user rearrangements without carrying any lock state.
- Generation start creates the sole prefill indicator. The first reasoning block atomically removes it and creates the clickable `Thinking` preview; stream completion collapses that same durable block without deleting its content. The user can expand or collapse it during streaming and after completion. No state renders both labels or the generic dots.
- Feed grouping supplies one final assistant response, its completion time, and run duration to the footer. The footer formats those values, schedules only its next relative-time refresh, and clears that timer when its feed node is destroyed. Copy reads the same final response text; Fork remains visual-only.
- Update [smoke.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/smoke.mjs), [window-menu.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/window-menu.mjs), [workshop-layout.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/workshop-layout.mjs), [workshop-zones.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/workshop-zones.mjs), and [thinking-block.mjs](c:/Users/Vinnie/cursor/promptforge/crates/promptforge-ws-server/ui/test/thinking-block.mjs) for the new menu order and dynamic selection, complete sidebar and lock removal, visible tabs, New Agent behavior, independent agent state and cleanup, restored Agent tabs, default zone order plus restored rearrangements, immediate dot-free prefill, and repeated thinking toggles. Add focused footer tests for one footer per turn, clipboard feedback, inert Fork behavior, relative-time rollover, tooltip text, grouped-run persistence, accessibility, and timer cleanup.
- Verify each step with its narrowest failing test first. Run the full UI suite and TypeScript typecheck after steps 3 and 6, and whenever review-and-fix changes files. At the final gate, run `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --locked --workspace --all-features` from the PromptForge workspace.

Recommendation: put the footer at the feed-turn layer rather than inside message plugins so grouped thinking and tool nodes cannot duplicate or remove it. Confidence: high - the feed already exposes the exact final-message and duration boundaries needed.

Recommendation: keep panel positions persistent after users move them, with Workspace-left, Editor-middle, Agent-right as the clean default only. Confidence: high - this preserves the purpose of an always-unlocked movable workbench.

Recommendation: make New Agent create an independent tab rather than clear the existing conversation. Confidence: high - that matches both the requested tabbed workbench and the selected multiple-tabs behavior.

## Plan review
- **Coverage:** every requested behavior has a step, owner files, and test coverage. No chat-only facts remain required for execution.
- **Data flow:** model catalog flows from boot fetch and socket pushes into one state owner, then into the Model menu and every live Agent. Agent creation flows from File > New Agent through stable panel ids to a dedicated `ChatUI`. Thinking flows from generation start to prefill, then to the durable reasoning block. Turn footers read the feed's final response, completion timestamp, and duration.
- **Dependencies:** thinking and footer are independent feed steps; Model menu must precede Agent tabs because the Agent controller consumes shared model state; Agent tabs must precede final lock removal because tab visibility and persistence are verified together.
- **Efficiency:** no Rust server change is expected; the work stays in the UI shell and existing socket/server paths. Parallelism is limited after step 3 because Agent tabs and unlocked layout both touch layout, menu, and persistence files.
- **Residual risk:** multiple `ChatUI` instances are the largest change. The plan contains the risk by isolating plugin lifecycle per tab, sharing only the socket provider and model state, and requiring cleanup tests. Confidence: medium-high - murm-ui supports containment, but the current workshop boot path assumes one global chat instance and will need careful rewiring.

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: model-turn-actions

## Origin

This plan consolidates a same-day design conversation (Aug 26, 2026) in which the user corrected an earlier workshop UI implementation, using Cursor's own chat UI as the explicit visual and behavioral reference. Several plan items are reversals of decisions made earlier in that same chat, which is why the plan removes as much as it adds.

## Decisive user statements (verbatim)

Model-turn footer, dictated with Cursor screenshots attached:

> "I want each model turn to have these buttons: copy to clipboard, fork, and show time on hover, and tooltip"

Thinking states, rejecting the shipped dot loader after seeing it run:

> "This is wrong. It should not be showing the three dots. Just "Planning next moves" during prefill and then "Thinking" while thinking tokens are streaming in"

> "and Thinking should be clickable to expand the thinking so you can see it, then roll it back up"

> ""Planning next moves" should not have elipses at the end ... in the ui"

Retention of reasoning after completion, the auditability motive behind the durable Thinking block:

> "the thinking disappears after its done. I want it to stay just like cursor so it can be audited"

Model menu, zone order, and lock removal, all in one decisive message:

> "Model should be a top level menu in the menu bar instead of a drop down in its own panel. relocate the model selector as a Model menu and completely remove that leftmost panel. The Workspace panel should be left, the Agent panel should be right, and the Editing panel should be middle. Completely remove the lock/unlock feature, including the icon and the code to make the dividers lockable. They should always behave unlocked."

Agent tabs:

> "docked windows need tabs, I didn't see any tab for the Agent window. can we add a menu item for New Agent?"

## Discarded alternatives

- **Lock/unlock workbench.** Earlier the same day the user had requested the lock: "there should be a little lock icon in the bar, in the top of the workshop panel, and If, if you unlock it, then it becomes movable and you can stick it to either side." After living with it he reversed course entirely ("They should always behave unlocked"). This is why the plan deletes `layout-lock.ts` outright and bumps the layout schema: stale persisted snapshots carry the obsolete `locked` field and must not reapply it.
- **Model selector sidebar.** The leftmost panel was the original home of model selection. It was discarded in favor of a title-bar Model menu, which also makes Workspace the leftmost visible panel and gives the Dockview workbench the full window width.
- **Three-dot loader with 500 ms grace delay.** The delay existed to avoid flashing a loader on fast responses (paraphrase). The user rejected the dots on sight; the plan replaces them with immediate `Planning next moves` prefill that the first reasoning token atomically replaces.
- **Fork behavior.** The user asked for the button set as seen in Cursor; only Copy was ever given a specified behavior. The plan deliberately ships Fork inert (accessible button, tooltip, no click mutation) as a scoping decision, not an oversight (paraphrase).
- **Thinking display details.** Earlier in the chat the user specified a roughly four-line scrolling window with dim text animated in a blend of grays, modeled on Cursor's collapsed one-line autoscrolling status: "nobody wants a default where all the thinking just scrolls the window endlessly." The final plan keeps the durable toggleable block as the core; the prefill-only shimmer and scroll containment survive as styling details in `thinking.css`.

## Design thinking

- Cursor is the explicit reference implementation for the turn footer, the thinking block, and the dark rounded tooltips; the plan's phrase "matching the supplied Cursor reference" refers to screenshots the user pasted during the session.
- Zone affinity comes from the user's panels vision stated earlier that day: "each zone has a preference. Like the agent zone prefers to have agent windows, the document zone prefers to have documents. So whenever a new document is created, it goes to the zone where it has affinity." The plan preserves this as the affinity mapping while making the default boot order Workspace-left, Editor-middle, Agent-right explicit.
- Modularity was a standing requirement from the same conversation: "I want each piece of code to be very well isolated, especially the TypeScript... so that I can swap pieces out" (paraphrase of a longer dictated passage). This motivates the per-Agent `ChatUI` controller with isolated plugin lifecycles per tab and only the socket provider and model state shared.
- The plan's restraint on `noUncheckedIndexedAccess` reflects a prior audit finding unrelated pre-existing failures; the user wanted the TypeScript configuration kept stable during feature work (paraphrase).
