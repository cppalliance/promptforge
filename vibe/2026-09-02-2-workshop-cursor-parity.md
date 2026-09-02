---
name: Workshop Cursor parity
overview: "Full visual and behavioral parity between the PromptForge workshop window and Cursor's workspace window: menu bar, file tree, agent panel (feed + input card with toolbar inside), status bar, zoom keys, auto-launch, and global typography. Every target value is extracted from Cursor's bundles and cited."
todos:
  - id: tokens
    content: Add layout tokens to style.css, update font stack and base font-size
    status: completed
  - id: zoom
    content: "Fix zoom: add with_browser_accelerator_keys(false) in main.rs"
    status: completed
  - id: menus
    content: "Restyle menu bar: title padding 8px/radius 5px, popover 160px/radius 5px/shadow, item height 2em/radius 4px, fadeIn animation"
    status: completed
  - id: statusbar
    content: "Restyle status bar: 22px height, 12px font, 22px line-height, 7px edge padding, tabular-nums (leave LEDs alone)"
    status: completed
  - id: tree
    content: "Restyle workshop tree: 22px rows, 8px indent, 11px/700/uppercase header"
    status: completed
  - id: feed
    content: "Agent feed: centered 780px column, 14px row gap, 12px inline padding"
    status: completed
  - id: bubbles
    content: "Agent messages: user bubble right-aligned 12px radius, assistant 13px text inset"
    status: completed
  - id: agent-card
    content: "Agent input card: DOM restructure (toolbar+mic+send inside 18px-radius card), prompt-input chromeless"
    status: completed
  - id: toolbar
    content: "Agent toolbar inside card: 8px gap, 8/10/10 padding, border-top separator"
    status: completed
  - id: tool-cards
    content: "Tool-call cards: 12px radius, 10px padding, 6px gap"
    status: completed
  - id: markdown
    content: "Markdown conversation text: 14px/22px"
    status: completed
  - id: auto-launch
    content: Auto-launch chat on agent panel init, skip agent menu
    status: completed
  - id: new-agent
    content: New Agent menu item resets session (fresh conversation each time)
    status: completed
  - id: tests
    content: "Update pinned test values: agent-session-view, agent-toolbar, titlebar-style, workbench-mount"
    status: completed
  - id: verify
    content: npm run build + typecheck + test; cargo build -p promptforge-workshop
    status: completed
isProject: false
---

# Workshop Cursor Parity

All Cursor values cited from `workbench.desktop.main.css` (as `desktop.css @ <byte>`), `workbench.glass.main.css` (as `glass.css @ <byte>`), or `workbench.glass.main.js` / `workbench.desktop.main.js` (as `glass.js @ <byte>` / `desktop.js @ <byte>`).

Workshop files under `promptforge/crates/promptforge-workshop-server/ui/`.

**Execution notes:**
- Single commit. All CSS, TS, Rust, and test changes land together.
- Auto-launch and "New Agent" lifecycle details: resolve by reading `agent-panel.ts`, `AgentSessionService`, and `AgentSocket` during implementation. Plan names the intent; code inspection fills the API surface.
- Body font-size goes to 13px globally (Cursor's default). Fix anything that breaks.
- Steps 1-4 (zoom, menus, statusbar, tree) are independent and can run in parallel after step 0 (tokens).

---

## 0. Global typography and tokens

**Font stack.** Cursor on Windows (`desktop.css @ 1224315`):
`Segoe WPC, Segoe UI, sans-serif`

Workshop (`style.css` body): `system-ui, -apple-system, "Segoe UI", Roboto, sans-serif`

Change `--font-prose` and `html, body` to `"Segoe WPC", "Segoe UI", sans-serif` so Windows rendering matches Cursor exactly. Keep a `-apple-system, BlinkMacSystemFont` prefix for macOS/Linux fallback (Cursor does the same in its titlebar fallback @ `desktop.css @ 1246324`).

**Base font-size.** Workshop body is `14px`. Cursor's UI base is `13px` (`--cursor-font-size-base`), with `14px` only for agent conversation text. Change body to `13px` globally. Add explicit `font-size: var(--font-size-lg, 14px); line-height: var(--line-height-lg, 22px)` on `.markdown-content` and `.agent-item__text` (the conversation surfaces). Any element that looks wrong at 13px is missing an explicit override and needs one regardless. Fix any breakage found during verify.

**New tokens** to add in `style.css` `:root`:

| Token | Value | Cursor source |
|---|---|---|
| `--agent-max-width` | `780px` | `glass.js @ ~30053651` constant `xEi` |
| `--agent-card-radius` | `18px` | `glass.js @ ~18689503` `--prompt-input-border-radius-expanded` = `--cursor-radius-4xl` |
| `--feed-padding-inline` | `12px` | `glass.js @ ~34868832` `--composer-messages-padding-inline` |
| `--feed-row-gap` | `14px` | `glass.js @ ~28325321` transcript `n1i` |
| `--agent-outer-padding` | `16px` | `glass.css` followup `glass-9go8x9` |
| `--bubble-radius` | `12px` | `glass.css @ ~638238` `--conversation-surface-border-radius` = `--cursor-radius-xl` |
| `--assistant-inset` | `13px` | `glass.css @ ~638238` `--conversation-glass-text-inset` |
| `--space-2-5` | `10px` | Cursor `--cursor-spacing-2-5` (gap in scale between 8 and 12) |
| `--token-ring-size` | `16px` | Cursor `TokenRing.js @ ~26377128` (already consumed with fallback, never defined) |
| `--statusbar-height` | `22px` | `desktop.css @ 687122` |

---

## 1. Fix zoom keys (Ctrl+= / Ctrl+-)

**Root cause:** [src/main.rs](promptforge/crates/promptforge-workshop/src/main.rs) line ~200 - `WebviewWindowBuilder` does not call `.with_browser_accelerator_keys(false)`. WebView2 on Windows intercepts Ctrl+= / Ctrl+- / Ctrl+0 at the native layer for its own built-in zoom, racing or blocking the JS `keydown` handler.

**Fix (Rust, one line):** Add `.with_browser_accelerator_keys(false)` to the Windows `#[cfg]` block in `open_window`, next to the existing `.disable_drag_drop_handler()`:

```rust
#[cfg(target_os = "windows")]
let builder = builder
    .disable_drag_drop_handler()
    .with_browser_accelerator_keys(false);
```

This disables WebView2's built-in keyboard shortcuts (zoom, find, devtools F12) so the JS handler in [src/ui/workshop/shortcuts.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/workshop/shortcuts.ts) is the sole zoom path.

**JS side:** No changes needed - `event.key` matching (`"="`, `"+"`, `"-"`, `"0"`) works fine once WebView2 stops eating the events. Numpad is not in scope (user confirmed not using numpad).

**Test impact:** `test/zoom.mjs` pins key strings `"="`, `"+"`, `"-"`, `"0"` and menu labels "Zoom In", "Zoom Out", "Reset Zoom" - all unchanged.

---

## 2. Menu bar

Files: [src/ui/window-menu.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/window-menu.ts), [src/ui/window-menu.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/window-menu.css), [src/ui/window-chrome.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/window-chrome.css)

### Menu button (title bar items)

| Property | Workshop now | Cursor target | Cite |
|---|---|---|---|
| Title padding-inline | `0 12px` | `0 8px` | `desktop.css @ 676993` |
| Title border-radius | none | `5px` | `desktop.css @ 676993` |
| Hover bg | `color-mix(#F0F0F0 8%, transparent)` | `var(--vscode-menubar-selectionBackground)` - same 8% wash, fine | `desktop.css @ 675532` |
| Hover transition | instant | instant (Cursor has no transition on menubar hover) | confirmed absent |

### Dropdown popover

| Property | Workshop now | Cursor target | Cite |
|---|---|---|---|
| Min-width | `200px` | `160px` | JS `xMb @ desktop.js ~30746654` |
| Border-radius | `6px` | `5px` | JS `desktop.js ~30735607` inline `border-radius:5px` |
| Shadow | `0 6px 18px rgba(0,0,0,0.5)` | `0 2px 8px ${shadowColor}` | JS `xMb` |
| Padding (bar) | `4px 0` | `4px 0` | same |
| Open animation | none | `fadeIn 0.083s linear` | JS `xMb` "context-view.monaco-menu-container" |

### Menu items

| Property | Workshop now | Cursor target | Cite |
|---|---|---|---|
| Item padding | `4px 12px` | margin `0 4px`, padding `0 2em` (~0 26px at 13px) | JS `xMb` |
| Item height | auto | `2em` (~26px) | JS `xMb` |
| Item border-radius | none | `4px` | JS `xMb` |
| Shortcut font-size | `12px` | inherits 13px | |
| Separator margin | `4px 0` | `5px 0` | JS `xMb` |

### Menu labels

Workshop already has "New Agent" in File menu (`test/window-menu.mjs` asserts this). No "Agent Window" label exists. The label is correct.

**Behavior change** for "New Agent": currently calls `openInZone("agent", {})` which focuses the singleton agent panel. Change to: reset the current agent session (close WS session, re-launch "chat") so each click starts a fresh conversation. Details in step 7 below.

**Test impact:** `test/titlebar-style.mjs` pins popover min-width (`200px`) and shadow - update to `160px` and `0 2px 8px`. `test/window-menu.mjs` pins menu labels - "New Agent" already correct.

---

## 3. Status bar

Files: [src/ui/status-bar.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/status-bar.ts), [src/ui/status-bar.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/status-bar.css)

| Property | Workshop now | Cursor target | Cite |
|---|---|---|---|
| Height | `28px` (`--status-bar-height`) | `22px` | `desktop.css @ 687122` |
| Font-size | `13px` (`--font-size-base`) | `12px` | `desktop.css @ 687122` |
| Line-height | `1.4` | `22px` (absolute, matches height) | `desktop.css @ 688140` |
| Padding-inline | `12px` | first-left `7px`, last-right `7px` | `desktop.css @ 689391, 689499` |
| Item label padding | (via gap) | `0 5px` | `desktop.css @ 690358` |
| Item label margin | (via gap) | `3px` left+right | `desktop.css @ 688857` |
| Background transition | none | `0.15s ease-out` | `desktop.css @ 687280` |
| Hover bg | none | `var(--vscode-statusBarItem-hoverBackground)` - add subtle hover | `desktop.css @ 691752` |
| Font-variant | default | `tabular-nums` | `desktop.css @ 688140` |

**Leave alone:** the two LEDs (`.status-bar__led`, `.status-bar__led--rec`) and their animation/colors.

Change `--status-bar-height` token from `28px` to `22px`. Restyle `.status-bar` and `.status-bar__text` / `.status-bar__right` to match Cursor's edge-padding model (7px edges, 5px label pad, 3px label margin) instead of the current 12px gap. Add `font-variant-numeric: tabular-nums`.

**Test impact:** `test/titlebar-style.mjs` pins `--status-bar-height` value (if asserted) - update. `test/smoke.mjs` and `test/workbench-mount.mjs` pin status text "Ready" - unchanged.

---

## 4. Workshop tree (explorer parity)

Files: [src/ui/workshop/workshop-panel.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/workshop/workshop-panel.ts), [src/ui/workshop/zones.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/workshop/zones.css)

Match Cursor's explorer tree density:

| Property | Workshop now | Cursor target | Cite |
|---|---|---|---|
| Row height | (auto) | `22px` | `desktop.css @ 906194` |
| Row line-height | (auto) | `22px` | same |
| Row padding-left | (auto) | `4px` | `desktop.css @ 905927` |
| Tree indent | (auto/nested ul) | `8px` per level | `desktop.js @ 2528800` `DefaultIndent=8` |
| File icon size | (auto) | `16x16` | `desktop.css @ 87717` |
| Twistie/chevron | (auto) | `16px` wide, font `10px`, `padding-right: 6px` | `desktop.css @ 82683` |
| Section header | (auto) | `11px`, weight `700`, `text-transform: uppercase`, height `22px` | `desktop.css @ 288703, 288936` |
| Hover bg | (auto) | `var(--bg-hover)` (8% wash) | JS injected list styles |

The workshop tree uses a custom DOM (`workshop-tree__row`) not Monaco's tree widget. CSS changes only - add explicit `height: 22px; line-height: 22px` on rows, `padding-left: 4px`, chevron sizing, section header uppercase at 11px/700. Indent is already via nested `<ul>` with padding-left - set `padding-inline-start: 8px` on `.workshop-tree__children`.

Header "WORKSHOP" label: add `text-transform: uppercase; font-size: 11px; font-weight: 700; height: 22px; line-height: 22px` to `.workshop-tree__header`.

**Test impact:** `test/workshop-layout.mjs` and `test/workshop-zones.mjs` assert DOM presence and shortcuts, not CSS values - unchanged.

---

## 5. Agent panel - feed and messages

Files: [src/ui/agent-session-view.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/agent-session-view.ts), [src/ui/agent-session.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/agent-session.css)

### Feed column (centered, max-width)

`.agent-session__feed`:
- `padding: var(--space-3, 12px)` -> `padding-block: var(--space-3, 12px); padding-inline: var(--feed-padding-inline, 12px)`
- `gap: var(--space-2, 8px)` -> `var(--feed-row-gap, 14px)`
- Add: `inline-size: 100%; max-inline-size: var(--agent-max-width, 780px); margin-inline: auto; box-sizing: border-box`

### User bubbles (right-aligned cards)

`.agent-item--user`:
- Remove: `border-inline-start: 2px solid var(--accent-dim)`
- Add: `align-self: flex-end; background: var(--bg-card); border: 1px solid var(--border-subtle); border-radius: var(--bubble-radius, 12px); min-inline-size: 150px; max-inline-size: 85%; padding: 8px 12px`
- Source: `glass.css @ ~617444` `.composer-human-message` rules

### Assistant messages (text inset)

`.agent-item--reply`:
- Add: `padding-inline: var(--assistant-inset, 13px)`
- Source: `glass.css @ ~638238` `--conversation-glass-text-inset`

### Tool-call / tool-result items

`.agent-item--tool-call`, `.agent-item--tool-result`:
- Add: `padding-inline: var(--assistant-inset, 13px)` to align with assistant text column

### Markdown conversation text

[src/ui/markdown-render.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/markdown-render.css):
- `.markdown-content`: `font-size: 13px; line-height: 18px` -> `var(--font-size-lg, 14px)` / `var(--line-height-lg, 22px)`
- Source: Cursor default `--conversation-text-font-size: var(--cursor-font-size-lg)` @ `glass.css @ ~638238`

**Test impact:** `test/agent-session-view.mjs` DOM order assertions - see step 6 for restructure.

---

## 6. Agent panel - input card (toolbar inside)

### DOM restructure in [src/ui/agent-session-view.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/agent-session-view.ts)

**Before:**
```
section.agent-session
  ol.agent-session__feed
  div.agent-toolbar (standalone sibling)
  div.agent-session__bar
    div.prompt-input (bordered)
    button.agent-session__mic
    button.agent-session__send
```

**After:**
```
section.agent-session
  ol.agent-session__feed (centered 780px column)
  div.agent-session__outer (16px outer padding wrapper)
    div.agent-session__bar (the card: 18px radius, border, bg, flex column)
      div.prompt-input (chromeless - no border/radius/bg)
      div.agent-toolbar (inside card, flex row)
        left: mode-chip, model-picker
        right (auto-margin): token-ring, mic, Send
```

Changes:
- New wrapper `.agent-session__outer` owns the 16px outer padding and max-width centering
- `.agent-toolbar` moves from sibling-of-bar to last child of bar
- Mic button and Send button move from bar siblings into `.agent-toolbar` right slot (after token-ring's `margin-inline-start: auto`)
- No-ModelService path: bar still works with no toolbar; mic/send stay at bar bottom

### CSS changes

**`.agent-session__outer`** (new):
- `flex: none; padding-inline: var(--agent-outer-padding, 16px); padding-block-end: var(--agent-outer-padding, 16px); inline-size: 100%; max-inline-size: var(--agent-max-width, 780px); margin-inline: auto; box-sizing: border-box`

**`.agent-session__bar`** becomes the card:
- `display: flex; flex-direction: column; gap: 0; align-items: stretch; background: var(--prompt-input-bg); border: 1px solid var(--prompt-input-border); border-radius: var(--agent-card-radius, 18px); overflow: hidden; padding: 0`
- Remove: old `border-top`, `padding: 12px`, `gap: 8px`, `align-items: flex-end`

**`.prompt-input`** ([src/ui/prompt-input.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/prompt-input.css)):
- Remove: `border`, `border-radius` (chrome moves to the card)
- Background: `transparent`
- Editor padding `8px 12px` and min/max height `36px`/`200px` are already correct (match Cursor @ `glass.js @ ~18689503`)

**`.agent-toolbar`** ([src/ui/agent-toolbar.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/agent-toolbar.css)):
- `block-size: var(--height-base, 28px)` -> `min-block-size: var(--height-base, 28px)` (let content breathe)
- `gap: var(--space-1, 4px)` -> `var(--space-2, 8px)` (Cursor toolbar gap @ `glass.js @ ~17641530`)
- Add: `padding: var(--space-2, 8px) var(--space-2-5, 10px) var(--space-2-5, 10px)` (Cursor `--prompt-input-toolbar-padding` @ `glass.js @ ~18689503`)
- Add: `border-block-start: 1px solid var(--border-subtle)` (card internal separator)
- Keep: `.agent-toolbar > .token-ring { margin-inline-start: auto }` for right-slot alignment

**Mic/Send inside toolbar:**
- `.agent-session__send` / `.agent-session__mic`: reduce min-size from `44px` to `var(--height-base, 28px)` for toolbar density (Cursor's in-toolbar actions are 28px tall)

### Tool-call cards

[src/ui/tool-call-card.css](promptforge/crates/promptforge-workshop-server/ui/src/ui/tool-call-card.css):
- `.tool-call-card` radius: `var(--radius, 6px)` -> `var(--bubble-radius, 12px)` (Cursor `--conversation-surface-border-radius` @ `glass.css @ ~498261`)
- `.tool-call-card__body` padding-inline: `12px` -> `10px`, gap: `8px` -> `6px` (Cursor `--conversation-tool-card-padding-x: 10px`, `--conversation-tool-card-gap: 6px` @ `glass.css @ ~638238`)
- `.tool-call-card__summary` padding-inline: `8px 12px` -> `6px 10px` (Cursor tight-x 6px, right 10px)

### Test impact

- `test/agent-session-view.mjs`: update DOM order assertion (feed -> outer -> bar with toolbar inside)
- `test/agent-toolbar.mjs`: update CSS source strings (gap `var(--space-2, 8px)`, min-block-size, padding) and child order (ring, mic, send)
- `test/prompt-input.mjs`: unchanged (36/200 clamp matches Cursor)
- `test/token-ring.mjs`: unchanged (16px/stroke 2 matches Cursor)

---

## 7. Auto-launch agent (skip the menu)

Files: [src/ui/workshop/agent-panel.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/workshop/agent-panel.ts), [src/ui/agent-menu.ts](promptforge/crates/promptforge-workshop-server/ui/src/ui/agent-menu.ts)

Currently `AgentPanel.init()` shows `AgentMenu` (the "Launch an agent to start a session" screen) and waits for a click. Change to:

1. After `socket.connect()` succeeds and `onDidChangeAgents` fires with a non-empty list, auto-call `service.launch("chat")` (or the first available agent if "chat" is absent).
2. Show the session view immediately (feed + input card). Skip painting the agent menu entirely.
3. The "Launch an agent..." text and the agent menu UI become dead code for now. Leave the class in place but do not mount it. (We will sort agent types later per user instruction.)

**"New Agent" creates a new session each time:** In the menu command handler for "New Agent" (`window-menu.ts` / `shortcuts.ts` `newAgent`), instead of `openInZone("agent", {})` (which focuses the singleton), call `service.launch("chat")` to reset the session. This closes the current WS session and starts a fresh one, clearing the feed. The agent panel stays singleton in the dock; only the session resets.

**Test impact:** `test/agent-menu.mjs` tests the menu buttons/labels/error states - these tests remain valid (the class still exists) but the auto-launch path needs a new test. `test/workbench-mount.mjs` asserts `.agent-menu` present - update to assert `.agent-session__feed` instead since the menu is skipped.

---

## 8. Stray controls audit

| Control | Verdict | Action |
|---|---|---|
| Agent menu (pre-session) | Removed from view | Step 7: auto-launch bypasses it |
| Gateway Config panel | Keep | User didn't ask to remove; it's functional and behind a menu item |
| About dialog | Keep | Standard Help > About |
| Dockview tab strips | Keep | Structural; match Cursor's tab styling in a future pass |
| Add Folder in tree | Keep | Cursor has similar tree actions |
| Model menu Profiles | Keep | Functional |

No other stray controls identified. The agent mode chip, model picker, and token ring are Cursor-aligned controls in the toolbar - they stay.

---

## 9. Build and verify

From `promptforge/crates/promptforge-workshop-server/ui/`:
- `npm run build`
- `npm run typecheck`
- `npm test`

From workspace root:
- `cargo build -p promptforge-workshop` (confirms esbuild layer plugin + Rust `with_browser_accelerator_keys` compiles)
- `cargo test -p promptforge-workshop` (if any Rust tests exist)

---

## Summary of value changes by file

| File | Changes |
|---|---|
| `style.css` | New tokens (agent-max-width, agent-card-radius, feed-padding-inline, feed-row-gap, etc.); font stack to Segoe WPC; body font-size 13px; --status-bar-height 22px |
| `window-chrome.css` | (no changes - titlebar height is token-driven) |
| `window-menu.css` | Title padding 8px, radius 5px; popover min-width 160px, radius 5px, shadow 0 2px 8px; item height 2em, margin 0 4px, radius 4px; separator margin 5px; fadeIn animation |
| `status-bar.css` | Height 22px, font 12px, line-height 22px, edge padding 7px, label padding 0 5px, tabular-nums, hover transition |
| `zones.css` | Tree row 22px, indent 8px, header 11px/700/uppercase |
| `agent-session.css` | Feed centered 780px, 14px gap, 12px inline pad; user bubble right-aligned 12px radius; assistant 13px inset; new `.agent-session__outer`; bar becomes card (18px radius) |
| `prompt-input.css` | Remove border/radius/bg (chromeless inside card) |
| `agent-toolbar.css` | Gap 8px, padding 8px 10px 10px, border-top separator, min-block-size |
| `tool-call-card.css` | Radius 12px, body padding 10px, gap 6px, summary padding 6px 10px |
| `markdown-render.css` | Font 14px/22px for conversation text |
| `agent-session-view.ts` | DOM restructure: toolbar + mic + send inside card |
| `agent-panel.ts` | Auto-launch "chat" on connect; skip agent menu |
| `window-menu.ts` | "New Agent" resets session instead of just focusing panel |
| `main.rs` | `with_browser_accelerator_keys(false)` on Windows |
| Tests (6 files) | Update pinned values to match new layout spec |


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Workshop Cursor parity (workshop_cursor_parity_8f71c0d3)

## Origin

The plan grew from a paste-into-fresh-chat prompt whose premise was that a prior 18-commit rebuild had produced correct components with the wrong composition: "The components work and are tested, but the visual composition does not resemble Cursor's agent panel - the layout, spacing, nesting, and density are wrong." The tokens in `style.css` were already Cursor's palette; the missing piece was "the layout CSS and DOM structure that arranges those tokens into Cursor's actual appearance."

The original prompt was narrowly scoped (agent session panel only; "No new components. No behavior changes. No new dependencies.") and prescribed the method that defines the plan's shape: extract the real values from Cursor's bundled workbench JS/CSS and cite the source for every change. The user later restated this as a hard requirement: "I want everything we need extracted from Cursor and specified in the plan ahead of time." That sentence is why every target value in the plan carries a byte-offset citation into `workbench.glass.main.css/js` or `workbench.desktop.main.css/js`.

## Scope expansion

Mid-chat the user blew the scope open from the agent panel to the whole window, in one decisive message:

- "The CTRL= and CTRL- don't work in the workshop, fix them"
- "I want the menus and the workspace tree control copied as well"
- "I want the status bar fixed to match (proportions, text, spacing, but leave the two LED alone)"
- "I want the chat box in the Agent panel to be identical right now the controls are above when they should be inside"
- "The Agent window has this weird "Launch an agent to start a session" text and a "chat" button, no one asked for that. The Agent window should come up with the chat control already there, and with the Lua program already launched (is that why "chat" because "chat.lua"?) we will sort agent types later"
- "The menu should say New Agent not Agent Window, and it must create a new Agent window each time"
- "At 100% the UI should be identical to Cursor, the spacing, the font sizes, the font, the layout"
- "Look for stray controls that are in the app now which dont belong"
- "dont say composer, call it agent"

The original "no behavior changes" constraint was thereby overridden: auto-launch, the New-Agent session reset, and the zoom fix are all behavior changes the user explicitly requested. The parity bar is the "At 100%" sentence - identity, not resemblance.
## Discarded alternatives

- **Which chat surface to copy.** Cursor ships several chat surfaces (sidebar agent pane, editor-tab/fullscreen agent, inline chat, legacy full-input-box composer) sharing one underlying input card. The user settled it: "I want the Workspace style of Cursor which has the tree of files on the left." The plan therefore targets the sidebar agent pane values (780px column, 16px outer padding, toolbar inside the card); the fullscreen/tab variants were ignored. A screenshot of Cursor's own input bar confirmed the target.
- **The earlier, narrower plan.** `cursor_agent-panel_layout_port_5841d139.plan.md` (agent-panel feed/input only) was superseded by this plan and moved to trash.
- **Numpad-zoom theory.** "I wasn't using the numpad to zoom" killed that line of investigation. The real root cause was WebView2 intercepting Ctrl+= / Ctrl+- at the native layer for its own zoom, hence the one-line Rust fix `with_browser_accelerator_keys(false)`. Numpad support is explicitly out of scope.
- **The agent menu screen.** Rather than restyle the "Launch an agent to start a session" menu, the user wanted it out of the flow: auto-launch "chat" on connect and show the session view immediately. The menu class stays in the codebase as dead code because "we will sort agent types later."
- **New Agent semantics.** The user's words were "it must create a new Agent window each time." The plan implements this as a session reset inside the singleton dock panel (paraphrase: close the WS session, relaunch "chat", clear the feed; the panel itself stays singleton in the dock). Note the residual tension: during execution the user again asked "why isn't it opening a new window?" - the fresh-conversation intent was settled, the window-vs-session reading was not.

## Execution decisions

- **Single commit:** "This can be one commit of course" - this dissolved the plan review's concern about pinned tests breaking between steps; test updates land together with the code.
- **Gateway config UI excluded:** "this doesn't change gateway config ui right?" - confirmed; it lives in its own webview with its own CSS and stays untouched (plan step 8 keeps it).
- **Global 13px body font:** chosen over per-element overrides (paraphrase of the planning discussion: Cursor's UI base is 13px with 14px only for agent conversation text; anything that looks wrong at 13px was already missing an explicit font-size and needs one anyway).
- **Thinking budget for executors:** high across all waves (paraphrase: waves 1-2 are mechanical value swaps with prescribed before/after; wave 3, the auto-launch wiring, is "read the service files, wire one call" - no architectural reasoning).

## Governing sentiment

The execution phase that followed hardened one principle the plan only implies. After repeated near-misses: "Just. Fucking. Copy. Cursor. for fucks sake" - plus a standing instruction to "Update AGENTS.md in the directory with the css and html (or its parent if need be) and append some rules to lock us into Cursor copying of the proper parts." The plan's cite-every-value discipline is the durable expression of that intent: Cursor's bundle is the spec, and approximation is the failure mode.
