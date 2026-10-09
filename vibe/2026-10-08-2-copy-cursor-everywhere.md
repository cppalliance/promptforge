---
name: Copy Cursor everywhere
overview: Make every Workshop surface that has a Cursor counterpart look and behave exactly like Cursor 3.22.12's IDE. That covers the agent transcript, composer, and pane chrome; the title bar and menus; dock layout and explorer; the editor; overlays and the status bar; the run panel, dialogs, and gateway config app. Everything in this plan uses functionality the Workshop already has. Cursor features that need new functionality are deferred and listed in the Decision Record.
todos:
  - id: step-1
    content: "Step 1: IDE tokens, text tiers, inline codicons, shimmer, controls, progress, focus, scrollbars, and two dialog skins in look and ui"
    status: pending
  - id: step-2
    content: "Step 2: reasoning timing, generating and reconnecting flags, tool ids, cancel, error titles, and the pure transcript model"
    status: pending
  - id: step-3
    content: "Step 3: keyed transcript view, rows, markdown, feed scroll, turn footer, context menu, toast token, and the composer error popup"
    status: pending
  - id: step-4
    content: "Step 4: composer, menus, agent keybindings, zone attribute, pane header, chat tabs, rename, empty state, and the lucide removal from ui"
    status: pending
  - id: step-5
    content: "Step 5: title bar, menu skin and structure, zoom keys, About, and the Windows quit accelerator"
    status: pending
  - id: step-6
    content: "Step 6: dockview palette, tabs, dirty dot, watermark, drop zones, default sizes, and the file tree"
    status: pending
  - id: step-7
    content: "Step 7: editor theme, Cursor Dark highlight style, find widget, whitespace modes, and the Shiki theme"
    status: pending
  - id: step-8
    content: "Step 8: quick input, toasts, status bar, command failure toasts, and workbench inputs"
    status: pending
  - id: step-9
    content: "Step 9: run panel as Cursor Settings rows, with form-modal dialogs"
    status: pending
  - id: step-10
    content: "Step 10: gateway config app on Cursor's settings look, then the branch-wide gate"
    status: pending
isProject: false
---

# Copy Cursor everywhere

<product-contract>

## Product Requirements

The Workshop is a Cursor-style workbench for PromptForge. Every surface that has a Cursor counterpart must look, read, and behave exactly like Cursor 3.22.12's IDE. This plan takes everything the Workshop can copy with the functionality it already has. Cursor features that need new Workshop functionality are deferred. The plan is done when every listed surface matches the specifications in Technical Design, and every property those specifications don't list matches Cursor's installed bundle.

- Problem and users:
  - The operator (the repository owner) works in Cursor every day and wants the Workshop indistinguishable from it.
  - Today the Workshop drifts from Cursor in many ways:
    - It shows labels Cursor never shows: "You" over user messages, the model name over every reply, and "Reasoning (model)".
    - It uses values from Cursor's separate Glass window where the IDE's values apply.
    - It keeps dockview's stock Dark+ colors.
    - Its token sheet has swapped text tiers and the wrong palette.
    - It lacks Cursor states such as "Planning next moves", "Thinking", and "Thought 4s".
- Goals:
  - Copy Cursor's look, layout, wording, and behavior for:
    - the agent transcript, composer, and agent pane chrome
    - the title bar and menus
    - dock layout, tabs, and the file tree
    - the code editor
    - quick input, toasts, dialogs, the status bar, and the base controls
    - the run panel
    - the gateway config app
  - Keep every value a token.
  - Leave every changed surface covered by tests.
- Non-goals:
  - Cursor features that need functionality the Workshop doesn't have (listed under Deferred).
  - Cursor's Glass Agents Window and its editor-tab agent.
  - Cursor-only icon glyphs.
  - Account and plan UI.
- Success criteria:
  - Running the Workshop beside Cursor 3.22.12 on Windows with Cursor Dark shows no visible or behavioral difference on any listed surface, other than the deviations recorded under Decisions.
  - Every test suite named in Testing Plan passes.
- Constraints:
  - The reference is Cursor 3.22.12 on Windows with the Cursor Dark theme:
    - its IDE workbench bundle, `C:\Users\Vinnie\AppData\Local\Programs\cursor\resources\app\out\vs\workbench\workbench.desktop.main.css` and `workbench.desktop.main.js`, where newer UI styles are compiled class rules embedded in the JS
    - its theme, `C:\Users\Vinnie\AppData\Local\Programs\cursor\resources\app\extensions\theme-cursor\themes\cursor-dark-color-theme.json`
    - Copy the IDE workbench and its workspace-sidebar agent, not `workbench.glass.main.*`.
  - Reading a value Technical Design doesn't list:
    - Read byte windows from the bundle (`rg -o -b ".{0,400}PATTERN.{0,400}"`), never whole lines.
    - Resolve each custom property to its Cursor Dark value.
    - Record what you found in the commit message.
  - The repository is public (`github.com/vinniefalco/promptforge`). Code, tests, commits, and this plan may carry facts about Cursor (colors, sizes, strings, timings, behaviors). They must not carry Cursor's source code or its proprietary glyphs.
  - Component CSS holds no raw color, size, or spacing values. Values live as custom properties in `crates/workshop/look/tokens.css` or `crates/workshop/ui/src/tokens/component.css`.
  - No change to the agent wire protocol, the server, or any persisted format, with one exception: the Render Whitespace editor setting stores `none`, `selection`, or `all` (see Decision Record). Chat renames last for the session only.
  - The step count stays small: at most 10 steps, each the largest behavior slice one set of tests can cover.
- Open questions: None

## Functional Specification

The operator sees Cursor's agent turn as it happens:

- the submit
- "Planning next moves"
- "Thinking" while the model thinks, settling into "Thought 4s"
- "Explored ..." groups and two-tone tool lines
- the reply streaming in with a word fade

They can stop a turn, retry after an error, and get through a dropped connection. Around the agent, they work in Cursor's title bar, menus, tabs, tree, editor, quick input, toasts, dialogs, run panel, and gateway settings. The workflows and data don't change; their presentation and keyboard model become Cursor's.

- Actors and workflows:
  - One operator using one Workshop window with any number of agent panels.
  - **Agent turn:**
    1. The operator sends.
    2. The feed pins to the bottom and the tail shows "Planning next moves".
    3. Reasoning appears as a collapsed "Thinking" row, which settles to "Thought <duration>".
    4. Tool calls appear as lines or "Explored ..." groups.
    5. The reply streams in.
    6. A footer with Copy follows the finished turn.
  - **Stop:** the stop button or Ctrl+Shift+Backspace cancels the turn. Esc never does.
  - **Chats:**
    - New chat tab: Ctrl+T, Ctrl+N, or the header "+".
    - Close: Ctrl+W.
    - Cycle: Ctrl+[ and Ctrl+].
    - Rename: double-click the tab.
    - Toggle the pane: Ctrl+L or Ctrl+I.
    - Reuse an empty chat: Ctrl+Shift+L or Ctrl+Shift+I.
  - **Workbench:** the menus, title bar, tabs, tree, editor, quick input, toasts, dialogs, run panel, and gateway settings keep their current functions under Cursor's look and keyboard model.
- Inputs and outputs:
  - Inputs: the existing agent socket frames (`crates/workshop/ui/src/services/protocol.ts`), operator input, and the existing workbench services.
  - Outputs: DOM only.
  - The agent wire, the server, and persisted state don't change.
  - The bound tool id is already present in every tool-call batch: `ToolCallEvent.tool` in `crates/promptforge-internal/types/src/metrics.rs`, serialized by `render_tool_calls` in `crates/workshop/server/src/agents/wire.rs`.
- States and validation:
  - **Tail status:** none, "Planning next moves", "Thinking", the running tool's verb and details, or "Reconnecting...".
  - **Composer action button:** mic, send, stop, or disabled. Its color comes from the mode.
  - **Collapsibles:** closed or open. Thinking is collapsed while it streams.
  - **Feed scroll:** pinned or released.
  - **Tabs:** active, inactive, hover, dirty, or in an unfocused group.
  - **Clipped user message:** clipped or expanded.
- Errors and recovery:
  - **Agent errors** show in a popup docked on the composer, not as feed rows. "Try again" re-sends the last user message.
  - **A socket that drops mid-turn** shows "Reconnecting..." in the tail until the next session acknowledgment.
  - **A failed send while the socket is down** raises the popup titled "Connection failed", with the message "The connection was interrupted. Please check your network connection and try again."
  - **A failed command** raises the toast "Command '{0}' resulted in an error".
  - **Gateway field errors** show under their input with `role=alert`.
- Security and privacy behavior:
  - Every model, tool, and user string stays untrusted:
    - Markdown renders through the existing DOMPurify path in `crates/workshop/ui/src/parts/agent/markdown-render.ts`.
    - User text, tool arguments, and tool output land through `textContent`.
  - Hover-card links come from untrusted search results:
    - Only `http:` and `https:` URLs become links.
    - Anything else renders as plain text.
    - A link is a plain anchor like the one the markdown renderer's `link` method builds (`crates/workshop/ui/src/parts/agent/markdown-render.ts`). The desktop shell opens http(s) targets in the system browser (`crates/workshop/desktop/src/navigation.rs`).
  - No new network requests.
- Acceptance criteria:
  - Each surface matches its specification under Technical Design.
  - Properties the specification doesn't list match the reference bundle.
  - Every behavior above has a test.
  - The operator's side-by-side visual check, run after the last step, passes or records each deviation.

</product-contract>
<implementation-contract>

## Technical Design

A shared foundation in `@workshop/look` comes first: Cursor's IDE token values, codicons, a tunable shimmer, Cursor's controls, and two dialog skins. Every surface builds on it. The agent transcript gets a real structure: a DOM-free transcript model that turns session items into turns, rows, and a tail status. The view reconciles those rows by key, so rows update in place. Every other surface is a restyle plus small behavior changes inside the module that already owns it. Surface specifications below hold every value and rule an implementer needs; for any property they don't list, use the reference bundle.

- Architecture:
  - The agent pipeline:

    ```mermaid
    flowchart LR
      Wire[AgentSocket] --> Service[SessionService]
      Service -->|"items, generating"| Model[TranscriptModel]
      Model -->|"turns, rows, tail"| View[SessionView]
      View --> Rows[RowComponents]
      View --> Scroll[FeedScroll]
      Rows --> Collapsible[Collapsible]
    ```

  - The service stays DOM-free.
  - The model is a pure function: `(items, generating, reconnecting) -> { turns, tail }`, where every row carries a stable key.
  - The view replaces today's item-identity prefix diff with a keyed reconcile. It reuses the component for a key and calls `update(row)`, creates new ones, removes ones that are gone, and places the tail last.
  - Open and closed state lives in the row components, so it survives every delta.
  - Other surfaces keep their module structure and change in place.
- Modules and interfaces:
  - **`AgentSessionService`** (`crates/workshop/ui/src/services/agent-session.ts`):
    - The constructor takes an optional `now: () => number`, defaulting to `performance.now()`.
    - `ReasoningItem` gains `startedAt` and `endedAt: number | null`.
    - New `generating: boolean` and `reconnecting: boolean`, with change emitters.
    - `ToolCallRow` gains `tool: string | null`.
    - `ErrorItem` gains `title: string | null`.
    - New `cancelTurn(): boolean`.
  - **`TOAST_STACK`** (`crates/workshop/ui/src/services/toast-service.ts`, new): a service token for the `ToastStack` that `crates/workshop/ui/src/main.ts` creates (line 137), defined the way `STT_STATUS` is in `services/stt-status.ts`. `main.ts` registers the stack, so the agent view, the status bar, and the command-failure path reach it without importing the app.
    - `AgentSessionWire` gains `cancelTurn()` and `onDisconnect`. `AgentSocket` already has both (`crates/workshop/ui/src/services/agent-socket.ts`).
  - **Transcript model** (`crates/workshop/ui/src/parts/agent/transcript/transcript-model.ts`, pure), plus tool labels (`tool-labels.ts`):
    - Exports the row types:
      - `human`
      - `markdown`
      - `thought`
      - `group`, holding thinking steps and tool steps
      - `tool`, holding one tool step
    - Each row has a stable key:
      - `h:<item index>` for a human row
      - `r:<reply>` for a reply
      - `t:<reply>` for a thought
      - a group takes its first step's key
      - `c:<turn>:<call id>` for a tool step, falling back to the item index
    - Exports the tail status union.
  - **Row components** in `crates/workshop/ui/src/parts/agent/transcript/`:
    - `collapsible.ts` (shared header, chevron, and animated body)
    - `human-message.ts`
    - `tool-line.ts`
    - `web-hover-card.ts`
    - `feed-scroll.ts`
    - styles in `collapsible.css`, `human-message.css`, `tool-line.css`, and `transcript.css`
  - **`ChatBox`** (`crates/workshop/ui/src/parts/chatbox/`):
    - The action prop gains `stop`, and the mic folds into the action button.
    - New `mode` prop (`agent | plan | ask | debug | multitask`) colors the button.
    - New error popup props: title, message, and an optional Try again.
  - **`@workshop/look`** (`crates/workshop/look/`):
    - new `shimmer.ts` exporting `setShimmer(element, on)`, exported as `"./shimmer"`
    - codicons as inline SVG strings in `icons.ts`, checked against the `@vscode/codicons` devDependency
    - controls: button, input, select, and a new `.switch`
    - two dialog skins in `modal.ts` / `modal.css`, where `ModalButton` (`modal.ts:30`) gains `primary`
    - progress variants
  - **Tab renderer** (`crates/workshop/ui/src/parts/layout/panel-tab.ts`) gains `setTabDirty(panelId, dirty)`, modeled on `setTabLoading`.
- File and public API changes:
  - `crates/workshop/look/`:
    - `tokens.css`
    - `shimmer.css`, plus a new `shimmer.ts`
    - `controls.css`
    - `modal.ts`, `modal.css`
    - `progress.ts`, `progress.css`
    - `toast.ts`, `toast.css`
    - `status-bar.css`
    - `dropdown.css`
    - `icons.ts`
    - `package.json`: the `"./shimmer"` export and the `@vscode/codicons` dependency
    - `test/boundary.mjs` and `test/icons.mjs`: drop the `lucide` allowance and the lucide viewBox checks
    - `crates/workshop/package-lock.json`: the one lockfile follows the dependency changes
    - `THIRD_PARTY_NOTICES.md`
    - `AGENTS.md`: its focus bullet defers to Cursor
  - `crates/workshop/ui/`:
    - `style.css`: remove the global `:focus { outline: none !important }` at line 75
    - `src/tokens/component.css`
    - `src/main.ts`: `hideBorders`, the watermark, the drop resolver, default sizes
  - `crates/workshop/ui/src/services/agent-session.ts`
  - `crates/workshop/ui/src/parts/agent/`:
    - `agent-session-view.ts`, `agent-session.css`
    - `markdown-render.ts`, `markdown-render.css`
    - `agent-toolbar.*`, `mode-chip.*`
    - `agent-panel.ts`, `agent-menu.ts`
    - `agent.contribution.ts`: the title "Agent Session" at line 26 becomes "New Agent"
    - the new `transcript/` files
  - Retire `tool-call-card.ts`, `tool-call-card.css`, and `crates/workshop/ui/test/tool-call-card.mjs`. Move them to the workspace's trash folder, `c:\Users\Vinnie\cursor\cabinet\_trash\`, and stage their removal.
  - `crates/workshop/ui/src/parts/chatbox/`:
    - `chat-box.ts`, `chat-box-view.ts`, `chat-box.css`
    - `types.ts`
    - `typeahead-popup.*`
    - `chip-view.ts`
  - `crates/workshop/ui/src/parts/chrome/`:
    - `command-center.*`
    - `window-chrome.*`
    - `model-picker-trigger.*`
    - `token-ring.*`
    - `about-dialog.*`
    - `update-view.*`
    - `zoom.ts`
  - `crates/workshop/ui/src/parts/menu/`:
    - `menu.ts`, `menubar.ts`, `window-menu.css`
    - `menubar.contribution.ts`, `edit.contribution.ts`
    - `stubs.contribution.ts`: drop `ctrlcmd+l` and `ctrlcmd+shift+l` from the two "Add Symbol to ... Chat" stubs at lines 134 and 136
  - `crates/workshop/platform/keybinding-parser.ts`: add the NumPad add and subtract key names
  - `crates/workshop/desktop/src/menu.rs`: the quit item
  - `crates/workshop/ui/src/parts/layout/`:
    - `zones.ts`, `zones.css`
    - `panel-tab.ts`
    - `keybinding-dispatcher.ts`: the idle "Ready" at line 83
  - `crates/workshop/ui/src/parts/editor/`:
    - `editor-surface.ts`: `promptforgeTheme` at line 117, `promptforgeHighlight` at line 178
    - `editor-panel.ts`: the dirty title prefix at line 394
    - `editor-panel.css`
    - `goto-line.ts`
    - `editor-settings-service.ts`
  - `crates/workshop/ui/src/parts/workspace/workshop-panel.ts`, `workshop-panel.css`
  - `crates/workshop/ui/src/parts/quickinput/*`
  - `crates/workshop/ui/src/parts/status/*`
  - `crates/workshop/ui/src/parts/run/run-panel.ts`, `run-rows.ts` (`argControl`), `run-panel.css`
  - `crates/gateway/config-ui/ui/src/`:
    - `components/tab-bar.ts`
    - `components/confirm-modal.ts`
    - `components/toggle-control.ts`
    - `pages/*.ts`
    - `styles/*.css`
  - `crates/shared-ui/tokens.css`
- Data, persistence, failure, security, and privacy constraints:
  - No wire, server, or persisted-format change.
  - `ToolCallRow.tool` is parsed from the existing batch JSON. Anything malformed parses to `null`, which renders as a generic tool.
  - Untrusted strings are handled as described in Functional Specification.
  - The hover card turns only `http:` and `https:` URLs into links.
  - The codicon artwork is CC-BY-4.0. `crates/workshop/look/THIRD_PARTY_NOTICES.md` records it, and `@vscode/codicons` is a `devDependency` that only a `look` test reads.
  - Animations respect `prefers-reduced-motion`:
    - shimmers stop and show the row's resting color
    - height and fade animations are dropped
    - scrolling jumps instead of easing
- Surface specifications (Cursor Dark resolved values; text tiers mean `--cursor-text-primary` #F0F0F0, secondary 74%, tertiary 60%, quaternary 36%):
  - **Foundation** (`crates/workshop/look/`):
    - **Tokens.** Use the IDE override values:
      - red #E34671, blue #81A1C1, cyan #88C0D0
      - shadow ink #00000066, 60%, and 30%, with base and xl box-shadow stacks
      - `--cursor-bg-active` and `--cursor-bg-focused` #F0F0F01E
      - `--cursor-sidebar` #141414
      - add `--cursor-text-link` #81A1C1, accent hover and accent at 8%, input field background, text-invert, disabled control colors, a 2px radius, and a 300ms duration
      - Make `--text-tertiary` and `--text-quaternary` aliases of `--cursor-text-tertiary` and `--cursor-text-quaternary`; today their values are swapped.
      - `--code-font: Consolas, Menlo, Monaco, "Droid Sans Mono", "Courier New", monospace`
    - **Icons.**
      - Codicons ship as inline SVG strings from `@vscode/codicons` (`src/icons/<name>.svg`) and replace every lucide icon in `look` and `ui`. The set is exactly what the specifications use: close, chevron-right, chevron-down, mic, arrow-up, stop-circle, add, ellipsis, trash, new-folder, warning, info, error, and check.
      - The gateway config app and `shared-ui` keep their own lucide icons.
      - Cursor-only glyphs such as `restore` have no codicon and no consumer here, so none is added.
    - **Shimmer.**
      - `.ws-shimmer-text` reads `--shimmer-base`, `--shimmer-peak`, and `--shimmer-duration`.
        - The defaults stay the current values: a 2s loop, from `--cursor-text-primary` at 60% to `--cursor-text-primary`.
        - Under reduced motion the row keeps its own resting color.
      - Transcript tones, all at 1s:
        - tail status: secondary at 60% to secondary
        - thinking header: tertiary at 60% to secondary
        - tool verb: secondary at 60% in the muted scope to #F0F0F0 at 60%
      - `setShimmer` sets a negative `animation-delay` measured from one module start time, so every sweep stays in phase and rebuilt rows don't restart it. `panel-tab.ts` drops its private copy and uses it.
    - **Controls.**
      - `.button`: 24px tall, 8px padding, 6px radius, 13px weight 400.
        - `-sm`: 20px tall with a 4px radius.
        - primary: text #181818; hover mixes 10% white into the accent; it replaces the gold primary.
        - secondary: 8% fill, 14% on hover.
        - ghost; outline (transparent with a 12% border); danger.
      - `.input`: 24px tall, padding 4px 5px, 1px border at 12%, 6px radius, 3% fill, 13px/18px; the border rises to 20% on focus; 150ms transitions.
      - `.select`: 12px text, 12% border, 6px radius, padding 3px 2px 3px 6px.
        - its menu: #181818 with a #F0F0F013 border, 6px radius, at least 160px wide, items padded 5px 8px at 0.6 opacity (1 on hover)
      - `.switch`: a 32x20 track, 14% white when off, #3FA266 when on, with a 16px white thumb moving 12px over 200ms.
    - **Progress.**
      - Settings variant: a 4px bar with a 2px radius, a 12% track, and a 200ms transform. Indeterminate mode, used when the fraction is null, slides a 34% bar over 1.35s.
      - Workbench variant: 2px, square ends, green, 0.1s linear.
    - **Focus.** A 1px #F0F0F026 outline at offset -1px on list rows, tree rows, menu rows, and inputs.
    - **Scrollbars.** Views use 10px square bars shown on hover. Menus use 7px.
    - **Confirmation dialog** (save, revert, overwrite):
      - 0.5 scrim, 200px from the top
      - width from 300px to `min(560px, 92vw)`
      - #181818 with a 1px #F0F0F013 border, 8px radius, `0 4px 20px rgba(0,0,0,.15)`
      - 12px padding and text, 4px between buttons
      - Windows button order
      - wording: "Do you want to save the changes you made to {file}?" / "Your changes will be lost if you don't save them." / Save, Don't Save, Cancel
    - **Form modal** (Add Folder, Choose Prompt, Choose Input, gateway confirms):
      - a 320px card with a 12px radius, 12% stroke, no shadow
      - header padding 12px 16px with a 14px title; body padding 12px 16px 16px
      - footer padding 10px with a top border at 8%; Cancel first, primary last
      - backdrop rgba(0,0,0,.5); opens in 300ms, scaling up from 0.97
      - Delete the per-dialog copies in `zones.css` and `editor-panel.css`.

  - **Agent transcript, service rules:**
    - **Reasoning timing:**
      - `startedAt` is set on a round's first reasoning delta.
      - `endedAt` is set on the first text delta of the same reply, or on an error or `input_required` while still open.
      - The durable `agent_thought` copies both from the pending item it replaces.
    - **`generating`:**
      - Turns on when `respond()` succeeds, and when a `tool_call_update` answers a call of a model tool-call batch.
      - Turns off on `agent_message`, `input_required`, an error, or a session acknowledgment with a new session id.
      - The built-in chat agent loops ask, then model loop (`crates/workshop/agents/agents/chat.md`), so a reply always ends the turn.
    - **`cancelTurn()`:** the server answers a cancel with nothing, so after a successful send the service settles its own view. It turns `generating` off and closes any open thinking (sets `endedAt`).
    - **`reconnecting`:** turns on at `onDisconnect` and off at the next session acknowledgment.
    - **`ToolCallRow.tool`:** the batch entry's `tool` string (the wire serializes the bound tool id as one slash-separated string), or `null`.
    - **`ErrorItem.title`:** `null` for server errors, which the popup titles "Connection Error". A failed `respond()` while the socket is down folds an error with the title "Connection failed" and the message "The connection was interrupted. Please check your network connection and try again."
  - **Agent transcript, model rules:**
    - **Turns:**
      - A user item starts a turn.
      - A tool result attaches to its call by id within its turn and never becomes a row.
    - **Grouping:**
      - Reasoning items and tool calls append to an open group. A batch splits into one step per call.
      - A reply, a user item, or an error closes the open group.
      - Ask-tool calls are always standalone.
      - When a group closes:
        - one tool call and nothing else becomes a `tool` row
        - thinking only becomes a `thought` row
        - anything else becomes a `group` row
    - **Thought label:**
      - While streaming the action is `Thinking`, with no details.
      - When settled the action is `Thought`, with details:
        - `briefly` when the duration is unknown or under 500ms
        - otherwise `${Math.round(ms / 1000)}s`, which gives "4s" and "300s"
        - There is never a "for", and never minutes or hours.
      - **Title-only thinking:** when every non-blank line is `**title**` or `# title`, the last title replaces `Thought`. Its details then follow different rules:
        - none under 500ms
        - otherwise `Ns`
        - when the rounded seconds are 0, one decimal (`0.6s`)
      - A thought row's duration is the sum of its thoughts' durations.
    - **Group summary:**
      - The verb is `Exploring` while the group is the last row of a generating turn, otherwise `Explored`.
      - Parts, in this order: `N search` / `N searches`, `N fetch` / `N fetches`, `N tool` / `N tools`, joined with ", ". There is no "and".
      - Numbers use tabular figures.
    - **Tool lines:**
      - `web/search`: `Searching web` / `Searched web`; details are the `query` argument.
      - `web/fetch`: `Fetching page` / `Fetched page`; details are the `url` argument.
      - an ask tool: `Asking questions` / `Asked questions`.
      - any other tool: `Running` / `Ran`; details are `<call name> in <tool id namespace>`, with the call name in the verb's color.
      - A call is loading while it has no result and the turn is generating. An ask call is also loading while it waits for the operator's answer, because `generating` is off during a wait.
    - **Tail status:**
      - None when not generating, or while the last row is streaming non-empty assistant text.
      - `Reconnecting...` (three ASCII dots) while reconnecting. It has no Cancel action, because a cancel frame can't be sent over a down socket.
      - Otherwise, scan back to the turn's user row:
        - A running tool gives its loading verb plus details, unless its own standalone tool row shows it.
        - Streaming thinking gives `Thinking`, unless a thought row shows it.
      - Failing both, `Planning next moves`.
      - When the last row is an open group that is the active tail, the tail renders inside that group.
  - **Agent transcript, rows:**
    - **Collapsible:**
      - The header is a flex row with a 4px gap, 14px/22px text.
        - action in tertiary, details in quaternary with `font-variant-numeric: tabular-nums`, weight 400
        - hovering doesn't brighten the text
      - A trailing 12px `chevron-right` sits after the details with a 3px margin.
        - opacity 0 until the header is hovered; stays visible while open
        - color #F0F0F085, rising to #F0F0F0A8 on hover or open
        - rotates 90deg when open over 150ms; color and opacity fade out over 100ms
      - The body starts 14px below the header and animates its height from 0 over 150ms with `cubic-bezier(0.215, 0.61, 0.355, 1)`.
      - Default closed.
    - **Human message:**
      - The row: no label, full column width, at least 150px wide.
      - Fill and border:
        - fill `color-mix(in srgb, var(--bg-input) 90%, var(--bg-raised))`, which renders #1F1F1F
        - 1px `--cursor-stroke-secondary` border, 12px radius
        - hover: border `--cursor-stroke-primary`, fill about #202020, over 0.1s
      - Text: padding 8px 10px; 14px/22px #F0F0F0; `pre-wrap`; `overflow-wrap: break-word`.
      - **Clip:** past 77px (3.5 lines) the text is clipped, with a mask fading the last 41px (2px top and 8px bottom bleed).
        - Clicking toggles expanded and clipped.
        - The cursor is a pointer only when the message overflows.
      - **Sticky:** `position: sticky; top: 0; z-index: 101` inside its turn wrapper, over the panel color, with a 4px bottom margin.
        - A 20px fade sits below it, shown only while stuck.
        - It doesn't stick when taller than the scroller.
    - **Assistant markdown row:** no label, no bubble, an 11px inline inset. A new row fades in and rises 8px over 160ms.
    - **Thought row:**
      - A collapsible following the thought label rules. The header shimmers with the thinking tone while streaming.
      - It stays closed while streaming unless the operator opens it.
      - Its body is the thinking markdown, 14px/22px #F0F0F0 at opacity 0.5, with no max height.
    - **Group row:**
      - A collapsible holding the summary.
      - Expanded, its steps sit 4px apart inside a muted scope: secondary maps to tertiary and tertiary maps to quaternary.
      - Thinking steps render as inner thinking lines. They're open while streaming unless closed, and closed once settled.
    - **Tool row and tool step:**
      - A flex line with a 4px gap, `white-space: nowrap`, `user-select: none`, 14px/22px.
        - The verb never shrinks.
        - The details truncate with an ellipsis.
        - Inside the muted scope: verb at 60%, details at 36%.
      - While loading only the verb shimmers.
      - No icons, dots, or badges.
      - **Search and fetch lines** aren't expandable. They get a hover card:
        - opens after 600ms; disabled while loading
        - 360px wide and at most 280px tall, placed bottom-start
        - lists result titles and URLs parsed from the search result `{query, results: [{title, url, description}]}` (`crates/plugin-web/src/provider.rs`); a fetch lists its url
        - a result that doesn't parse as that shape gives no card, and the line stays plain
        - clicking a link opens it and closes the card
      - **Generic tools** expand into one block when they have args or a result:
        - #181818 background, 1px #F0F0F014 border, 8px radius, 4px top spacing
        - one `<pre>` in mono 13px/18px in secondary, padding 6px 10px, `pre-wrap`, `overflow-wrap: anywhere`, scrolling past 240px
        - contents: `JSON.stringify(args, null, 2)`, a blank line, then the result
    - **No error rows.** Errors go to the composer's error popup.
  - **Agent transcript, markdown** (`markdown-render.ts`, `markdown-render.css`):
    - Body text 14px/22px #F0F0F0 at weight 400 with normal letter-spacing.
    - Blocks are 16px apart. A paragraph before a list gets 8px, before a code block or table 12px, and inside a list item 0.
    - Bold is weight 590.
    - Headings:
      - sizes: h1 1.428em, h2 1.318em, h3 1.214em, h4 1em, h5 0.866em, h6 0.8em; h5 and h6 in secondary
      - weight 590, line height 1.42
      - margin above: 20.5px for h1 to h3, 12px for h4 to h6; 4px below
    - Lists: 8px above, 18px below, a 2em indent; items in a flex column with an 8px gap.
    - Inline code: 0.9em mono, padding 1.5px 2px, 5px radius, #F0F0F014 fill, no border.
    - Code blocks:
      - 12px margin, 1px #F0F0F01F border, 12px radius, #181818 background
      - 12px/18px text, 6px vertical and 10px horizontal padding
      - a copy button at top-right 4px, shown on hover, reading "Copy code" then "Copied" in green
      - no language label
    - Links: #81A1C1, weight 500, never underlined. Hover adds a 24% blue fill with a 4px radius and 2px padding.
    - Blockquote: a 3px rounded bar at #F0F0F01F, 16px left padding, text in secondary.
    - Tables: a 12px-radius card with cells padded 6px 10px.
    - `hr`: 16px margin, 1px at 8%.
    - **Streaming fade:**
      - No caret.
      - New text fades in from 0 to 1 over 150ms (linear), split at whitespace.
      - Each render wraps only text runs younger than 150ms, with a negative `animation-delay` so they continue across renders.
      - `pre`, `svg`, and `math` are skipped.
  - **Agent transcript, feed:**
    - Panel background #181818.
    - A centered column at most 840px wide, with 16px side padding.
    - Each user item starts a turn wrapper.
    - Rows are 14px apart with an 11px inline inset and no block padding.
    - A trailing spacer of 14px plus `clamp(80px, round(20% of scroller height), 240px)`, set from JS.
    - **Scroll** (`feed-scroll.ts`):
      - Pinned when within 4px of the bottom.
      - Scrolling up within a 250ms input window unpins; reaching the bottom re-pins.
      - While pinned, growth up to one viewport height follows smoothly, at most 3.6px/ms and snapping after 250ms.
      - Bigger growth, or reduced motion, jumps.
      - A send forces an instant pin.
      - A `ResizeObserver` detects growth.
  - **Composer** (`crates/workshop/ui/src/parts/chatbox/`, `agent-toolbar.*`, `mode-chip.*`, `model-picker-trigger.*`, `token-ring.*`):
    - The sidebar composer copies Cursor's older input box, not its React prompt input. Replace the 36px minimum height, 200px maximum, 6% fill, and 18px radius taken from the React input. The 12px radius and the 8/10/6 padding stay.
    - **Card:** fill `color-mix(in srgb, var(--bg-input) 90%, transparent)` (about #202020); border and shadow transitions of 100ms ease-in-out; 10px space on each side.
    - **Editor:** 14px/22px with no letter-spacing; spellcheck and autocapitalize off; height grows from 22px to 240px.
    - **Toolbar:** a fixed 28px row with 4px gaps, 9px above it, and 4px before the action button. Below 300px the mode label hides; below 260px the ring hides.
    - **Action button:** one round button.
      - With an empty box and an idle agent it's the mic: 66% fill, #181818 glyph, tooltip "Voice Input (Ctrl+Shift+Space)".
      - With text it's the send arrow.
      - While the agent is generating it's stop: tooltip "Stop (Ctrl+Shift+Backspace)", calling `cancelTurn()`.
      - Fill by mode:
        - Agent #F0F0F0 with a #141414 glyph
        - Plan #F1B467
        - Ask #3FA266
        - Debug and Multitask #F0F0F0 at 80%
      - Disabled keeps the fill at opacity 0.3.
      - The separate mic button goes.
    - **Mode chip:**
      - 20px tall, padding 2px 4px 2px 8px; a 16px icon at 0.5 opacity, the label at 0.8, a 14px chevron at 0.5.
      - Mode tints use a 24% fill with colored text: Plan yellow, Ask green, Debug #E34671, Multitask #B180D7.
      - Tooltip "Switch Agent Mode (Ctrl+.)".
      - Its menu: check-only selection, a description on each row, opens upward at the chip's left minus 6px, at least 170px wide.
    - **Menu surface** for the mode, @, and model menus:
      - #181818 with a 1px #F0F0F026 border, 6px radius, 2px padding
      - rows padded 2px 6px with a 4px radius, 12px text, 14px icons
      - highlight #F0F0F011, a 10px check
    - **@ menu:**
      - 240px wide, at most 280px tall
      - each row's path is an 11px right-aligned subtitle at 0.4 opacity (0.6 when highlighted), truncated from the left
      - section headers aren't uppercase and have no border
    - **Mention chips in the input:**
      - 6px radius, padding 1px 4px, 24% blue fill, label at most 200px wide
      - the close X replaces the icon on hover, only while the editor is focused
    - **Model button and menu:**
      - Button: 20px tall, 4px radius, 6px side padding, 13px/18px in secondary, 8% fill on hover, a 9px chevron at 0.7, tooltip "Switch Model (Ctrl+/)".
      - Menu: 230px wide, at most 320px tall, check-only selection, empty text "No models found".
    - **Context ring:** 15px with stroke 2 and radius 5.5, round line caps, an 8% track, in a 20px box. Hidden while there's no usage data.
    - **Placeholder:** "Add a follow-up" while generating; otherwise "Plan, Build, / for skills, @ for context".
    - **Keys:**
      - Enter sends and ignores auto-repeat. Ctrl+Enter sends.
      - Ctrl+. or Shift+Tab opens the mode menu, and pressing again cycles modes.
      - Ctrl+/ opens the model menu.
    - **Error popup:**
      - Docked on the card's top edge: padding 12px, 8px radius, about #1E1E1E, 12% border.
      - A `warning` codicon in #F1B467.
      - Title 12px at weight 500, defaulting to "Connection Error", with the message below.
      - A right-aligned secondary "Try again" button (6px gap) re-sends the last user item's text through `respond()`. It is enabled only while a wait is pinned, because the chat agent returns to its ask after a failed round. The resend adds a second user message to the agent's history.
  - **Agent pane chrome** (`agent.contribution.ts`, `agent-panel.ts`, `agent-menu.ts`, `panel-tab.ts`, `zones.ts`):
    - **Stop and title:**
      - `composer.cancelComposerStep` is bound to Ctrl+Shift+Backspace while the active editor is an agent panel. Esc is never bound to stop.
      - The panel type's default title is "New Agent".
    - **Keybindings:**
      - Ctrl+L and Ctrl+I, "Open Chat": hide the pane when the chat has focus; otherwise reveal it and focus the input.
      - Ctrl+Shift+L and Ctrl+Shift+I: reuse an empty agent panel, or open a new instance.
      - Ctrl+T while an agent panel is active, and Ctrl+N while one is active and the editor text isn't focused: New Chat Tab. Both reuse an empty chat. Every agent chord in this list registers at `KeybindingWeight.BuiltinExtension` (`crates/workshop/platform/keybinding-registry.ts`), so it beats the menu stubs that share a chord (Go menu "Add Symbol", Toggle Developer Tools, Select All Occurrences), which register at the default `WorkbenchContrib`.
      - Ctrl+W closes the agent tab. Closing the last one hides the right zone.
      - Ctrl+[ and Ctrl+] cycle chat tabs.
    - **Right-group header:**
      - a 35px row on #181818 holding New Agent (+), More Actions (...), and Close (x)
      - 24px buttons with a 6px radius and 2px gap; icons #F0F0F084; hover #5a5d5e50
      - The "..." menu: Toggle Chat Pane, then Close Tab, Close Other Tabs, Close All Tabs.
    - **Chat tabs:**
      - padding 3px 6px, 6px radius, 13px/18px labels at weight 400
      - 60 to 120px wide; the active tab can grow to 200px, on about #252525
      - inactive labels #F0F0F0BD
      - close icon on hover only; middle-click closes
    - **Rename:** double-click for an inline rename, plus a tab-menu "Rename Chat" (prompt "Enter new chat name", placeholder "Chat name"). Renames last for the session only.
    - **Turn footer:** a 28px row after each finished turn, which is every turn but the last, and the last once `generating` is off. It holds Copy, which puts the turn's reply texts (markdown source, joined by a blank line) on the clipboard. The icon swaps to a check, and the toast reads "Message copied to clipboard".
    - **Transcript context menu:** Copy Message (or Copy when text is selected), Select All, Search with Google.
    - **Empty state:** nothing above the composer, which sits at the bottom. The launcher's lead text goes.
    - **Right zone:** at least 300px wide, preferring `min(400px, W/4)`.

  - **Title bar** (`crates/workshop/ui/src/parts/chrome/`):
    - **Center:** a 12px folder-name button (padding 1px 6px, 6px radius, hover #FFFFFF14) replaces the command-center pill and its "?". Mousedown opens quick open.
    - **Window controls:**
      - glyphs #F0F0F084, hover #FFFFFF1A, close hover #E81123E6
      - they keep their physical size when the window is zoomed
    - **App icon:** 16px, centered in a 35px box flush to the left edge.
    - **Inactive window:** title-bar text #F0F0F099.
    - **Toolbars:**
      - left: Toggle Primary Side Bar
      - right: Toggle Agents (the right zone) and a settings gear
      - 22x22 buttons, hover #5A5D5E50
    - **About:**
      - a 380px card with the icon, and version and date copy buttons
      - "Copy version info" on Enter; Escape closes
      - Cursor's update-button labels
    - **Zoom:**
      - a factor of 1.2 per level, from -8 to 8
      - reset on Ctrl+NumPad0
      - Ctrl+NumPad+ and Ctrl+NumPad- once the parser knows those key names
  - **Menus** (`crates/workshop/ui/src/parts/menu/`):
    - **Skin:**
      - row hover and focus #F0F0F01E
      - rows have no padding; the label and shortcut are each padded 0 2em
      - shortcut: 13px, right-aligned, opacity 0.7, full on hover
      - check marks in an absolute 2em left column, in the text color
      - submenu chevron: 16px at 0.7, about 9px from the right
      - disabled rows: label #CCCCCC80, row opacity 0.5, shortcut 0.4
      - 5px radius, outline instead of a border, no fade on top-level dropdowns
      - max height is the window height minus the menu's top minus 35px, scrolling with a 7px scrollbar
    - **Behavior:**
      - The menubar opens on mousedown, and rows fire on mouseup (press, drag, release).
      - Submenus open after 250ms and close after 750ms.
      - Submenus and context menus flip to stay on screen.
    - **Structure:**
      - "Duplicate Workspace" loses its "...".
      - Share becomes its own group.
      - "Export Profile (Default)..." replaces "Export Profile...", and Import Profile goes.
      - Show Release Notes moves into the first Help group.
      - New stubs: "Open Extension Monitor" and "Split in Group".
      - Shortcut labels: Ctrl+Shift+I (Toggle Developer Tools), Ctrl+Q (Open View...), Ctrl+Shift+L (Select All Occurrences).
      - On Windows, Ctrl+Q opens View instead of quitting, including in `crates/workshop/desktop/src/menu.rs`.
  - **Layout and tabs** (`zones.css`, `zones.ts`, `panel-tab.ts`, `main.ts`):
    - **Dockview palette:**
      - groups #181818, tab strip #141414
      - active tab #181818, inactive #141414
      - tab text #F0F0F0 and #F0F0F05C; unfocused groups #F0F0F0BD
      - separators #F0F0F013, drop overlay #F0F0F011
      - sash hover #F0F0F026 after 300ms, fading in over 100ms
    - **Borders and zones:**
      - `hideBorders: false` gives 1px #F0F0F013 borders between parts.
      - Each zone's group carries `data-ws-zone`, stamped at open, rebuild, and restore.
    - **Tabs:**
      - 35px tall, 10px left padding, at least 120px wide
      - 1px #F0F0F013 right border and strip underline; the active tab gets a #181818 underline so it merges into the editor
      - hover #2A2A2AB3 only for tabs in unfocused groups
    - **Close button:**
      - 20x20 in a 28px slot, 5px radius, hover #5a5d5e50
      - hidden by default; fully visible on the active or hovered tab in the active group, half visible in other groups
      - tabs without a close get 10px right padding
    - **Dirty state:**
      - Drop the "● " title prefix in `editor-panel.ts`.
      - Use `setTabDirty`: a dot replaces the × until hover.
    - **Empty editor group:** its tab strip hides and its background is #141414.
    - **Watermark** via `createWatermarkComponent`:
      - centered, at most 500px wide, opacity 0.5, text #F0F0F099, a 100px product mark
      - shortcut rows with key caps, padded 4px 4px 4px 8px with a 4px radius
      - in order: New Agent, Show Files, Search Files, and Add Folder when exactly one root is open
      - only commands that have a keybinding; rows hide below 478px
    - **Docking:**
      - a `dropPositionResolver` with 10% edge bands, then thirds
      - `dndTabIndicator: "line"` (a 1px #F0F0F0 line)
      - `disableTabsOverflowList: true`
    - **Active group:** tracked by the Workshop, so focusing the tree or the agent doesn't dim the editor tabs.
    - **Left zone header:** centered pills with a 2px gap, 22px tall, 4px radius, 12px/500 text in #F0F0F0BD, hover #F0F0F011.
    - **Default sizes:**
      - the empty editor group exists at startup
      - tree `min(300px, W/4)`, at least 214px, replacing the fixed 280 in `main.ts`
      - agent `min(400px, W/4)`, at least 300px
  - **File tree** (`workshop-panel.ts`, `workshop-panel.css`):
    - **Skin:**
      - text #F0F0F0BD, hover #F0F0F011
      - focus #F0F0F01E with a 1px #F0F0F026 outline
      - no padding and no reserved scrollbar gutter
    - **Geometry:**
      - full-width rows driven by a depth variable, with a 4px inset
      - a twistie box of 8px per depth plus 16px plus 6px, holding a 16px chevron rotated -90deg when collapsed
      - file names start at 34 + 8 x (depth - 1) px
    - **Indent guides:** 1px lines at x = 20 + 8 x (k - 1); #F0F0F013 while the tree is hovered, #F0F0F030 for the list holding the focused row.
    - **Section header:** a 16px chevron, then the title in #F0F0F05C on #141414, with no bottom border. Actions appear on hover as 20px buttons with a 5px radius.
    - **Empty tree:** "You have not yet added a folder to the workspace." with an "Open Folder" button.
    - **Drag pill:** #F0F0F01E, 10px radius, 12px text.
  - **Editor** (`editor-surface.ts`, `editor-panel.css`, `goto-line.ts`, `editor-settings-service.ts`). Cursor uses stock Monaco defaults, except that the minimap is off by default.
    - **Text:** 14px with a 19px line height, set on `.cm-scroller` so line numbers share it.
    - **Tokens:** a new `--editor-*` block for colors and sizes, plus `--editor-token-*` for syntax.
    - **Gutter:**
      - #181818 background, no right border
      - line numbers #F0F0F05C, the active number #F0F0F0, no tint on the active line's gutter
      - a 19px glyph margin, line numbers at least 5 characters wide, a 26px fold lane, no content padding
    - **Current line:** #262626, hidden whenever a selection exists (a has-selection class via `editorAttributes`).
    - **Selection:** #40404099 focused, #40404077 unfocused, 3px corners. Overriding CodeMirror's built-in dark theme needs the long `.cm-selectionLayer` selector.
    - **Cursor and focus:** a 2px #F0F0F0 bar shifted 1px left, `drawSelection({ cursorBlinkRate: 1000 })`, and no focus outline on the editor.
    - **Highlights:**
      - find matches #88C0D044, current match #88C0D066, no outline
      - word under the cursor #F0F0F01E; other occurrences of a selection #404040CC
      - bracket match box #F0F0F01E with a transparent 1px border, shown even when the editor is unfocused
    - **Syntax:**
      - `promptforgeHighlight` maps lezer tags onto Cursor Dark's `tokenColors` from the reference theme file. Read the scope colors from that file.
      - The agent's Shiki theme uses Cursor's CSS-variables theme, with `--syntax-fg` #F0F0F0, `--syntax-comment` #F0F0F099 italic, and every slot filled.
    - **Folding:** codicon chevrons in #C5C5C5 that fade in on gutter hover over 0.5s; the folded placeholder is `⋯` in #808080.
    - **Behavior:**
      - `scrollPastEnd()`, and a 4-space indent unit
      - Alt+click adds a cursor; Shift+Alt+drag selects a column; drop `crosshairCursor`
    - **Scrollbars:** 14px vertical and 12px horizontal, square, #F0F0F011 and #F0F0F01E.
    - **Whitespace:** Render Whitespace becomes `none | selection | all`, defaulting to `selection`. The menu item shows checked by default, and its toggle flips between `none` and `all`.
    - **Find widget:** a floating panel built with `createPanel`, 419px wide, 33px rows, 28px from the right, with Cursor's labels.
    - **Wording:** Go to Line uses `Go to line {0}.`. The editor context menu has Cut, Copy, Paste, and Command Palette....
  - **Quick input** (`crates/workshop/ui/src/parts/quickinput/`):
    - **Frame:** width `min(62vw, 820px)`, max 80vw, 8px radius, no border, the xl shadow, no padding or gap, 35px from the top.
    - **Rows:**
      - 22px tall, padding 0 7px, margin 0 5px, 6px radius, no gaps
      - the list is at most `min(440px, 40vh)` tall
      - selected #F0F0F01E, hover #F0F0F011
    - **Box:** 26px, padding 0 7px, inside a 4px 6px header with no divider; placeholder #F0F0F099 at 0.5.
    - **Matches:** highlights #88C0D0 at weight 700; descriptions 0.9em at 0.6.
    - **Key chips:** one chip per key with "+" between, 11px, 3px padding, 3px radius, shown plain on the selected row.
    - **Help (`?`):**
      - the label is the prefix (or "…") and the description is the help text
      - sorted by prefix, with `?` itself left out; `?>` jumps into that mode
      - command-center help rows carry `commandCenterOrder` (10, 20, 25, 40, 50, 60, 70), `commandCenterLabel`, and a `commandId` for keybindings
    - **Wording:**
      - Cursor's exact string for every placeholder and help entry, for example `:` is "Go to Line/Column"
      - an empty result shows "No matching commands"
      - group labels "recently used" and "other commands" sit at the right of the row, with a #F0F0F01C top line
  - **Toasts** (`crates/workshop/look/toast.*`):
    - **Placement:** 25px from the bottom and 3px from the left; 340px wide with a 6px margin.
    - **Card:** #181818, no border, the base shadow, a 40px blur, 12px/16px text, no colored edge, a 0.3s slide-up.
    - **Stack:** newest on top, 3 visible.
    - **Lifetimes:** info and success 15s, warning 18s, error 20s. Hover pauses the timer and shows a close X.
    - **Severity glyph:** a 12px codicon in #3794FF, #F1B467, or #E34671.
  - **Status bar** (`crates/workshop/ui/src/parts/status/`, `crates/workshop/look/status-bar.*`):
    - Items fill the 22px height, with padding 0 5px and square corners. Pressed is #F0F0F01E. The top line is an inset shadow.
    - When idle the status text is empty, not "Ready" (`keybinding-dispatcher.ts:83`).
    - Local errors go to toasts instead of red status text.
    - The LEDs stay as they are.
  - **Workbench inputs and selects:** 26px tall, 2px radius, 1px #F0F0F013 border. Selects get padding 2px 23px 2px 8px and a 16px caret 6px from the right.
  - **Run panel** (`crates/workshop/ui/src/parts/run/`):
    - **Contract rows** become Cursor Settings rows:
      - a 4% tinted card with a 12px radius
      - row padding 12px, gap 20px, inset dividers at 4%, a 13px primary label
      - `arg.description` is a visible second line, not a tooltip
      - rows stack when the panel is under 500px wide
      - boolean args use `.switch`
    - **Empty and error states:** a 22px line height and 20px left padding. "Browse..." and "Choose Prompt" are underlined inline links.
    - **Toolbar:** a 35px strip with 8px padding.
      - The path field: 24px, 12px text, 2px radius, #F0F0F00A fill, #F0F0F013 border.
      - The "Prompt" label is for screen readers only.
    - **Drag over:** a #F0F0F011 overlay class.
    - **Footer:** padding 10px, gap 8px, a top border at 8%, right-aligned.
    - **Dialogs:** Choose Prompt and Choose Input use the form modal, with Cancel first.
  - **Gateway config app** (`crates/gateway/config-ui/ui/src/`, `crates/shared-ui/tokens.css`):
    - **Tokens:** `shared-ui` takes the same IDE token values as `look`.
    - **Left nav** replaces the top `tab-bar`:
      - a sticky column `clamp(100px, 25%, 200px)` wide, with a 48px gap to the content
      - cells padded 4px 6px with a 6px radius, 12px/16px text in secondary, 12px icons
      - selected and hover fill at 6%; dividers between groups
      - under 710px it becomes a 40px column of icons only
    - **Settings cards:**
      - the section title (12px, secondary) sits above the card
      - the card is 4% tinted with a 12px radius, no border or padding
      - each row has a 13px label at weight 400, 13px help text in secondary as a second line, and the control on the right
    - **Banners:**
      - pending changes: accent at 8% fill, accent at 20% border, 12px radius, padding 12px 14px
      - restart and danger: a callout with a 4px left bar
    - **Model lists:**
      - a search box "Add or search model": padding 5px 12px, 4px radius
      - a refresh icon that spins while loading
      - rows with a 12px gap and no hover fill
      - an empty state in the "No models available" style
    - **Secrets:**
      - a "{Provider} API Key" section holding an "API Key" password row
      - saves on blur when edited; Enter and Escape blur
      - placeholder "Enter API key"; shows "Secret saved" once a key exists
      - no Verify button
    - **Field errors:** 12px/16px #E34671 right under the input, with `role=alert`; the input gets `aria-invalid` and `aria-describedby`.
    - **Modals:** they and `confirm-modal.ts` use the form modal values, titled Cursor's way, for example "Enable {Provider} API Key".
    - **Switches:** `toggle-control.ts` takes the `.switch` values.

</implementation-contract>
<verification-contract>

## Testing Plan

Each behavior change ships with its tests in the same commit. The pure transcript model and the scroll controller get unit tests. The views keep their jsdom suites, rewritten for the new DOM. Tests that pin old Workshop behavior change in the same commit as that behavior. After the last step, the operator runs a side-by-side visual check against the reference; no step can run it, because it needs the operator's installed Cursor and eyes.

- Unit:
  - New `crates/workshop/ui/test/transcript-model.mjs`:
    - every grouping case
    - summary grammar and plurals
    - tool verbs and details for `web/search`, `web/fetch`, ask, and generic tools
    - thought labels: briefly, 4s, 300s, unknown, and title-only with `0.6s`
    - every tail-status rule, including suppression and reconnecting
  - New `crates/workshop/ui/test/feed-scroll.mjs`, with a fake scroller and a `ResizeObserver` shim: pin, release inside the input window, re-pin, forced pin on send, and reduced-motion jumps.
  - New `crates/workshop/ui/test/tool-line.mjs`, with fake timers:
    - the 600ms hover delay, and the card disabled while loading
    - search results parsed into links, with non-http(s) URLs left as plain text
    - the generic block's contents
    - only the verb shimmers while loading
  - Extend `crates/workshop/ui/test/agent-session-service.mjs`:
    - clock stamps
    - `generating` and `reconnecting` transitions
    - `ToolCallRow.tool` parsing, including malformed input
    - `cancelTurn`
  - Extend `crates/workshop/ui/test/markdown-render.mjs`: fade spans only on new text, `pre` skipped, no caret.
  - Look tests in `crates/workshop/look/test/`:
    - `setShimmer` phase
    - each inline codicon string against its `@vscode/codicons` file, and the boundary walk with no `lucide` allowance
    - controls and modal skins: the `primary` flag and the button order
- Integration and end-to-end:
  - Rewrite `crates/workshop/ui/test/agent-session-view.mjs`:
    - no "You" label and no model labels
    - turn wrappers and sticky human messages
    - clip and expand on click
    - the thought row collapsed while streaming, with its open state surviving deltas and the settle
    - results folded into tool lines
    - group open state kept
    - tail text at each phase
    - the error popup instead of error rows
  - Composer action-button states in `crates/workshop/ui/test/chat-box.mjs`: mic, send, stop, disabled, and the mode fills.
  - Mode chip, model picker, and token ring tests follow the new values.
  - `crates/workshop/ui/test/run-panel.mjs` keeps passing; the shimmer class name doesn't change.
  - Menu, keybinding, quick input, toast, and status bar suites follow the new wording and behavior. That includes the idle status text and the Ctrl+L / Ctrl+Shift+L moves.
  - Desktop Rust changes (the quit item) run the workshop cargo tests.
  - The gateway config UI runs its own suite.
- Regression, security, and performance:
  - Untrusted strings stay inert: no markup from user text, tool args, or tool output, and DOMPurify still runs on markdown.
  - The hover card rejects `javascript:` and other non-http(s) URLs.
  - Reduced motion turns off the shimmer, height, fade, and smooth-scroll animations.
  - The keyed reconciler touches only changed rows. A test checks that settled history nodes keep their identity across deltas.
- Exit criteria:
  - All suites named in Project Survey pass.
  - After the last step, the operator's side-by-side check of every listed surface against the reference finds each difference fixed or recorded here as an accepted deviation.
  - Every deviation found is recorded under Decision Record, then fixed or accepted.



</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - **Copy Cursor everywhere.** Cursor's IDE settles every look, layout, wording, and behavior question. User: "The answer to every question is: copy Cursor." and "I want anything and everything possible to copy Cursor."
  - **Copy the IDE sidebar agent, not Glass.** The operator's screenshots show the sidebar, and Glass values differ: 13px inset vs 11px, a different composer, different densities.
  - **Thinking stays collapsed while it streams.** Cursor's thinking-only row behaves the same way. User: "dont show the thinking tokens unless the user unrolls".
  - **Duration labels follow Cursor's formatter:** "Thought 4s", "Thought 300s", "Thought briefly", with no "for" and no minutes. This replaces the earlier request "Thinking for 4s" / "Thinking for 5m" and the "Thought for 4s" option, because the user then said "I want to copy Cursor exactly".
  - **No speaker or model labels.** User: "the User chat should not say "User" it should just have their text. And the agent response should not keep repeating the model".
  - **Long user messages clip like Cursor's, and a click expands them.** Cursor's only way to the full text is edit mode, which the Workshop lacks. User picked "Clip like Cursor, and clicking expands it in place".
  - **Web search and fetch results show in Cursor's hover card.** User picked "Copy Cursor: a hover card listing results".
  - **Errors move to the composer popup.** Cursor never paints errors as transcript rows.
  - **Esc never stops a run; Ctrl+Shift+Backspace does,** as in Cursor.
  - **A folder-name button replaces the command-center pill,** because Cursor ships with the command center off.
  - **Ctrl+Q opens View on Windows.** Cursor binds it that way, so the Workshop's quit loses that shortcut.
  - **The idle status bar text is empty, and local errors go to toasts.**
  - **Focus outlines appear wherever Cursor draws them.** The `look` rule that forbade outlines is rewritten to defer to Cursor.
  - **Cursor's primary button replaces the gold one.**
  - **Two dialog skins, as Cursor has:** VS Code's dialog for confirmations and Cursor's modal for forms.
  - **The gateway Secrets page has no Verify button,** because Cursor 3.22.12 has none.
  - **Text tiers are fixed by aliasing.** `--text-tertiary` and `--text-quaternary` become aliases of the correct `--cursor-text-*` tiers, so one change fixes every consumer. Each step rechecks its own surface.
  - **Icons are codicons, shipped as inline SVG strings.** Codicons are the open-source set Cursor's icon font extends. `crates/workshop/look/icons.ts` holds each icon's SVG from the `@vscode/codicons` package's `src/icons/<name>.svg`, sized like today's exports and filled with `currentColor`, and a `look` test checks each string against the package file. The package is a `devDependency` that only that test reads, so `look` sources import nothing new, `test/boundary.mjs` loses its `lucide` allowance instead of gaining one, and the license notice records the icon artwork under CC-BY-4.0. Reason: the icon font needs the Workshop server's `bundle/` route to serve `.ttf`, but `bundle_content_type` in `crates/workshop/server/src/routes/assets.rs` admits only `.js` and `.css`, and this plan rules out server changes. The CSP's `default-src 'self'` refuses a data-URL font. Importing `.svg` files needs an esbuild loader in `crates/workshop/ui/build.mjs` and in each of the 84 test files that call `esbuild.build`.
  - **Render Whitespace persists as a string, the plan's one persisted-format change.** The user bucket's editor settings keep the `renderWhitespace` key, now holding `none`, `selection`, or `all`. A stored `true` reads as `all` and a stored `false` as `selection`. The settings writer stores the whole object on every change, so `false` is on record for every operator who ever flipped any editor toggle and can't mean an explicit off. The `config.editor.renderWhitespace` context key stays boolean. An older build reads the string as mistyped and falls back to its own default. This narrows the constraint against persisted-data changes for this one key.
  - **One shimmer technique, with per-tone custom properties and phase lock.** It looks the same as Cursor's masked overlay at 14px with less DOM.
  - **Reasoning time is measured in the browser.** The Engine's `Thinking` event carries no timing (`crates/promptforge-internal/types/src/event.rs`). Every agent panel starts a fresh session (`crates/workshop/ui/src/parts/agent/agent-panel.ts`), so no duration is lost to replay.
  - **A keyed reconciler replaces the prefix diff,** so grouped rows can grow in place and keep their open state.
  - **Values live in this plan.** The repository is public and the plan travels into it. For a value the plan doesn't list, the implementer reads the reference bundle and records the finding in the commit message.
  - **The reconnecting row has no Cancel.** Cursor offers one, but a cancel frame can't cross a down socket, and no queued cancel exists. The feature is deferred.
  - **Try again is a resend, not a resume.** Cursor resumes the failed request. The Workshop's agent loop returns to its ask after a failed round, so the popup re-sends the last user text through the open wait.
  - **The side-by-side check is the operator's, after the last step.** It needs the operator's Cursor and eyes, which no step can supply. Deviations it finds are fixed in follow-up changes or recorded here as accepted.
  - **Cancel settles the view locally.** The server answers a cancel with nothing, so `cancelTurn()` turns `generating` off itself.
  - **The step count stays small:** at most 10 steps. User: "keep the number of steps small".
- Rejected alternatives:
  - **Server-measured thinking time:** needs wire and Engine changes. Revisit if durations must survive a reload.
  - **Masked-overlay shimmer for tool verbs:** more DOM for no visible gain at 14px. Revisit if the visual check shows a difference.
  - **Keeping lucide icons:** they aren't Cursor's. Revisit never.
  - **The codicon font:** needs a `.ttf` route on the Workshop server and a font loader in the UI build. Revisit if the server gains a font route.
  - **Render Whitespace on the stored boolean:** a stored `false` can't tell an explicit off from the old default, so either existing operators lose the `selection` default or an explicit off can't be stored. Revisit never.
  - **Error rows in the feed:** Cursor uses the composer popup. Revisit never.
  - **Fixing each `--text-tertiary` / `--text-quaternary` consumer separately:** one alias change covers them all. Revisit if a consumer must keep the old value.
- Assumptions, risks, and notes:
  - **Placeholder:** "Plan, Build, / for skills, @ for context" is assumed to be what the operator's Cursor shows. Cursor's default branch says "Plan, search, build anything". The visual check settles it.
  - **Gap under the user bubble:** the code gives 14px, but the operator's screenshot looked closer to 8px. Implement 14px and confirm in the visual check.
  - **Text tier aliasing:** it changes the look of every consumer of the old tokens, and later steps must recheck them.
  - **Dockview:** its theme overrides and options (`hideBorders`, `createWatermarkComponent`, `dropPositionResolver`, `dndTabIndicator`, `disableTabsOverflowList`) are tied to dockview 8.3.1. An upgrade may break them.
  - **Ctrl+Q:** moving it removes a quit gesture operators may rely on.
  - **Visual check:** it needs Cursor 3.22.12 installed at the reference path. A Cursor update changes the reference.
### Deferred and Out of Scope

- Deferred (each revisits when the named functionality exists):
  - **Agent turns:** restore checkpoints, edit-and-resubmit, footer timestamps and "Worked for", thumbs feedback, fork. Needs per-turn snapshots, session rewind, wall-clock turn timing, a feedback endpoint, and session fork.
  - **Shell, edit, and approval rows:** terminal cards, edit and delete groups with diff stats, approval rows, "attempted" error lines. Needs a shell tool, file-edit tools, an approval flow, and an error flag on tool results.
  - **Mention chips in sent messages.** Needs mentions on the wire.
  - **Cancel on the reconnecting row.** Needs a cancel that survives a down socket.
  - **Extra tail strings:** "Waiting for response", "Taking longer than expected…", "Warming up…", "Wrapping up". Needs the states behind them.
  - **Composer data:** message queue and steering, attachments, real @ sources, the `/` menu, model metadata (Auto, MAX), context usage, in-box voice UI, modes that change agent behavior.
  - **Chat history:** persistence and the history flyout, pin/archive/delete/unread, generated titles, resume and request ids, export and share, completion notifications, find in chat, maximize and open as editor, the agents side bar.
  - **Workbench shell:** native caption buttons and Snap Layouts, menubar overflow, About as its own window with build metadata, a bottom panel zone, navigation history, the status bar entry model, a settings store, profiles, staged updates.
  - **Explorer and files:** seti file icons, git and problems decorations, the selection and keyboard model, file operations, drag and drop inside the tree, compact folders, preview and pinned tabs, breadcrumbs, multiple side bar views.
  - **Editor:** sticky scroll, indent guides, bracket pair colors, the overview ruler, the minimap, language intelligence, diagnostics and Problems, inline completions, Ctrl+K edits, the git gutter and blame.
  - **Overlay services:** a hover widget, a notification center and rich notifications, native message boxes, fuzzy quick pick and its extras, more quick access providers, overlay scrollbars, validation controls.
  - **Settings and gateway:** settings search, live apply, per-model enable, custom models, the use-own-key switch, a refresh endpoint, nav tooltips and badges, a file-browser quick pick, prompt execution with an output view.
  - **Original stand-ins** for Cursor-only glyphs.
- Out of scope:
  - Cursor's Glass Agents Window and editor-tab agent.
  - Account and plan items.
  - Copying Cursor's source code or proprietary glyphs.



</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the default member `crates/gateway/app` (package `gateway`, binary `promptforge-gateway`); a single crate is `cargo build --locked -p <crate>`, and the Tauri desktop app (package `workshop`, binary `promptforge-workshop`) is the explicit `cargo build --locked -p workshop`. The `gateway-config-ui` and `workshop-server` build scripts bundle their UIs through `build-ui` into `OUT_DIR`, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first on a fresh clone. The Workshop UI bundle alone is `npm run build --workspace ui` in `crates/workshop`. `cargo workshop [--release] [--target <triple>]` (alias for `build-workshop`) builds the Gateway, stages it as the Tauri sidecar, builds Workshop, and removes the staged sidecar. Aliases live in `.cargo/config.toml` (`cargo xtask`, `cargo workshop`); Windows links with `rust-lld` and the static CRT.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`; drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. UI: `node --test test/<name>.mjs` run from `crates/workshop/ui` (same form in `crates/workshop/look` and `crates/workshop/platform`). Local tools present: cargo-nextest 0.9.128, cargo-deny 0.20.2, Node 24.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, several `-p` flags for a family. The Workshop trio is `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, with the extra shapes `-p workshop-workspace --all-features` and `-p workshop-server --features headless`. UI: `npm test --workspace ui` (or `look`, `platform`) in `crates/workshop`; the Gateway config UI is `npm test` in `crates/gateway/config-ui/ui`. Structural and boundary checks: `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI runs the trio after `cargo build --locked -p gateway --no-default-features` and `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source target/debug/promptforge-gateway[.exe]`, and removes the sidecar afterward with `node tools/stage-gateway-sidecar.mjs remove --target <triple>`. UI suites: `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`. CI fails when any build leaves `git status --porcelain` non-empty.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` (PowerShell: `$env:CARGO_BUILD_WARNINGS='deny'`), plus the headless shape gate `cargo check -p gateway --no-default-features`. Never run a standalone `cargo check --workspace` beside clippy. UI typecheck: `npm run typecheck --workspaces --if-present` in `crates/workshop` and `npm run typecheck` in `crates/gateway/config-ui/ui`. Supply chain: `cargo deny check`; CI also runs `cargo audit` and `cargo hakari verify`. `.githooks/pre-push` runs the headless check, the workspace clippy, and `cargo deny`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; `.githooks/pre-commit` runs it). No formatter is configured for the TypeScript or CSS packages.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus the default-feature facade docs `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, and the private-item docs `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items` and `cargo doc --locked --no-deps -p workshop-server --document-private-items`, all under `-D warnings`. Facade surface: `cargo +<pinned nightly> xtask api --check` against `crates/promptforge/public-api.txt`, with the nightly named in `crates/build-xtask/src/api/toolchain.rs`. Docs site: `cargo xtask site [--books-only]` with `MDBOOK` pointing at an mdBook binary.
- Test placement and naming conventions:
  - Rust unit tests sit in a sibling `<stem>-tests.rs` wired as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (`crates/workshop/server/src/serve.rs`, `crates/promptforge-internal/engine/src/heading_address.rs`), in `<module>/tests.rs` plus `tests-<label>.rs` siblings (`crates/workshop/workspace/src/workspace_file/`), in a `<module>/tests/` directory once a group reaches three files (`crates/promptforge-internal/engine/src/execute/tests/`), or as a small inline `mod tests { }` (`crates/workshop/server/src/csp.rs`).
  - Integration tests are one binary per crate at `tests/it/main.rs` with one module per topic; `promptforge` and `harness` use `tests/suite/main.rs`. Helpers go in `tests/common/` or `support.rs`, data in `tests/fixtures/` and `tests/prompts/`. A few crates keep standalone `tests/<name>.rs` targets (`gateway-stt-engine`, the `native_*.rs` files in `gateway-stt-backend-whisper`, `gateway-cloud-providers`, `build-workshop`). Criterion benches live in `benches/` of `promptforge-engine` and `promptforge-lua`.
  - Test functions are full snake_case behavior sentences, such as `a_policy_refuses_an_undeclared_path_before_the_watcher_sees_it`. The STT crates run in nextest's `heavy` group (`.config/nextest.toml`).
  - Workshop UI tests are `node:test` files at `crates/workshop/ui/test/<kebab-name>.mjs` (100 files, shared setup in `test/helpers/`, such as `bootWorkbench` from `helpers/boot.mjs`); `look` and `platform` use their own `test/` directories, the Gateway config UI uses `src/**/*.test.mjs`, and `tools/*.test.mjs` sit beside their scripts.
- Directory map:
  - `crates/promptforge` (Engine facade, `public-api.txt`), `crates/promptforge-plugin` (the Plugin contract), and the private container `crates/promptforge-internal/` (`types`, `vfs`, `model-client`, `lua`, `parser`, `engine`).
  - `crates/harness` (Harness facade), `crates/harness-gateway-client` (Gateway client), and the private container `crates/harness-internal/runner`.
  - Plugin crates: `crates/plugin-web`, `crates/plugin-user-input`, `crates/plugin-mcp`.
  - `crates/gateway/` private container: `app` (package `gateway`), `cloud-providers` (also a `shared-cloud-providers` binary), `config`, `config-ui` (TypeScript `ui/`), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api`, `engine`, `backend-whisper`, `whisper-ffi`). Public pair outside it: `crates/gateway-api-types` and `crates/gateway-api-discovery`.
  - `crates/workshop/` container: `desktop` (Tauri package `workshop`), `server`, `server-api`, `agents`, `gateway`, `menu`, `status`, `run-log`, `user-state`, `workspace`, `protocol`, `registry`, `support`, plus the npm workspace `ui/`, `look/`, `platform/` (one install and lockfile at `crates/workshop`).
  - Workshop UI `crates/workshop/ui/src/`: `main.ts` (entry), `base/` (paths), `services/` (app services such as `agent-session.ts`, `agent-socket.ts`, `protocol.ts`, `ui-storage.ts`; no views), `parts/` (feature directories: `agent`, `chatbox`, `chrome`, `editor`, `gateway`, `layout`, `menu`, `quickinput`, `run`, `shared`, `status`, `stt`, `take`, `workspace`, `workspace-document`), and `tokens/component.css`. `crates/workshop/look` holds design tokens and shared controls (`tokens.css`, `semantic.css`, `sizes.css`, `progress.ts`, `shimmer.css`, `status-bar.ts`); `crates/workshop/platform` holds UI mechanics (`workshop-part.ts`, registries, `status-indicators.ts`, `lifecycle.ts`).
  - Shared: `crates/shared-error-source`, `crates/shared-loopback`, and `crates/shared-ui` (TypeScript and CSS for the Gateway config UI, not a Rust crate).
  - Build tooling: `crates/build-xtask` (structural checks as tests; `cargo xtask new-crate|tidy|api|site`), `crates/build-ceiling` (500-line Rust file cap run from every build script), `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`, and `crates/workspace-hack` (cargo-hakari).
  - `guide/` (mdBook books `workshop`, `language`, `gateway` plus site `chrome` and `landing`), `prompts/` (example prompts), `tools/` (Node scripts for Gateway sidecar staging and a TTS live check), `vibe/` (dated plan archive), `images/` (README art), `.github/workflows/` (CI), `.githooks/`, `.config/` (nextest, hakari), `.cursor/rules/` (`workshop-architecture.mdc`, `workshop-spa.mdc`). `local/` and `target-msrv/` are gitignored.
- Component boundaries (enforced by `cargo test -p build-xtask`, with the matrix in `crates/build-xtask/src/product.rs`):
  - Engine, inside out: `promptforge-types` and `promptforge-vfs` are leaves; `promptforge-model-client` uses types; `promptforge-lua` uses model-client, types, vfs; `promptforge-parser` uses lua, types; `promptforge-engine` uses all five; `promptforge` re-exports them. `promptforge-plugin` uses only `promptforge-types` and `promptforge-vfs`. No Engine crate names the gateway, Workshop, or the Harness.
  - Plugins depend only on `promptforge-plugin` (plus outside libraries and `workspace-hack`).
  - Harness: `harness-runner` uses `promptforge` and `promptforge-plugin`; `harness` adds `harness-runner`; `harness-gateway-client` uses `harness`, `plugin-web`, `promptforge`.
  - Gateway: `gateway-api-types` is the leaf wire vocabulary; `config` and `progress` sit on it, `protocol` on config, `routing` on config and protocol, `local` and `web-search` above those, the STT stack `whisper-ffi` and `stt-engine` under `backend-whisper` under `stt/api`, and `gateway` (app) on top. Gateway crates name no Engine, Harness, or Workshop crate.
  - Workshop tiers flow one way, server to features to services to vocabulary: `workshop-protocol` (wire types) and `workshop-support` are leaves, `workshop-registry` uses protocol, the feature crates (`menu`, `status`, `gateway`, `user-state`, `workspace`) use protocol, registry, support, and `workshop-server` composes everything plus the Harness and Plugins. Subsystems meet only through `workshop-registry` slots; the composition root is `crates/workshop/server/src/app/compose.rs`. Workshop reaches the gateway only over HTTP through `gateway-api-types` and `gateway-api-discovery`. The desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`.
  - The Host installs Plugins in `crates/workshop/server/src/agents.rs`: `plugin_web::PACKAGE`, `plugin_user_input::PACKAGE`, and one `plugin_mcp::PACKAGE` per configured MCP server.
  - Workshop UI: `services/` own no views, `parts/` build panels on `WorkshopPart` from `@workshop/platform`, and `@workshop/platform` never imports the app. Lazy panels load by dynamic `import()` through an `index.ts` and never import the eager entry bundle (`main.ts` and its `*.contribution.ts` modules).
- Conventions summary:
  - Rust edition 2024 on stable (`rust-toolchain.toml`), resolver 3. Every dependency is declared once in root `[workspace.dependencies]` with a comment explaining any pin or feature choice; members inherit `[lints] workspace = true`, depend on `workspace-hack`, and take `build-ceiling` as a build dependency.
  - Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny outside tests, `allow` attributes banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` deny outside its owned boundary, `missing_docs` and `unreachable_pub` warn (fatal under the gate). Process-global installers are `disallowed-methods` outside binary entry points (`clippy.toml`).
  - Crate roots open with a `//!` doc; Workshop crates and many others end it with a `## Invariants` section (25 of 47 `lib.rs` files) listing allowed dependencies, and Workshop `lib.rs` files are facades of `mod` declarations and `pub use` re-exports.
  - Rust files stay at or under 500 lines (`build-ceiling`). Source directories are flat: one or two child files sit beside the parent as `foo-bar.rs` with `#[path]`, three or more become `foo/`.
  - Engine, Harness, Host, and Plugin are capitalized defined terms; Engine crates say "the caller", never the Host. `crates/workshop/ui/test/docs-claims.mjs` enforces this in docs and rules.
  - Error and status messages are written for model consumption: concise, naming required versus actual. Comments state non-obvious constraints and cite upstream issue URLs for workarounds. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly (`serde_json` with `float_roundtrip`).
  - Workshop UI copies Cursor's workspace-sidebar agent for look, layout, wording, and behavior (`crates/workshop/ui/AGENTS.md`). CSS sits beside its TypeScript, uses `.ws-` class prefixes, and takes no raw color, size, or spacing values, only `@workshop/look` and `tokens/component.css` custom properties. Files and directories are kebab-case. Every persisted value goes through `ui-storage` to the server, either account state (`/user/state`) or workspace state (`/workspace/file/state`).
  - Commit subjects are short imperative sentences with no prefix; a finished plan lands as `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

Four components, in dependency order:

1. Look foundation (Step 1). First, because every other surface reads its tokens, text tiers, icons, shimmer, controls, progress variants, and dialog skins. On its own it fixes the swapped text tiers and the palette on every surface at once. One joint piece: the token sheet, the controls, the shimmer, and the two dialog skins are one stylesheet family that the `look` and `ui` suites check together, and the tier alias changes every consumer in the same moment.
2. Agent pane (Steps 2 to 4). Second, because it is the operator's main surface and the largest drift, and it needs only Step 1. Three pieces, built sequentially:
   - Session data (Step 2): the service rules and the pure transcript model. First, because the view renders the model's rows and tail, and the composer's stop button and placeholder read `generating` and call `cancelTurn()`. The model has no consumer until Step 3; its unit tests cover it.
   - Transcript view (Step 3): rows, markdown, feed, scroll, the turn footer, the transcript context menu, and the composer's error popup. The popup lands here because the model has no error row, so the commit that drops error rows from the feed must also give errors their new home.
   - Composer and pane chrome (Step 4): the composer card and its menus, then the pane's keybindings, header, tabs, rename, empty state, and zone size. Last, because the action button's stop state and the placeholder need Step 2, and the popup it docks comes from Step 3.
3. Workbench shell (Steps 5 to 9). Third, following the plan's item order; it needs only Step 1. Five pieces, built sequentially in step order because they land on one branch and share files: `main.ts` (Steps 3, 4, and 6), `zones.ts` and `zones.css` (Steps 1, 4, and 6), `editor-panel.ts` (Steps 6 and 7), and the status text in `keybinding-dispatcher.ts` (Step 8). Each piece is one step:
   - Title bar and menus (Step 5). First in the shell, because its menu shortcut labels share chords with Step 4's agent bindings, and it holds the plan's only Rust change.
   - Layout, tabs, and the file tree (Step 6). Before the editor, because it replaces the editor's dirty-title prefix with the tab dot.
   - Editor (Step 7).
   - Quick input, toasts, the status bar, and workbench inputs (Step 8). Before the run panel, because the run panel's selects take the workbench input values.
   - Run panel (Step 9).
4. Gateway config app (Step 10). Last, because it is a separate package with its own suite that takes the foundation's values through `crates/shared-ui`, and as the last step it runs the branch-wide gate. One joint piece: the token copy, the left nav, and the page restyle share `styles/*.css` and one suite.

The steps form one chain of ten commits on one branch, so they run in order.

Before the run's first commit, the session that runs the steps prepares `c:\Users\Vinnie\cursor\promptforge`. Nothing here is committed:

- Confirm `master` is checked out, the worktree is clean, and `a1201da73` is an ancestor of `HEAD`. No branch is created. When `vibe/ACTIVE` names this plan, the run is resuming and skips this setup.
- Run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`, then `cargo build --locked -p gateway --no-default-features` and `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`, so the Workshop cargo runs in Steps 5 and 10 can build the desktop app. Their outputs are gitignored. After Step 10's gate, remove the sidecar with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`.

Rules for every step:

- Follow the surface specifications in the Technical Design; a step names the files and the rules, and the specification holds the values. For any property the specification doesn't list, read the reference bundle with byte windows (`rg -o -b ".{0,400}PATTERN.{0,400}"`), resolve each custom property to its Cursor Dark value, and record what you found in the commit message.
- Component CSS takes no raw color, size, or spacing value. New values go into `crates/workshop/look/tokens.css` or `crates/workshop/ui/src/tokens/component.css`, and into `crates/shared-ui/tokens.css` for the gateway app.
- Model, tool, and user strings stay untrusted: markdown goes through the DOMPurify path in `markdown-render.ts`, and everything else lands through `textContent`. No step adds a network request.
- Animations respect `prefers-reduced-motion` as the Technical Design lists.
- Code, tests, and commits may state facts about Cursor, never its source code or proprietary glyphs.
- Each step rechecks its own surface against the reference after Step 1's tier alias, and changes the tests that pin old Workshop behavior in the same commit.
- UI commands run from `crates/workshop` unless a step says otherwise. Run `npm run build --workspace ui` before `npm test --workspace ui`, because the boot helper loads `dist/`. A focused run is `node --test test/<name>.mjs` from `crates/workshop/ui`.
- Each step's **Tests** bullet lists the commands its verification runs as written. Its **Test cases** bullet lists the behaviors its new or changed tests cover.
- Retired files move to `c:\Users\Vinnie\cursor\cabinet\_trash\` and their removal is staged; nothing is deleted outright.
- Commit subjects are short imperative sentences with no prefix.

<step-1>

### Step 1: Put the Workshop on Cursor's IDE tokens, icons, and controls [completed]

- Component: Look foundation
- Placement: first, for the reasons in the component list.
- Construction: one joint piece built as one step. The tier alias, the controls, the shimmer, and the dialog skins change shared stylesheets that the `look` and `ui` suites check together, and the two dialog consumers stop rendering correctly the moment their per-dialog copies go.
- `crates/workshop/look/`:
  - `tokens.css`: the IDE override values from the Foundation specification (red, blue, cyan, the three shadow inks with the base and xl stacks, `--cursor-bg-active` and `--cursor-bg-focused`, `--cursor-sidebar`), the new tokens (`--cursor-text-link`, accent hover, accent at 8%, input field background, text-invert, disabled control colors, the 2px radius, the 300ms duration), `--code-font`, and `--text-tertiary` and `--text-quaternary` (lines 87-88) rewritten as aliases of `--cursor-text-tertiary` and `--cursor-text-quaternary`.
  - `shimmer.css` and a new `shimmer.ts`: `.ws-shimmer-text` reads `--shimmer-base`, `--shimmer-peak`, and `--shimmer-duration` with today's defaults; three tone modifiers for the tail status, the thinking header, and the tool verb at 1s; `setShimmer(element, on)` sets the negative `animation-delay` from one module start time and the element's duration. `package.json` exports it as `"./shimmer"`.
  - `icons.ts`: codicon SVG strings for close, chevron-right, chevron-down, mic, arrow-up, stop-circle, add, ellipsis, trash, new-folder, warning, info, error, and check, per the Decision Record's inline-SVG decision. The four existing exports (`ICON_FOLDER_PLUS`, `ICON_TRASH_2`, `ICON_MIC`, `ICON_SEND`) keep their names and sizes and switch to new-folder, trash, mic, and arrow-up. Later steps swap their own surfaces' glyphs to these exports.
  - `package.json`, `THIRD_PARTY_NOTICES.md`, `test/boundary.mjs`, and `test/icons.mjs`: `@vscode/codicons` becomes a `devDependency`, and the notice records the codicon artwork under CC-BY-4.0. With no lucide import left in `look`, `lucide` leaves `package.json`, its notice, and the boundary allowance, and `test/icons.mjs` stops checking the lucide viewBox. `crates/workshop/package-lock.json` follows (`npm install --package-lock-only --prefix crates/workshop`). `crates/workshop/ui` keeps its own `lucide` dependency until Step 4 removes its last importers.
  - `controls.css`: `.button` (with `-sm`, a primary that replaces the gold one, secondary, ghost, outline, and danger), `.input`, `.select`, and the new `.switch`. `dropdown.css`: the select menu.
  - `progress.ts` and `progress.css`: the settings variant, indeterminate when the fraction is null, and the workbench variant.
  - `modal.ts` and `modal.css`: `ModalButton` (line 30) gains `primary`; the confirmation skin with Windows button order, and the form modal skin with Cancel first and the primary last.
  - The focus outline on list rows, tree rows, menu rows, and inputs, and the view and menu scrollbars, as shared rules.
  - `AGENTS.md`: the focus bullet (line 6) defers to Cursor.
- `crates/workshop/ui/`:
  - `style.css`: remove the global `:focus { outline: none !important }` rule (lines 75-77).
  - `src/tokens/component.css`: component tokens follow the new values.
  - `src/parts/layout/panel-tab.ts`: drop the private shimmer epoch and period (lines 66-72) and drive the loading title through `setShimmer` (lines 182-195).
  - `src/parts/shared/panel-dialog.ts`: the editor's save, revert, and overwrite prompts open the confirmation skin with "Do you want to save the changes you made to {file}?" / "Your changes will be lost if you don't save them." / Save, Don't Save, Cancel.
  - `src/parts/workspace/add-folder.ts`: the Add Folder dialog opens the form modal.
  - Delete the per-dialog copies in `src/parts/layout/zones.css` (the Add Folder block from line 32) and `src/parts/editor/editor-panel.css` (the panel dialog block from line 35).
- Rules:
  - Today's shimmer defaults stay, so `run-panel.mjs` and the tab loading state pass with only the shared phase changed.
  - Under reduced motion the shimmer stops at the row's resting color.
  - Choose Prompt, Choose Input, and the gateway confirms move to the form modal in Steps 9 and 10.
- Test cases:
  - `setShimmer`: two elements started at different times share one phase, and a rebuilt element keeps it. Under reduced motion the shimmer stops at the resting color.
  - Each codicon string equals its `@vscode/codicons` `src/icons/<name>.svg` file apart from the size attributes and `currentColor`, and the boundary walk still refuses every non-relative import.
  - The modal `primary` flag, the confirmation skin's Windows button order and save wording, and the form modal's Cancel-first order.
  - The `.switch` states and the progress bar's indeterminate mode when the fraction is null.
  - The tab loading shimmer goes through `setShimmer`.
- Tests:
  - `npm test --workspace look` and `npm run typecheck --workspace look`.
  - `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
  - `rg -n "lucide" crates/workshop/look --glob "!**/test/**" --glob "!**/node_modules/**"` finds nothing. The boundary test keeps `lucide/...` as a rejected specifier, so test files are excluded.
- Commit: `Put the Workshop on Cursor's IDE tokens and controls`.

</step-1>

<step-2>

### Step 2: Time reasoning and model the agent transcript [completed]

- Component: Agent pane
- Placement: second. Step 3 renders this model's rows and tail, and Step 4's stop button and placeholder read `generating` and call `cancelTurn()`.
- Construction: one piece built as one step. The model's thought durations, loading states, tail status, and tool labels read the service's new fields, so the service rules and the model are one behavior that their two unit suites cover together. Nothing renders the model yet.
- `crates/workshop/ui/src/services/agent-session.ts`:
  - `AgentSessionService`'s constructor (line 182) takes an optional `now: () => number`, defaulting to `performance.now()`.
  - `ReasoningItem` (line 68) gains `startedAt` and `endedAt: number | null`, stamped by the reasoning timing rules; the durable `agent_thought` (line 298) copies both from the pending item it replaces.
  - `generating` and `reconnecting` booleans with change emitters, following their rules in the service rules specification.
  - `ToolCallRow` (line 39) gains `tool: string | null`, parsed from the batch entry's `tool` string; anything malformed gives `null`.
  - `cancelTurn(): boolean`, forwarding to the wire. After a successful send it turns `generating` off and sets `endedAt` on any open thinking.
  - `ErrorItem` (line 94) gains `title: string | null`: `null` for server errors, and "Connection failed" for the local failure `respond()` folds when the socket is down (line 249), whose message becomes "The connection was interrupted. Please check your network connection and try again."
  - `AgentSessionWire` (line 26) gains `cancelTurn()` and `onDisconnect`, which `AgentSocket` already has (`agent-socket.ts` lines 104-106 and 187).
- New `crates/workshop/ui/src/parts/agent/transcript/transcript-model.ts`, pure: `buildTranscript(items, generating, reconnecting)` returning `{ turns, tail }`; the row types `human`, `markdown`, `thought`, `group`, and `tool` with their stable keys; the tail status union; and the turn, grouping, thought label, title-only thinking, group summary, loading, and tail status rules.
- New `crates/workshop/ui/src/parts/agent/transcript/tool-labels.ts`: the verbs and details for `web/search`, `web/fetch`, ask tools, and other tools (`<call name> in <tool id namespace>`), the loading rule (an ask call without a result is loading even while `generating` is off), and the plural parts of the group summary.
- Rules: the service stays DOM-free; the wire, the server, and persisted state don't change.
- Test cases:
  - Extend `test/agent-session-service.mjs`: clock stamps from an injected `now`, the `generating` and `reconnecting` transitions, `ToolCallRow.tool` parsing including malformed input, `cancelTurn` turning `generating` off and closing open thinking, and the error titles.
  - New `test/transcript-model.mjs`: every grouping case, the summary grammar and plurals, tool verbs and details for `web/search`, `web/fetch`, ask, and generic tools, thought labels (briefly, 4s, 300s, unknown, and title-only with `0.6s`), and every tail-status rule including suppression and reconnecting.
- Tests: `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
- Commit: `Time reasoning and model the agent transcript`.

</step-2>

<step-3>

### Step 3: Render the agent transcript the way Cursor does [completed]

- Component: Agent pane
- Placement: third. It renders Step 2's model, and it gives errors the composer popup that Step 4's card hosts.
- Construction: one piece built as one step. The keyed reconciler, the row components, the markdown fade, the feed scroll, and the error popup replace the current feed in one move: the old prefix diff can't host grouped rows that grow in place, and dropping error rows without the popup would hide errors.
- `crates/workshop/ui/src/parts/agent/transcript/`:
  - `collapsible.ts` and `collapsible.css`: the shared header, chevron, and animated body, default closed, keeping its open state across `update(row)` calls.
  - `human-message.ts` and `human-message.css`: the human message row with the clip, click-to-expand, and sticky rules.
  - `tool-line.ts` and `tool-line.css`: tool rows and tool steps, the muted scope, verb-only shimmer through `setShimmer` while loading, and the generic tool block.
  - `web-hover-card.ts`: the search and fetch hover card. It parses `{query, results: [{title, url, description}]}`, links only `http:` and `https:` URLs as plain anchors like the markdown renderer's `link` method (`markdown-render.ts` line 166), shows no card when the result doesn't parse as that shape, and closes on a link click.
  - `feed-scroll.ts`: the pin, release, re-pin, follow, jump, and forced-pin rules, with a `ResizeObserver`.
  - `transcript.css`: the feed column, turn wrappers, row spacing, thought and group rows, the tail, and the spacer.
- `crates/workshop/ui/src/parts/agent/agent-session-view.ts`:
  - A keyed reconcile over the model's rows replaces the item-identity prefix diff. It reuses the component for a key and calls `update(row)`, creates new ones, removes ones that are gone, and places the tail last, or inside the open group that is the active tail.
  - Turn wrappers start at each user item; JS sets the trailing spacer; no "You" or model label.
  - After each finished turn (every turn but the last, and the last once `generating` is off), a 28px footer with Copy puts the turn's reply texts, as markdown source joined by a blank line, on the clipboard, swaps its icon to a check, and raises the toast "Message copied to clipboard" through `TOAST_STACK`.
  - The transcript context menu offers Copy Message (Copy when text is selected), Select All, and Search with Google.
  - The error case (line 178) leaves the feed, and the import of `tool-call-card` goes.
- `crates/workshop/ui/src/parts/agent/agent-session.css`: the panel background and the feed values.
- `crates/workshop/ui/src/parts/agent/markdown-render.ts` and `markdown-render.css`: the Markdown specification's body text, block spacing, headings, lists, inline code, code blocks with the hover copy button, links, blockquotes, tables, and `hr`; the streaming fade with no caret, wrapping only text runs younger than 150ms with a negative `animation-delay`, and skipping `pre`, `svg`, and `math`.
- `crates/workshop/ui/src/parts/chatbox/` (`chat-box.ts`, `chat-box-view.ts`, `chat-box.css`, `types.ts`): the error popup props (a title defaulting to "Connection Error", a message, and an optional Try again) and the popup docked on the card's top edge with the `warning` codicon and the right-aligned secondary "Try again" button.
- `crates/workshop/ui/src/parts/agent/agent-panel.ts`: agent errors open the popup with the `ErrorItem`'s title (or "Connection Error") and message. Try again re-sends the last user item's text through `respond()`, and is enabled only while a wait is pinned.
- `crates/workshop/ui/src/services/toast-service.ts` (new) and `crates/workshop/ui/src/main.ts` (line 137): the `TOAST_STACK` token, registered with the existing toast stack, so the agent view reaches it without importing the app.
- Retire `src/parts/agent/tool-call-card.ts`, `src/parts/agent/tool-call-card.css`, and `test/tool-call-card.mjs` under the trash rule.
- Rules:
  - Open and closed state lives in the row components and survives every delta and the settle.
  - Reduced motion drops the height and fade animations and makes scrolling jump.
  - The deferred agent-turn features (checkpoints, edit-and-resubmit, timestamps, thumbs, fork) stay out.
- Test cases:
  - Rewrite `test/agent-session-view.mjs`: no "You" label and no model labels; turn wrappers and sticky human messages; clip and expand on click; the thought row collapsed while streaming, with its open state surviving deltas and the settle; results folded into tool lines; group open state kept; tail text at each phase, including "Reconnecting..."; the turn footer's Copy text and its toast; the context menu; the error popup instead of error rows; settled history nodes keeping their identity across deltas; no height or fade animation under reduced motion; and inert user text, tool args, and tool output.
  - New `test/feed-scroll.mjs` with a fake scroller and a `ResizeObserver` shim: pin, release inside the input window, re-pin, forced pin on send, and reduced-motion jumps.
  - New `test/tool-line.mjs` with fake timers: the 600ms hover delay, the card disabled while loading, search results parsed into links with `javascript:` and other non-http(s) URLs left as plain text, the generic block's contents, and only the verb shimmering while loading.
  - Extend `test/markdown-render.mjs`: fade spans only on new text, `pre` skipped, no caret, and DOMPurify still applied.
  - `test/chat-box.mjs`: the popup's default title, its message, and Try again, which is disabled until a wait is pinned.
  - `test/tool-line.mjs` also covers a result that doesn't parse as search results (no card, a plain line).
  - Any other suite that pins the old feed DOM or error rows follows the new DOM.
- Tests:
  - `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
  - `rg -n "tool-call-card" crates/workshop/ui/src crates/workshop/ui/test` finds nothing.
- Commit: `Render the agent transcript the way Cursor does`.

</step-3>

<step-4>

### Step 4: Copy Cursor's composer and agent pane chrome [completed]

- Component: Agent pane
- Placement: fourth, for the reasons in the component list.
- Construction: one piece built as one step. The action button's stop state and the Ctrl+Shift+Backspace binding are one stop behavior; the mode, @, and model menus share one menu surface; and the Ctrl+L and Ctrl+Shift+L chords leave the Go menu stubs and land on the agent bindings in the same change. The `ui` suite covers them together.
- Composer, in `crates/workshop/ui/src/parts/chatbox/`:
  - `chat-box.ts`, `chat-box-view.ts`, and `types.ts`: the action prop gains `stop`, and the separate mic button folds into the one round action button: mic when the box is empty and the agent idle, send with text, stop while generating (calling `cancelTurn()`), and disabled at opacity 0.3. The new `mode` prop (`agent | plan | ask | debug | multitask`) picks the fill. The placeholder reads "Add a follow-up" while generating, otherwise "Plan, Build, / for skills, @ for context". Enter sends and ignores auto-repeat; Ctrl+Enter sends.
  - `chat-box.css`: the card, editor, and toolbar values, replacing the 36px minimum, 200px maximum, 6% fill, and 18px radius, with the toolbar's 300px and 260px breakpoints.
  - `typeahead-popup.ts` and `typeahead-popup.css`: the @ menu on the shared menu surface.
  - `chip-view.ts` and `mention-chip.ts`: the mention chip values, with the close X replacing the icon on hover while the editor is focused.
- `crates/workshop/ui/src/parts/agent/agent-toolbar.*` and `mode-chip.*`: the mode chip with its tints and tooltip, and its upward check-only menu with descriptions. Ctrl+. or Shift+Tab opens it, and pressing again cycles modes.
- `crates/workshop/ui/src/parts/chrome/model-picker-trigger.*`: the model button and its 230px check-only menu with "No models found"; Ctrl+/ opens it. `token-ring.*`: the 15px ring, hidden without usage data.
- Pane chrome:
  - `src/parts/agent/agent.contribution.ts`: the default title "New Agent" (line 26). `composer.cancelComposerStep` on Ctrl+Shift+Backspace while the active editor is an agent panel, with no Esc binding. Open Chat on Ctrl+L and Ctrl+I hides the pane when the chat has focus, and otherwise reveals it and focuses the input. Ctrl+Shift+L and Ctrl+Shift+I reuse an empty agent panel or open a new instance. New Chat Tab on Ctrl+T while an agent panel is active, and on Ctrl+N while one is active and the editor text isn't focused, both reusing an empty chat. Every agent chord registers at `KeybindingWeight.BuiltinExtension`, so it beats the menu stubs that share a chord. Ctrl+W closes the agent tab, and closing the last one hides the right zone. Ctrl+[ and Ctrl+] cycle chat tabs.
  - `src/parts/menu/stubs.contribution.ts`: drop `ctrlcmd+l` and `ctrlcmd+shift+l` from the two "Add Symbol to ... Chat" stubs (lines 134 and 136).
  - `src/parts/layout/zones.ts`: each zone's group gets a `data-ws-zone` attribute at open, rebuild, and restore. The right-zone rules below and Step 6's per-zone rules select on it.
  - `src/parts/layout/zones.ts`, `zones.css`, and `panel-tab.ts`: the 35px right-group header with New Agent (+), More Actions (...), and Close (x), and its "..." menu; the chat tab values, close on hover only, and middle-click close; double-click inline rename plus the tab-menu "Rename Chat" prompt ("Enter new chat name", placeholder "Chat name"), lasting for the session only.
  - `src/parts/agent/agent-menu.ts` and `agent-session.css`: the empty state drops the launcher's lead text (`.ws-agent-menu__lead`), leaving nothing above the composer at the bottom.
  - `src/main.ts` and `zones.ts`: the right zone is at least 300px wide and prefers `min(400px, W/4)`.
  - The last `lucide` importers in `ui` swap to the Step 1 codicon exports: `parts/agent/mode-chip.ts`, `parts/chatbox/chat-box.ts`, `parts/chatbox/chip-view.ts`, and `parts/chrome/model-picker-trigger.ts`. `crates/workshop/ui/package.json` drops its `lucide` dependency, and `crates/workshop/package-lock.json` follows.
- Rules:
  - Ctrl+Shift+I and Ctrl+Shift+L also appear as menu stub chords in Step 5. The agent bindings win dispatch through their weight tier (`BuiltinExtension` over the stubs' default `WorkbenchContrib`), as the Functional Specification's chat workflows require.
  - Renames aren't persisted, and no wire field changes.
- Test cases:
  - `test/chat-box.mjs`: the mic, send, stop, and disabled states, the mode fills, the placeholder switch, and the Enter auto-repeat rule.
  - `test/mode-chip.mjs`, `test/agent-toolbar.mjs`, `test/model-picker-trigger.mjs`, `test/token-ring.mjs`, `test/typeahead-popup.mjs`, and `test/mention-chip.mjs` follow the new values and keys.
  - Keybindings: Ctrl+Shift+Backspace cancels and Esc doesn't; Ctrl+L toggles the pane by chat focus; Ctrl+L and Ctrl+Shift+L dispatch to the agent commands, not the Go menu stubs; Ctrl+T and Ctrl+N reuse an empty chat, and Ctrl+N stays with the editor while its text is focused; Ctrl+[ and Ctrl+] cycle tabs.
  - `test/agent-menu.mjs`, `test/tab-menu.mjs`, and `test/workshop-zones.mjs`: the `data-ws-zone` attribute after open, rebuild, and restore, the header and its "..." menu, rename by double-click and by the tab menu, middle-click close, closing the last tab hiding the right zone, the empty state, and the right zone's sizes.
- Tests:
  - `npm run build --workspace ui`, `npm test --workspace ui`, `npm run typecheck --workspace ui`, and `npm test --workspace platform`.
  - `rg -n "lucide" crates/workshop/ui/src crates/workshop/ui/package.json` finds nothing.
- Commit: `Copy Cursor's composer and agent pane chrome`.

</step-4>

<step-5>

### Step 5: Copy Cursor's title bar and menus [completed]

- Component: Workbench shell
- Placement: fifth, first in the shell. Its menu shortcut labels share Ctrl+Shift+I and Ctrl+Shift+L with Step 4's agent bindings, and it holds the plan's only Rust change.
- Construction: one piece built as one step. The title bar's toolbars and the menubar share the window chrome, and the zoom keys need the parser's NumPad names; the `ui`, `platform`, and Workshop cargo suites cover them together.
- `crates/workshop/ui/src/parts/chrome/`:
  - `command-center.ts` and `command-center.css`: a folder-name button replaces the command-center pill and its "?"; mousedown opens quick open.
  - `window-chrome.ts` and `window-chrome.css`: the window control colors at constant physical size under zoom, the 16px app icon in a 35px box, the inactive title-bar text, and the left (Toggle Primary Side Bar) and right (Toggle Agents, settings gear) toolbars.
  - `about-dialog.ts` and `about-dialog.css`: the 380px card with the icon and the version and date copy buttons, "Copy version info" on Enter, and Escape to close. `update-view.ts` and `update-view.css`: Cursor's update-button labels.
  - `zoom.ts`: a factor of 1.2 per level from -8 to 8, reset on Ctrl+NumPad0, and Ctrl+NumPad+ and Ctrl+NumPad-.
- `crates/workshop/platform/keybinding-parser.ts`: the NumPad add and subtract key names, parsed from `KeyboardEvent.code` and displayed like the digit names (lines 73, 204, and 258).
- `crates/workshop/ui/src/parts/menu/`:
  - `menu.ts`, `menubar.ts`, and `window-menu.css`: the menu skin; the menubar opening on mousedown and rows firing on mouseup; submenus opening after 250ms and closing after 750ms; submenus and context menus flipping to stay on screen.
  - `menubar.contribution.ts`, `edit.contribution.ts`, and `stubs.contribution.ts`: the structure changes and shortcut labels from the Menus specification, including the new "Open Extension Monitor" and "Split in Group" stubs, and Ctrl+Q on Open View... (`workbench.action.quickOpenView`, line 75) on Windows.
- `crates/workshop/desktop/src/menu.rs`: the quit item keeps `CmdOrCtrl+Q` (line 30) only on macOS; elsewhere it has no accelerator, and the comment at lines 32-37 says Ctrl+Q opens View there. A small private helper returns the platform's accelerator and gets a unit test in a sibling `menu-tests.rs`.
- Rules: File > Exit keeps working through the menu and the `quit` command; only its Windows shortcut goes.
- Test cases:
  - `test/command-center.mjs`, `test/window-chrome.mjs`, `test/titlebar-style.mjs`, `test/titlebar-browser-mode.mjs`, `test/titlebar-macos.mjs`, `test/update-view.mjs`, and `test/zoom.mjs`: the folder-name button opening quick open on mousedown, the toolbars, About's Enter and Escape, and the zoom levels and NumPad keys.
  - `test/menus.mjs`, `test/menubar-submenu.mjs`, `test/window-menu.mjs`, `test/menu-spec.mjs`, and `test/menu-registries.mjs`: press-drag-release firing, the submenu delays, flipping, the structure changes, and the shortcut labels.
  - `crates/workshop/platform/test/keybindings.mjs`: NumPad add and subtract parse and display.
  - `menu-tests.rs`: the accelerator is `CmdOrCtrl+Q` on macOS and absent elsewhere.
- Tests:
  - `npm run build --workspace ui`, `npm test --workspace ui`, `npm test --workspace platform`, `npm run typecheck --workspace ui`, and `npm run typecheck --workspace platform`.
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then with `$env:CARGO_BUILD_WARNINGS='deny'`, `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, and `cargo fmt --all --check`.
- Commit: `Copy Cursor's title bar and menus`.

</step-5>

<step-6>

### Step 6: Copy Cursor's dock layout, tabs, and file tree [completed]

- Component: Workbench shell
- Placement: sixth. It follows Step 5 in the shell's order, and it precedes Step 7 because it replaces the editor's dirty-title prefix with the tab dot.
- Construction: one piece built as one step. The dockview palette, the tab renderer, the watermark, the default sizes, and the tree's header pills meet in `zones.css`, `zones.ts`, and `main.ts`, and the layout suites boot them together.
- `crates/workshop/ui/src/parts/layout/zones.css`: the dockview palette, the sash hover, the tab geometry and borders with the active tab's merging underline, the unfocused-group tab hover, the close button rules, the empty editor group (strip hidden, #141414), and the left zone header pills.
- `crates/workshop/ui/src/parts/layout/zones.ts`: the Workshop tracks the active group, so focusing the tree or the agent doesn't dim the editor tabs. The per-zone rules select on Step 4's `data-ws-zone` attribute.
- `crates/workshop/ui/src/parts/layout/panel-tab.ts`: `setTabDirty(panelId, dirty)`, modeled on `setTabLoading` (line 84), shows a dot in place of the close button until hover. Tabs without a close get 10px right padding.
- `crates/workshop/ui/src/parts/editor/editor-panel.ts`: drop the "● " title prefix (line 394) and call `setTabDirty`.
- `crates/workshop/ui/src/main.ts`: `hideBorders: false` (line 276); `createWatermarkComponent` with the watermark rows; a `dropPositionResolver` with 10% edge bands, then thirds; `dndTabIndicator: "line"`; `disableTabsOverflowList: true`; the empty editor group at startup; the tree at `min(300px, W/4)` and at least 214px in place of `setSize({ width: 280 })` (line 256).
- `crates/workshop/ui/src/parts/workspace/workshop-panel.ts` and `workshop-panel.css`: the tree skin, row geometry driven by a depth variable, indent guides, the section header with hover actions, the empty tree text with its "Open Folder" button, and the drag pill.
- Rules: the dockview options are tied to dockview 8.3.1; this step doesn't upgrade it.
- Test cases:
  - `test/workshop-zones.mjs`, `test/workshop-layout.mjs`, `test/zone-stability.mjs`, `test/lazy-panel-sizing.mjs`, and `test/layout-policy-boot.mjs`: the active group holding when the tree or the agent takes focus; the empty group at startup; the default sizes; the watermark rows and their 478px cutoff; and the drop resolver's bands.
  - `test/editor-panel.mjs` and `test/tab-menu.mjs`: no "● " prefix, the dirty dot through `setTabDirty`, and close button visibility.
  - `test/workshop-panel-menu.mjs`, `test/workshop-panel-restore.mjs`, and `test/workspace-files.mjs`: the tree geometry, the indent guides, the section header, and the empty tree text and button.
- Tests: `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
- Commit: `Copy Cursor's dock layout, tabs, and file tree`.

</step-6>

<step-7>

### Step 7: Copy Cursor's editor [completed]

- Component: Workbench shell
- Placement: seventh. It follows Step 6, which already moved the dirty state to the tab.
- Construction: one piece built as one step. The theme, the highlight style, the find widget, and the whitespace mode configure one CodeMirror surface that the editor suites check together. The agent's Shiki theme joins it because it reads the same Cursor Dark scope colors.
- `crates/workshop/ui/src/parts/editor/editor-surface.ts`:
  - `promptforgeTheme` (line 117) reads a new `--editor-*` token block: the 14px/19px text on `.cm-scroller`, the gutter, the current line hidden while a selection exists (a has-selection class through `editorAttributes`), the selection colors through the long `.cm-selectionLayer` selector, the 2px cursor bar shifted 1px left, no focus outline, the find, word, occurrence, and bracket highlights, the fold chevrons and folded placeholder, and the 14px and 12px scrollbars.
  - `promptforgeHighlight` (line 178) maps lezer tags onto `--editor-token-*` values read from the reference theme's `tokenColors`.
  - `drawSelection({ cursorBlinkRate: 1000 })`, `scrollPastEnd()`, a 4-space indent unit, Alt+click to add a cursor, Shift+Alt+drag for a column selection, and no `crosshairCursor`.
  - A floating find widget built with `createPanel`: 419px wide, 33px rows, 28px from the right, with Cursor's labels.
  - `whitespaceExtension` renders `none`, `selection` (whitespace inside the selection only), or `all`.
- `crates/workshop/ui/src/parts/editor/editor-panel.css`: the editor values as tokens.
- `crates/workshop/ui/src/parts/editor/goto-line.ts`: `Go to line {0}.`
- The editor context menu, wherever `editor-commands.ts` or `editor.contribution.ts` declares it: Cut, Copy, Paste, and Command Palette....
- Render Whitespace, per the Decision Record's persistence decision:
  - `crates/workshop/ui/src/services/editor-settings-service.ts`: `EditorSettings.renderWhitespace` becomes `"none" | "selection" | "all"`, defaulting to `"selection"`; `set` accepts the mode.
  - `crates/workshop/ui/src/parts/editor/editor-settings-service.ts`: `readSettings` accepts the three strings and reads a stored `true` as `all` and `false` as `selection`. The `config.editor.renderWhitespace` context key stays boolean and is true unless the mode is `none`. `toggle` flips between `none` and `all`.
  - `crates/workshop/ui/src/parts/editor/editor.contribution.ts` (line 152): the menu item shows checked by default.
- `crates/workshop/ui/src/parts/agent/markdown-render.ts` and `markdown-render.css`: the Shiki theme becomes Cursor's CSS-variables theme, with `--syntax-fg` #F0F0F0, `--syntax-comment` #F0F0F099 italic, and every slot filled.
- Rules: Cursor uses stock Monaco defaults with the minimap off; the deferred editor features (sticky scroll, indent guides, the minimap, and the rest) stay out.
- Test cases:
  - `test/editor-panel.mjs` and `test/editor-idioms.mjs`: the has-selection class, the cursor blink rate, the indent unit, the multi-cursor and column selection gestures, the find widget's labels and geometry, and the Go to Line wording.
  - `test/editor-settings.mjs` and `test/boot-ui-storage.mjs` (its fixtures at lines 92 and 97 hold the boolean form): the three modes, the `selection` default, legacy `true` and `false` reads, the boolean context key, and the toggle between `none` and `all`.
  - `test/editor-commands.mjs`: the context menu's rows.
  - `test/markdown-render.mjs`: every Shiki CSS variable slot set.
- Tests: `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
- Commit: `Copy Cursor's editor`.

</step-7>

<step-8>

### Step 8: Copy Cursor's quick input, toasts, and status bar [completed]

- Component: Workbench shell
- Placement: eighth. It precedes Step 9 because the run panel's selects take the workbench input values set here.
- Construction: one piece built as one step. Command failures and local errors move from the status bar to toasts, and the idle status text goes, so the toast stack and the status bar change together; quick input shares the overlay values and the keybinding labels.
- `crates/workshop/ui/src/parts/quickinput/` (`quick-input.ts`, `quick-input.css`, `quick-access-providers.ts`, `commands-history.ts`, `quickinput.contribution.ts`): the frame, rows, box, match highlights, and key chips; the `?` help rows (prefix label, help-text description, sorted by prefix without `?` itself, `?>` jumping into that mode, and the command-center help rows with `commandCenterOrder`, `commandCenterLabel`, and `commandId`); Cursor's placeholders and help strings; "No matching commands"; and the "recently used" and "other commands" group labels.
- `crates/workshop/look/toast.ts` and `toast.css` (today a fixed 4s lifetime at `TOAST_LIFETIME_MS`): the placement, the card, a newest-on-top stack with 3 visible, lifetimes by severity, hover pausing the timer and showing a close X, and the severity codicons.
- `crates/workshop/platform/command-registry.ts` and the workbench: a command that throws or rejects raises the toast "Command '{0}' resulted in an error", with `{0}` filled as Cursor fills it (check the reference bundle). The platform reports the failure and `crates/workshop/ui` raises the toast, because `@workshop/platform` never imports the app.
- Status bar (`crates/workshop/look/status-bar.css` and `crates/workshop/ui/src/parts/status/`): items fill the 22px height with padding 0 5px and square corners, pressed is #F0F0F01E, and the top line is an inset shadow. `showLocal(label, "error")` (`crates/workshop/ui/src/parts/status/status-bar.ts` line 146, declared in `crates/workshop/platform/status-bar.ts` line 12) raises an error toast through Step 3's `TOAST_STACK` token instead of red status text, so its callers don't change. The LEDs stay as they are.
- `crates/workshop/ui/src/parts/layout/keybinding-dispatcher.ts`: the idle "Ready" (line 83) goes, leaving the idle status text empty.
- `crates/workshop/ui/src/tokens/component.css`: the workbench input and select values (26px tall, 2px radius, 1px #F0F0F013 border; selects padded 2px 23px 2px 8px with a 16px caret 6px from the right).
- Test cases:
  - `test/quick-input.mjs`: the help rows' labels, order, and `?>` jump, the wording, the empty result, the group labels, and the key chips.
  - `crates/workshop/look/test/shared-toast.mjs`: newest on top, 3 visible, the lifetimes, the hover pause, and the close X.
  - `crates/workshop/look/test/shared-status-bar.mjs`, `test/status-indicators.mjs`, `test/status-frames.mjs`, and `test/keybinding-dispatcher.mjs`: the empty idle text, local errors as toasts, and the item geometry.
  - A failing command: the platform suite checks the report, and a `ui` suite checks the toast text.
- Tests: `npm test --workspace look`, `npm test --workspace platform`, `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspaces --if-present`.
- Commit: `Copy Cursor's quick input, toasts, and status bar`.

</step-8>

<step-9>

### Step 9: Lay out the run panel as Cursor Settings rows

- Component: Workbench shell
- Placement: ninth, last in the shell, because its selects use Step 8's workbench input values.
- Construction: one piece built as one step. The rows, states, toolbar, footer, and dialogs belong to one panel and one suite.
- `crates/workshop/ui/src/parts/run/run-rows.ts` (`argControl`) and `run-panel.css`: contract rows become a 4% tinted card with a 12px radius, row padding 12px, gap 20px, inset dividers at 4%, and 13px primary labels; `arg.description` is a visible second line instead of a tooltip; rows stack under 500px; boolean args use `.switch`.
- `crates/workshop/ui/src/parts/run/run-panel.ts` and `run-panel.css`: the empty and error states (22px line height, 20px left padding, "Browse..." and "Choose Prompt" as underlined inline links); the 35px toolbar with the path field values and a screen-reader-only "Prompt" label; a #F0F0F011 drag-over overlay class; the right-aligned footer. Choose Prompt and Choose Input open the form modal with Cancel first.
- Rules: the shimmer class name doesn't change.
- Test cases: `test/run-panel.mjs` and `test/run-api.mjs`: the description line, the stacked layout under 500px, the switch for boolean args, the inline links, the drag-over class, and the Cancel-first dialogs.
- Tests: `npm run build --workspace ui`, `npm test --workspace ui`, and `npm run typecheck --workspace ui`.
- Commit: `Lay out the run panel as Cursor Settings rows`.

</step-9>

<step-10>

### Step 10: Move the gateway config app onto Cursor's settings look

- Component: Gateway config app
- Placement: last, for the reasons in the component list.
- Construction: one joint piece built as one step, for the reasons in the component list.
- `crates/shared-ui/`: `tokens.css` takes the same IDE token values as `look`, including the `--text-tertiary` and `--text-quaternary` aliases (lines 88-89). `controls.css` takes the `.switch` and button values, and `modal.css` the form modal values, because the gateway app's modals and controls come from `shared-ui`.
- `crates/gateway/config-ui/ui/src/`:
  - `components/tab-bar.ts` becomes the left nav: a sticky `clamp(100px, 25%, 200px)` column with a 48px gap to the content, the cell values, 12px icons from the app's existing `lucide` set, a 6% selected and hover fill, dividers between groups, and a 40px icon-only column under 710px.
  - `pages/*.ts`: settings cards with the section title above; rows with a 13px label, help text as a second line, and the control on the right; the pending-changes banner and the restart and danger callouts; model lists with the "Add or search model" box, a refresh icon that spins while loading, 12px row gaps without a hover fill, and a "No models available" empty state; the Secrets page's "{Provider} API Key" section with an "API Key" password row that saves on blur when edited, blurs on Enter and Escape, shows "Enter API key" and then "Secret saved", and has no Verify button; field errors at 12px/16px #E34671 under the input with `role=alert`, `aria-invalid`, and `aria-describedby`; modals titled Cursor's way, such as "Enable {Provider} API Key".
  - `components/confirm-modal.ts`: the form modal values with Cancel first.
  - `components/toggle-control.ts`: the `.switch` values.
  - `styles/base.css`, `styles/controls.css`, and `styles/layout.css`: the values as tokens.
- Rules: no gateway API, config format, or server change.
- Test cases:
  - `components/tab-bar.test.mjs`: nav selection, the dividers, and the icon-only column under 710px.
  - `pages/secrets-page.test.mjs` and `components/key-prompt.test.mjs`: save on blur only when edited, Enter and Escape blur, the placeholder and saved text, and no Verify button.
  - `pages/settings-page.test.mjs`, `pages/settings-sections.test.mjs`, `pages/models-page.test.mjs`, and `pages/cloud-models-page.test.mjs`: the card rows, the banners, the model list search and empty state, and field errors with `role=alert`, `aria-invalid`, and `aria-describedby`.
  - `styles/tokens.test.mjs`: the IDE token values and the tier aliases.
- Rules: this step adds no side-by-side check. The operator runs it after the last step, as the Testing Plan says.
- Tests:
  - In `crates/gateway/config-ui/ui`: `npm test` and `npm run typecheck`.
  - Branch-wide, because this is the last step:
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
    - With `$env:CARGO_BUILD_WARNINGS='deny'`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, then `cargo check -p gateway --no-default-features`.
    - `cargo fmt --all --check`, `cargo test -p build-xtask`, and `cargo deny check`.
    - In `crates/workshop`: `npm run build --workspace ui`, then `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present`.
    - `git status --porcelain` shows only this step's changes after the builds.
- Commit: `Move the gateway config app onto Cursor's settings look`.

</step-10>

</execution-plan>
