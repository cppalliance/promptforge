# The Chat Surface

You have a model selected and chat is ready. This chapter teaches you the chat surface itself: how to send a prompt, how to read the transcript, and how to steer a session once it is running. Chat is the heart of the Workshop, and everything here builds directly on the Models and Profiles chapter.

## Your first message

The Agent Session panel on the right side of the window is where you talk to the selected model. Chat always runs as a live agent session, not a one-shot buffered request. Every reply streams through the open session, which opens instantly and stays open for the whole session.

The default chat is a transparent pass-through with no added system prompt and no tools. Your messages go to the model currently selected in the interface. A fresh install always offers this working built-in chat agent, even when there is no agents directory at all. Later you can add your own agents by dropping `.lua` files into the agents directory; each file appears as a launchable agent under its file-stem name in a sorted list, and a newly added agent file shows up in the agent list on the next connect, without a restart. Placing a `chat.lua` file in the agents directory shadows the built-in one, so you can replace the default chat with your own program. An existing `chat.lua` that cannot be read surfaces its error instead of silently serving the embedded source.

To send your first message:

1. Click into the input box at the bottom of the Agent Session panel. The placeholder reads "Plan, Build, / for skills, @ for context".
2. Type your message.
3. Press Enter.

Enter sends the prompt. Shift+Enter inserts a newline without sending. If you use a CJK input method, an Enter that commits an IME composition never sends, so you can confirm candidates safely.

Sending delivers exactly the text you typed, never trimmed. An empty box sends nothing. A failed send keeps the text for retry. A successful send clears the box. The box grows and shrinks with what you type, within a minimum and maximum height (about 36px to 200px), and scrolls past the maximum.

The prompt box and send button enable only while the agent is asking for input. Otherwise the box is read-only and send is disabled.

A push-to-talk microphone button sits beside the send button. It stays visible in every state, and when dictation cannot start, a click names the blocker on the status bar. The Voice Input chapter covers dictation.

## Reading the transcript

The session reads as a scrolling feed of rows, one row per transcript entry, with each kind of entry styled distinctly. The feed scrolls itself to the newest entry whenever it repaints. New rows are announced to assistive technology as they arrive; settled history is never rebuilt or re-announced during streaming.

Your own messages appear under a muted "You" label as plain text, right-aligned as bubbles. Text you send is never interpreted as markup, so pasted or typed HTML cannot inject formatting or scripts.

Agent replies render as formatted Markdown with a muted line above naming the model that produced the reply. Replies and reasoning that are still streaming carry a visible pending style and a blinking caret at the live tail. While a reply streams, you see the answer text arrive chunk by chunk. The status bar shows "Running agent turn" while the agent thinks, "Streaming response..." while the reply streams, and "Ready" when the turn completes. The model's reasoning streams live on its own side channel, separate from the answer text, and appears in a collapsible block titled "Reasoning" or "Reasoning (model)". It stays open while it streams and collapses once it settles.

Tool calls appear as collapsible cards with a clickable header. The header shows the tool's name (or a generic "Tool call" / "Tool calls" label), a count badge for multi-call batches, and a status dot. A card opens on its own while the call runs and closes when the result arrives. A card you opened by hand stays open. Each call's arguments render as syntax-highlighted JSON. The result appears as a preformatted block labeled with the id of the call it answers. A batch that cannot be parsed still renders as raw text instead of vanishing.

Errors appear inline in the transcript with a visible "Error: " label, never by color alone. A message that could not be sent because the connection is down appears as a local notice: "The message was not sent: the agent socket is down."

You can observe per-reply model metrics such as token usage and generation speed attached to the assistant's replies. The log records which model produced each entry, per-reply token usage (prompt, completion, cached, and reasoning tokens), and per-reply timings (time to first token, generation speed in tokens per second, and end-to-end latency).

## Mentions and the composer extras

You can mention files with @ and pick them from a typeahead popup that opens next to the cursor. The list filters its entries by case-insensitive substring match against the text typed after the @. While the popup is open, ArrowUp and ArrowDown move the highlight through the suggestion list with wraparound, and Enter inserts the highlighted item instead of sending the message. Clicking a row inserts that file without moving focus out of the editor. Escape dismisses the popup. A query with no matches hides the popup.

Each referenced file appears as an inline pill inside the prompt editor, with a file icon and the file's label. The pill behaves as a single unit, not editable text. Clicking the X button on the pill removes the whole mention. The suggestion list currently offers three canned file entries (README.md, src/main.ts, Cargo.toml) as a stand-in until the workspace file index exists.

## The agent toolbar

A toolbar above the input bar groups the mode chip, the model picker, and a context-usage ring in one row.

The mode chip lets you choose among five agent interaction modes: Agent, Plan, Debug, Multitask, and Ask. The chip starts in Agent mode. Click it and pick a mode; the chip's icon and label update immediately and the change is announced to the rest of the application. Re-picking the current mode produces no change and no event.

The context ring is a small 16px gauge showing how much of the model's context window the current session has used. The arc fills in proportion to the percentage used. The ring reads 0 percent until real usage data exists, and readings are clamped between 0 and 100. Assistive technology hears it announced as "Context usage" with the current percentage.

The model picker in the toolbar is the pill button from the Models and Profiles chapter; it shares the same selection as the title-bar Model menu.

## Sessions that survive

A session is more durable than its connection. Agent sessions survive a dropped connection. The socket reconnects on its own and reattaches to the same session. The server replays the persisted event log from the beginning, and a per-client cursor drops duplicates, so you see each event exactly once and in order. Every unanswered prompt is re-announced in the order it was asked.

You can also attach to an already running session by its session id, resuming where that session stands. Sessions outlive sockets.

Your run history is recorded as a durable event log that survives restarts. Each session's conversation persists to a JSONL transcript file named after the session id under the sessions state directory. The log format is versioned, so session logs saved on disk keep loading after every application update. A damaged, truncated, or incompatible history file is refused with a clear error instead of showing a wrong or partial history. You can return to a previous run and continue it: the saved history is restored with its original ordering, and new events append to the same record. If saving the log to disk fails, the run keeps working and nothing you see is lost; the failure is logged as a warning and saving retries on later events.

The chat shows both sides of the conversation back to the model each turn, rebuilding the message list from the recorded user and agent messages. The conversation accumulates turn over turn, and what you typed reaches the model byte-exact, with newlines, quotes, and unicode preserved. Selecting another model takes effect on the next turn, and each reply is attributed to the model that produced it. A relaunch over retained or reloaded history resumes the conversation exactly where it stood.

## Cancelling and failing gracefully

You can cancel a running turn. Cancellation is a stop reason, never an error. Pending prompts close as cancelled, and the relaunched agent returns to waiting over its retained history. The chat is immediately usable again.

The chat survives a transport failure: the session surfaces the failure and returns to waiting for the next message. When a single model round fails, you see an error message naming the agent; the agent survives the failure and returns to waiting for input. When a run fails outright, you see an "Agent failed" notification carrying the error text. If stream chunks are dropped on a slow connection, the completed transcript event repairs the text. Late chunks that arrive after a cancel are discarded, so you never see duplicate or orphaned streaming text.

Closing a session ends the agent run for good with no relaunch. The saved transcript stays on disk.

## When the agent asks you a question

Some agent programs pause and ask for input. When an agent program needs input, the Workshop presents a prompt in the session's input box and waits for you to type an answer. The input box stays pinned to that request until it is answered. Each prompt accepts exactly one answer, and your typed answer reaches the agent byte-exact as typed, preserving newlines, quotes, braces, backslashes, and non-ASCII characters.

Cancelling a turn while a prompt is pending dismisses that prompt, so the input box is never left stuck on a dead question. A prompt that dies unresolved is explicitly cancelled on screen, never silently abandoned. A pending prompt survives a lost connection: on reconnect, every unanswered prompt is shown again in the order it was asked, and a stale prompt vanishes. You can answer a prompt that was asked while the socket was down; the answer is delivered normally once the session is back.

## The agent panel

You work with one agent session per panel. Opening a new panel starts a fresh session. Closing the panel ends the session and releases its connection. The panel automatically launches the "chat" agent when the server reports available agents, falling back to the first available agent when "chat" is not present. You can open additional agent sessions from the Agents menu (New Agent) or the Workshop menu (Open Agent Session). Each new session gets its own panel in the right zone. Agent windows are modal: one window serves one session at a time, and trying to open a second session in the same window is refused with an explanation.

While the panel has no active session, you see a launchable-agent menu labeled "Agents" for assistive technology, with the lead line "Launch an agent to start a session." There is one button per discovered agent, labeled with the agent's name; clicking it launches a session. When no agents are discovered, you see the message "No agents discovered." After you launch an agent, every launch button disables until the server answers, preventing a double launch. A refused launch shows the server's error message and re-enables the buttons for another try. When the agent socket is down, you see "The agent socket is down; it reconnects by itself. Try again shortly." and no launch is sent. The whole menu disappears once the session acknowledgment arrives, replaced by the session surface. Starting or reattaching to a session clears any pending input prompt; a same-session reattach keeps the transcript, and a new session starts the transcript fresh.

## What chat content can contain

Model-authored chat content renders as Markdown: headings, bold, italic, inline code, lists, blockquotes, tables, links, and images. Fenced code blocks are syntax-highlighted in the application's dark theme in twelve languages: bash, css, html, javascript, json, lua, markdown, python, rust, toml, typescript, and yaml. A code block in an unrecognized language renders as a plain code block, and if highlighting fails to initialize, code blocks still render as plain preformatted text.

You can size an image embedded in chat content by appending a ` =WxH` or ` =Wx` dimension suffix to the image source. Links show a tooltip on hover that defaults to the link URL.

Model-authored markup is sanitized before display. Scripts, inline event handlers, and dangerous URLs such as javascript: links are stripped. Tool results render as plain text, so markup inside a result can never execute.

Launching an agent is refused when the gateway settings cannot produce a usable model client. The error tells you to check `gateway.base_url` and `gateway.api_key` in `workshop.toml`. The rest of the Workshop keeps serving.

You can now hold a full conversation, steer it, and recover from anything that interrupts it. The next chapter teaches you to speak your prompts instead of typing them.

