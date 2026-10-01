# Plan for harness

<plan-reader>

A Rust developer building a program that runs agents through the Harness for a person at a screen: a chat server, a desktop app, or a command-line tool. They know async Rust, tokio channels, `Arc`, and `Result`. They have read what a PromptForge prompt is, but they have never run one from their own program.

</plan-reader>

<plan-example>

`desk`, a small Host that runs the built-in `chat` agent for one operator. Each tour adds one idea: launch an agent and read its result, stream its replies, answer its questions, stop a turn, and reattach after a disconnect. A stub model server on localhost gives canned replies, so each example runs offline.

</plan-example>

<plan-terms>

- agent: a prompt file your program can launch by name. Owner: lib.md
- session: one launched agent that keeps running until it finishes or you close it. Owner: lib.md
- gateway: the model server that answers every model call a session makes. Owner: lib.md
- operator: the person your program puts in front of a session to answer its questions. Owner: lib.md
- wait: an open question a session has asked the operator and is waiting on. Owner: lib.md
- delta: one small piece of a reply, sent while the model is still writing. Owner: lib.md
- transcript: every event a session has recorded, in order. Owner: log.md
- cancel handle: a shared flag that tells async work to stop at its next safe point. Owner: cancel.md
- store: the set of files a session reads and writes. Owner: vfs.md

</plan-terms>

<plan-links>

- PromptForge language guide: https://cppalliance.github.io/promptforge/language/

</plan-links>

<page-lib>

Purpose: Teach a Rust developer to run PromptForge agents through the Harness as long-running sessions an operator talks to.
Core idea: Your program tells the Harness where models live, launches agents by name, and relays what each session says and asks.
Need this when: always; start here.
Builds on: none
Primer sources: guide/src/language/01-what-a-prompt-is.md, guide/src/language/04-how-a-prompt-runs.md, guide/src/language/05-lua-environment.md, guide/src/language/10-models.md

### Tour: Launch an agent
- How: How do I set up the Harness, point it at a model server, and launch an agent by name?
- What if: What happens when I launch before any model server is bound?
- Why: Why must I wait for the session to close before I read its output text?
- Example: build the Harness over an agents folder, bind the stub model server and a selected model, launch `chat` with one line of input, wait for `Closed`, and assert the output text.
- Diagram: none

### Tour: Stream a reply
- How: How do I show a reply as the model writes it, then replace it with the finished text?
- What if: What happens when I subscribe after the session has already recorded some events?
- Why: Why does each streamed piece carry the same reply number as the finished event that replaces it?
- Example: subscribe to events and deltas, print each text piece as it arrives, and swap in the finished reply when its event lands.
- Diagram: none

### Tour: Answer the operator
- How: How do I show the operator a session's question and send back their answer?
- What if: What happens when I answer a question that is already answered or was never asked?
- Why: Why does a question stay open until it is answered, even when nobody is watching?
- Example: subscribe to open questions, answer the first one with canned operator text, and assert the next reply uses it.
- Diagram: the question loop, from the session asking to your program answering and the session resuming.

### Tour: Stop a turn
- How: How do I stop the current turn without ending the whole session?
- What if: What happens when a model call fails partway through a turn?
- Why: Why does a stopped turn start the agent again over what it already said, instead of ending the session?
- Example: cancel a turn while the stub model server hangs, watch the open question come back, then close the session for good.
- Diagram: none

### Tour: Reattach after a disconnect
- How: How do I pick a session back up after my client disconnects?
- What if: What happens when I look up a session that has already been closed?
- Why: Why must I subscribe to live events before I read the stored history?
- Example: keep the session's id, drop the client, look the session up by id, replay its history past the last seen event, and re-announce its open questions.
- Diagram: none

### Tour: The complete program
- How: How do the pieces from every tour fit into one Host?
- What if: What happens when my program never answers an open question?
- Why: Why does the Harness take its model server and model choice as values you push in, rather than reading them itself?
- Example: the whole `desk` Host, every line visible.
- Diagram: the Host loop, from launch to streaming, answering, and closing.

Owns:
- item: harness::CatalogBinding
- item: harness::Delta
- item: harness::DeltaKind
- item: harness::FailureKind
- item: harness::GatewayBinding
- item: harness::Harness
- item: harness::HarnessConfig
- item: harness::HostSnapshot
- item: harness::LaunchError
- item: harness::LaunchOptions
- item: harness::LaunchRequest
- item: harness::OutputError
- item: harness::Session
- item: harness::SessionEvent
- item: harness::SessionFailure
- item: harness::SessionId
- item: harness::SessionState
- item: harness::USER_INPUT_ASK_TOOL
- item: harness::WaitError
- item: harness::WaitFrame
- item: harness::display_chain

</page-lib>

<page-cancel>

Purpose: Show how to stop async work safely, at its next safe point, instead of dropping it mid-step.
Core idea: One shared flag, installed around a piece of work, reaches every part of that work that checks it.
Need this when: your program handles Ctrl-C or stops helper tasks on its own schedule.
Builds on: lib.md
Primer sources: none

### Tour: Stop work on Ctrl-C
- How: How do I let Ctrl-C stop async work without dropping it halfway through?
- What if: What happens when code waits for a cancel and no flag was installed?
- Why: Why must I hand a spawned task its flag myself?
- Example: install a fresh flag around the `desk` loop, cancel it from a Ctrl-C task, and assert the loop ends cleanly.
- Diagram: none

### Tour: Stop one helper
- How: How do I stop one helper task without stopping the rest?
- What if: What happens to the parent when I cancel a child flag?
- Why: Why does cancelling a flag a second time do nothing?
- Example: give two helper tasks their own child flags, cancel one, and assert the other and the parent keep running.
- Diagram: a parent flag with two child flags, showing which way a cancel travels.

Owns:
- item: harness::cancel::CancelHandle
- item: harness::cancel::current
- item: harness::cancel::is_cancelled
- item: harness::cancel::maybe_scope
- item: harness::cancel::scope
- item: harness::cancel::wait_cancelled

</page-cancel>

<page-log>

Purpose: Explain the saved history every session writes, and how to tell its failures apart.
Core idea: Every event a session records is saved to disk, so a history survives your client and your process.
Need this when: you replay a transcript or handle a failure to read the saved history.
Builds on: lib.md
Primer sources: none

### Tour: Read a session's history
- How: How do I read back every event a session recorded, across all its runs?
- What if: What happens when a saved record no longer reads as an event?
- Why: Why does a run number mean nothing outside the history that issued it?
- Example: list the `desk` session's run numbers, read its transcript from the start, and print each event's index.
- Diagram: none

### Tour: Tell history failures apart
- How: How do I tell a disk failure from a damaged saved record?
- What if: What happens when I ask about a run this history never started?
- Why: Why does the error keep the database library's own error type out of its public shape?
- Example: match each kind of history failure, and reach the underlying database error when one is present.
- Diagram: none

Owns:
- item: harness::log::DatabaseSource
- item: harness::log::JsonSource
- item: harness::log::LogError
- item: harness::log::RunId

</page-log>

<page-vfs>

Purpose: Show how to give a session files of your own instead of the empty store each run starts with.
Core idea: A session reads and writes through one shared file handle, and you can choose what sits behind it.
Need this when: an agent must read your files, or you want to keep what it writes.
Builds on: lib.md
Primer sources: guide/src/language/09-the-store.md

### Tour: Give a session your own files
- How: How do I hand a session a file store of my own when I launch it?
- What if: What happens when the agent never writes the output file it declares, or leaves an old one in place?
- Why: Why is the output file collected only after the run ends?
- Example: pass `desk` a store through its launch options with the declared notes file already seeded, and read the agent's output text back after the session closes.
- Diagram: none

### Tour: Guard and watch a session's files
- How: How do I guard and watch the files my program shares with a running session?
- What if: What happens when my program's access and the run's access touch the same path?
- Why: Why does the store hold claims on the paths a run reads and writes?
- Example: build the `desk` store with a policy and an operation sink, label each Host access with an `Origin`, tell the Host's operations from the run's in the sink, and handle the conflict when both touch the notes file.
- Diagram: one path claimed by the run's access and then touched by a Host access, ending in a conflict.

Owns:
- item: harness::vfs::Origin
- item: harness::vfs::VfsError
- item: harness::vfs::VfsRef

</page-vfs>
