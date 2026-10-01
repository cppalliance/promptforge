# Plan for promptforge

<plan-reader>

A Rust developer writing a program that runs PromptForge prompts: a command-line tool, a server, or a test suite. They know traits, enums, `Arc`, `Result`, threads, and async. They have never seen a PromptForge prompt.

</plan-reader>

<plan-example>

`greeter`, a small program that runs one prompt file and prints its result. The prompt starts by writing a note to the store and reading it back, and each tour adds one idea: a model reply, a tool call, cancelling, and an event log. The store lives in memory and every model and tool answer is canned, so each example runs offline.

</plan-example>

<plan-terms>

- Harness: the program that steps a run, performs its outside work, and answers each effect; in production that is the `harness` crate, and in these examples your own program plays the Harness's part. Owner: lib.md
- run: one execution of one prompt, from prepare to its result. Owner: lib.md
- section: one heading of a prompt with the text and Lua under it; sections run in file order. Owner: lib.md
- effect: a piece of outside work the run asks your program to do. Owner: lib.md
- event: a record of something that happened during a run, for your log. Owner: lib.md
- prepare: the step before a run starts, which matches your models and tools to what the prompt declares. Owner: lib.md
- model role: a name the prompt gives to a model it needs, which prepare fills with a real model. Owner: lib.md
- tool slot: a name the prompt gives to a tool it needs, which prepare fills from your tool catalog. Owner: lib.md
- requirements notice: the text that lists everything your program could not provide for a prompt. Owner: lib.md
- store: the run's set of virtual files, shared by every section. Owner: vfs.md
- mode: the setting that decides what the model may change right now. Owner: vfs.md
- capability: a named pack of tools, such as web fetch and search, that your program can offer. Owner: capabilities.md
- prelude: Lua source that a capability adds to every section of a run. Owner: capabilities.md
- untrusted output: tool output that is wrapped before a model sees it, so the model reads it as data and not as instructions. Owner: tools.md
- chain: one walk over sibling sections, started by the run itself or by a call such as starting a task. Owner: ids.md
- task: a section started to run beside the section that started it. Owner: ids.md
- provenance: the task an effect or event belongs to and its place in that task's order. Owner: ids.md

</plan-terms>

<plan-links>

- PromptForge user guide: https://cppalliance.github.io/promptforge/

</plan-links>

<page-lib>

Purpose: Teach a Rust developer to run PromptForge prompts from their own program.
Core idea: A run asks for outside work, and your program does the work and answers.
Need this when: always; start here.
Builds on: none
Primer sources: guide/src/language/01-what-a-prompt-is.md, guide/src/language/04-how-a-prompt-runs.md

### Tour: Run a prompt
- How: How do I parse a prompt and drive its run to a result?
- What if: What happens when the prompt file has a mistake, such as a missing title?
- Why: Why does each step of a run hand back work instead of finishing the prompt?
- Example: parse the one-section greeter prompt, answer its two store effects, and assert the result text.
- Diagram: none

### Tour: Answer a model
- How: How do I give the prompt's model role my model and answer its model call?
- What if: What happens when the role needs a larger context than my model has?
- Why: Why must I prepare the context before I create the run?
- Example: describe one canned model, prepare the greeter, answer its chat effect with a canned reply, and assert that the reply is the result.
- Diagram: none

### Tour: Call a tool
- How: How do I offer a tool to the prompt and answer the call it makes?
- What if: What happens when the prompt needs a tool from a capability my program did not supply?
- Why: Why does the run see only a description of my tool while my program runs the tool itself?
- Example: give the greeter a tool slot its Lua calls, add a matching tool description to the environment, answer the tool call effect with canned output, and assert the result.
- Diagram: none

### Tour: Stop a run
- How: How do I stop a run from another thread?
- What if: What happens when I cancel and never answer the effects that are still out?
- Why: Why does giving up on one effect end the run as cancelled rather than failed?
- Example: cancel the greeter from a second thread while its model call is out, give up on that effect, and assert the run ends cancelled.
- Diagram: none

### Tour: The complete program
- How: How do the pieces from every tour fit into one Harness?
- What if: What happens when the Harness answers an effect with the wrong kind of answer?
- Why: Why does the Harness learn that the run is over from the run itself and never from its events?
- Example: the whole greeter Harness, every line visible, now writing every parse and step event to a log.
- Diagram: the Harness loop, from step to effects to answers and back to step.

Owns:
- item: promptforge::CapabilityConflict
- item: promptforge::Environment
- item: promptforge::MissingService
- item: promptforge::ParseError
- item: promptforge::Prompt
- item: promptforge::Requirements
- item: promptforge::Run
- item: promptforge::RunContext
- item: promptforge::RunError
- item: promptforge::RunLimits
- item: promptforge::SourceLocation
- item: promptforge::UnmetRequirement
- item: promptforge::ParseErrorKind
- item: promptforge::RequirementCheck
- item: promptforge::RunErrorKind
- item: promptforge::RunResult
- item: promptforge::Step

</page-lib>

<page-effect>

Purpose: Teach a Rust developer to answer every kind of outside work a run can ask for, and to log what their Harness did.
Core idea: Every effect gets exactly one answer of its own kind, and its record is what your log keeps.
Need this when: your Harness answers more than store effects, or keeps a log of the work it did.
Builds on: lib.md
Primer sources: none

### Tour: Answer every kind of effect
- How: How do I answer each of the five kinds of effect a run can ask for?
- What if: What happens when I answer an effect the run never issued, or answer one effect twice?
- Why: Why can I answer a step's effects in any order without changing the result?
- Example: one canned prompt that asks for a model round, a tool call, a store operation, a timer, and a task history read, with each step's answers given in reverse order.
- Diagram: each kind of effect beside the kind of answer it takes.

### Tour: Log effects and answers
- How: How do I write each effect and its answer to my log?
- What if: What happens when I try to write an effect to the log directly, live store handle and all?
- Why: Why should my log key each effect by its provenance and not by its effect id?
- Example: run a canned prompt and write each effect's record and its answer's record as one log line, keyed by provenance.
- Diagram: none

Owns:
- item: promptforge::effect::ChatAnswerRecord
- item: promptforge::effect::EffectId
- item: promptforge::effect::ToolAnswerRecord
- item: promptforge::effect::ToolCallOrigin
- item: promptforge::effect::AnswerRecord
- item: promptforge::effect::Effect
- item: promptforge::effect::EffectAnswer
- item: promptforge::effect::EffectRecord
- item: promptforge::effect::ToolCaller

</page-effect>

<page-event>

Purpose: Teach a Rust developer to log, show, and debug what happens during a run.
Core idea: Events are a report only: recording every event, or none, never changes what a run does.
Need this when: you keep a run log, show a transcript, or debug a model's traffic.
Builds on: lib.md
Primer sources: none

### Tour: Log a run's events
- How: How do I write every event from a parse and a run to a log, one JSON line each?
- What if: What happens to the parse's events when the prompt fails to parse?
- Why: Why does recording every event, or dropping them all, leave the run's result the same?
- Example: parse and run a canned prompt, write each event as one JSON line, and print the event kinds in order.
- Diagram: none

### Tour: Show a transcript
- How: How do I turn a run's reply and tool events into a transcript a person can read?
- What if: What happens when a model's reply is cut off by its token limit?
- Why: Why does a reply event say whether it came from a chat turn or a one-off model call?
- Example: a canned run with one model reply and one tool result, printed as a short transcript.
- Diagram: none

### Tour: Capture raw model traffic
- How: How do I capture each model round's raw request and response in the event stream?
- What if: What happens when the captured bodies hold a user's private text?
- Why: Why is capture off unless I turn it on?
- Example: run a canned model round with capture on, and print its request and response events.
- Diagram: none

Owns:
- item: promptforge::event::DebugMode
- item: promptforge::event::Event
- item: promptforge::event::ReplyOrigin

</page-event>

<page-ids>

Purpose: Teach a Rust developer to group a run's log by task and to follow each task from start to end.
Core idea: Every effect and event names its task and its place in that task, the same way each time the same inputs run.
Need this when: you store a log you will search later, or show a run's tasks.
Builds on: event.md
Primer sources: none

### Tour: Group a log by task
- How: How do I group a run's events and effects by the task that made them?
- What if: What happens when a task id read back from my log does not parse?
- Why: Why is a task's id the same path as the id of the chain that runs it?
- Example: run a canned prompt that starts two tasks, and print its events grouped by task in id order.
- Diagram: the run's task tree, with each task's id written as a path.

### Tour: Follow a task's life
- How: How do I tell who started a task and how it ended?
- What if: What happens to a task that is still running when the section that owns it returns?
- Why: Why does the log call that task abandoned rather than cancelled?
- Example: a canned run whose main section returns while a task it started still waits, with the task's start and end events printed.
- Diagram: none

Owns:
- item: promptforge::ids::ChainId
- item: promptforge::ids::ParseIdError
- item: promptforge::ids::Provenance
- item: promptforge::ids::TaskId
- item: promptforge::ids::AbandonReason
- item: promptforge::ids::TaskOrigin

</page-ids>

<page-model>

Purpose: Teach a Rust developer to describe their models, see which model each prompt role got, and answer model rounds.
Core idea: The prompt names the roles it needs, prepare binds each role to a model you describe, and your program answers each round with that model's reply.
Need this when: your prompts call a model.
Builds on: lib.md, effect.md
Primer sources: none

### Tour: Describe your models
- How: How do I describe the models my program serves and collect them in a catalog?
- What if: What happens when two descriptions share one model id?
- Why: Why is a model's thinking mode part of its description?
- Example: describe two canned models, build the catalog, and look one up by its id.
- Diagram: none

### Tour: See which model each role got
- How: How do I read which model each of the prompt's roles was bound to?
- What if: What happens when I prepare the run without setting a current model?
- Why: Why is every role bound to the same current model?
- Example: prepare a prompt with two roles against one current model, and read both bindings.
- Diagram: none

### Tour: Answer a model round
- How: How do I turn a model's reply into a completion and answer the chat effect with it?
- What if: What happens when the model round fails and I answer with an error?
- Why: Why does the answer take a whole completion and not just the reply text?
- Example: answer one canned chat effect with a text reply, and a second with a failed round.
- Diagram: none

Owns:
- item: promptforge::model::Completion
- item: promptforge::model::CompletionError
- item: promptforge::model::CompletionOptions
- item: promptforge::model::Message
- item: promptforge::model::ModelBinding
- item: promptforge::model::ModelBindings
- item: promptforge::model::ModelCatalog
- item: promptforge::model::ModelDescriptor
- item: promptforge::model::ModelId
- item: promptforge::model::ModelIdError
- item: promptforge::model::ModelInvocation
- item: promptforge::model::Temperature
- item: promptforge::model::ToolArguments
- item: promptforge::model::ToolCall
- item: promptforge::model::ToolSchema
- item: promptforge::model::CompletionErrorKind
- item: promptforge::model::CompletionResult
- item: promptforge::model::ModelCatalogError
- item: promptforge::model::StreamDelta
- item: promptforge::model::TemperatureError
- item: promptforge::model::ThinkingMode

</page-model>

<page-transport>

Purpose: Teach a developer who writes their own model connection to build request bodies, read streamed replies, and report failed rounds.
Core idea: Your program owns the connection; these helpers turn a chat effect into a request body and a reply stream into a completion.
Need this when: you connect to a model server yourself instead of using a ready-made client.
Builds on: model.md
Primer sources: none

### Tour: Read a streamed reply
- How: How do I read a model's streamed reply into a completion while showing its text as it arrives?
- What if: What happens when the reply grows past the size limit?
- Why: Why does the reader take a clock from me instead of reading the time itself?
- Example: build the request body for one round, feed a canned stream through a chunk source, print each piece of text, and assert the finished reply.
- Diagram: one round, from request body to chunks to text pieces to the finished completion.

### Tour: Report a failed round
- How: How do I turn a failed server response into the error a chat answer carries?
- What if: What happens when the server's error body is too large to read?
- Why: Why is an error body escaped and cut short before it is kept?
- Example: read a canned error body under a size cap, escape it, and answer a chat effect with the resulting error.
- Diagram: none

Owns:
- item: promptforge::transport::ClientTimeout
- item: promptforge::transport::ClientError
- item: promptforge::transport::ChunkSource
- item: promptforge::transport::build_request_body
- item: promptforge::transport::escape_controls
- item: promptforge::transport::read_body_capped
- item: promptforge::transport::read_completion_stream

</page-transport>

<page-tools>

Purpose: Teach a Rust developer to offer tools to a run and to answer the tool calls it makes.
Core idea: A run sees only descriptions of your tools; your program keeps the code and runs each call.
Need this when: your prompts call tools.
Builds on: lib.md, effect.md
Primer sources: none

### Tour: Offer tools to a run
- How: How do I describe my tools, collect them in a catalog, and see which slots they filled?
- What if: What happens when a tool's name for the model contains a slash?
- Why: Why does a slot stay empty, with no refusal, when its capability offers other tools but not this one?
- Example: build a catalog with one fetch tool, prepare a prompt that names it, and read the filled slot back.
- Diagram: none

### Tour: Answer a tool call
- How: How do I answer a tool call with the tool's output?
- What if: What happens when the tool fails?
- Why: Why must every tool output say whether it can be trusted?
- Example: answer one canned call with trusted output, one with untrusted output, and one with a failure.
- Diagram: none

Owns:
- item: promptforge::tools::ToolBindings
- item: promptforge::tools::ToolCatalog
- item: promptforge::tools::ToolDescriptor
- item: promptforge::tools::ToolError
- item: promptforge::tools::ToolId
- item: promptforge::tools::ToolIdError
- item: promptforge::tools::ToolOutput
- item: promptforge::tools::OutputTrust
- item: promptforge::tools::ToolCatalogError
- item: promptforge::tools::ToolCatalogErrorKind
- item: promptforge::tools::ToolErrorKind
- item: promptforge::tools::ToolIdErrorKind

</page-tools>

<page-capabilities>

Purpose: Teach a Rust developer to name capabilities, check which tools belong to each, and give a run the Lua that capabilities add.
Core idea: A capability's name is the first two parts of every tool name it offers.
Need this when: you group tools into packs, or add Lua that every section of a run loads.
Builds on: tools.md
Primer sources: none

### Tour: Name a capability
- How: How do I parse a capability name and check that a tool belongs to it?
- What if: What happens when a name has too few or too many parts?
- Why: Why does the number of parts decide whether a name is a capability or a tool?
- Example: parse `promptforge/web`, check that `promptforge/web/fetch` belongs to it, and show the error for a one-part name.
- Diagram: none

### Tour: Add a prelude to every section
- How: How do I give every section of a run the Lua that a capability adds?
- What if: What happens when a prelude fails to load, or defines a name that is already taken?
- Why: Why does every section load the preludes before the prompt's shared library?
- Example: add one prelude that defines a helper, and run a canned prompt whose section calls it.
- Diagram: none

Owns:
- item: promptforge::capabilities::CapabilityId
- item: promptforge::capabilities::CapabilityIdError
- item: promptforge::capabilities::GlobalName
- item: promptforge::capabilities::GlobalNameError
- item: promptforge::capabilities::Prelude
- item: promptforge::capabilities::CapabilityIdErrorKind
- item: promptforge::capabilities::GlobalNameErrorKind

</page-capabilities>

<page-prompt>

Purpose: Teach a Rust developer to read what a prompt declares before running it, and to pass it arguments.
Core idea: The frontmatter is the prompt's contract with your program, and all of it is readable after parsing.
Need this when: you list a prompt's options, build its arguments, or choose a model before a run.
Builds on: lib.md
Primer sources: none

### Tour: Read a prompt's contract
- How: How do I list what a parsed prompt declares: its files, arguments, capabilities, tools, and model roles?
- What if: What happens when a declaration has a misspelled key?
- Why: Why does parsing check only the shape of these declarations and leave the rest to prepare?
- Example: parse a canned prompt that declares one of each, and print every declaration.
- Diagram: none

### Tour: Pass arguments to a run
- How: How do I build the argument text a run receives from the arguments a prompt declares?
- What if: What happens when the argument text is not valid JSON?
- Why: Why does the run accept arguments that break the declared types?
- Example: read a prompt's declared arguments, build matching JSON, run the prompt, and run it again with plain text.
- Diagram: none

### Tour: Check a prompt's model roles
- How: How do I read each model role's keywords and context minimum before I choose a model?
- What if: What happens when a role asks for no thinking and my model always thinks?
- Why: Why are keywords such as fast and small never checked?
- Example: read the roles of a canned prompt, and compare each against one model description.
- Diagram: none

Owns:
- item: promptforge::prompt::ArgDecl
- item: promptforge::prompt::ArgsDecl
- item: promptforge::prompt::CapabilityDecl
- item: promptforge::prompt::FileDecl
- item: promptforge::prompt::Frontmatter
- item: promptforge::prompt::ModelRole
- item: promptforge::prompt::ModelRoles
- item: promptforge::prompt::ToolSlots
- item: promptforge::prompt::ArgType
- item: promptforge::prompt::ModelKeyword
- item: promptforge::prompt::ToolSlot

</page-prompt>

<page-vfs>

Purpose: Teach a Rust developer to give a run its files: the store every section shares, real directories beside it, and rules about what the run may change.
Core idea: A run reaches files only through one handle your program builds, and every operation is checked before any backend sees it.
Need this when: your prompts read or write files, or several runs share a folder.
Builds on: lib.md
Primer sources: none

### Tour: Give a run its files
- How: How do I build a handle with a real directory and a store for a run?
- What if: What happens when the handle declares no store?
- Why: Why does a second run's write to the same real file fail instead of replacing the first run's?
- Example: mount a temporary folder at the root and a memory store beneath it, prepare two runs over the one handle, and write the same file from each.
- Diagram: the handle's mounts: the real directory at the root and the store beneath it.

### Tour: Keep a run from changing files
- How: How do I let a run read files but change them only when the mode allows?
- What if: What happens when a write arrives while the mode forbids changes?
- Why: Why does a mode switch take effect on the very next operation, even in the middle of a run?
- Example: gate a handle with a mode policy, write under a mode that allows it, switch the mode, and show the refused write.
- Diagram: none

### Tour: Watch every file operation
- How: How do I see every file operation a run makes, and who asked for it?
- What if: What happens when my watcher is slow?
- Why: Why does the watcher see only operations that the rules already allowed?
- Example: install a watcher that prints each operation and its origin, then run a canned prompt that writes and reads a note.
- Diagram: none

Owns:
- item: promptforge::vfs::Access
- item: promptforge::vfs::AcquireContext
- item: promptforge::vfs::AllowAll
- item: promptforge::vfs::Entry
- item: promptforge::vfs::ExecId
- item: promptforge::vfs::HostBackend
- item: promptforge::vfs::MemoryBackend
- item: promptforge::vfs::ModeHandle
- item: promptforge::vfs::ModePolicy
- item: promptforge::vfs::OpEvent
- item: promptforge::vfs::Origin
- item: promptforge::vfs::Stat
- item: promptforge::vfs::VfsPath
- item: promptforge::vfs::VfsPathBuf
- item: promptforge::vfs::VfsRef
- item: promptforge::vfs::VfsRefBuilder
- item: promptforge::vfs::FileType
- item: promptforge::vfs::Mode
- item: promptforge::vfs::Op
- item: promptforge::vfs::PathReason
- item: promptforge::vfs::StoreOp
- item: promptforge::vfs::StoreOutcome
- item: promptforge::vfs::Verdict
- item: promptforge::vfs::VfsError
- item: promptforge::vfs::Policy
- item: promptforge::vfs::Vfs
- item: promptforge::vfs::VfsAccess
- item: promptforge::vfs::perform_store_op
- item: promptforge::vfs::OpSink

</page-vfs>

<page-cancel>

Purpose: Teach a Rust developer to stop runs and tasks from any thread, one at a time or all together.
Core idea: A cancel handle is a shared flag in a tree, and cancelling a parent reaches every child below it.
Need this when: you stop runs on a user's request, on a timeout, or at shutdown.
Builds on: lib.md
Primer sources: none

### Tour: Stop many runs at once
- How: How do I stop every run my program started with one call?
- What if: What happens when I start a run from a parent handle that was already cancelled?
- Why: Why does cancelling one run's handle leave its parent and the other runs alone?
- Example: give two runs child handles of one parent, cancel the parent, and assert that both runs end cancelled.
- Diagram: a parent handle with one child per run, and the one direction a cancel travels.

### Tour: Wait for a cancel
- How: How do I wait in async code until a handle is cancelled?
- What if: What happens when the handle is already cancelled before I start waiting?
- Why: Why does a run check its flag instead of waiting for a cancel the way my async code does?
- Example: wait on a handle in a small executor while another thread cancels it.
- Diagram: none

Owns:
- item: promptforge::cancel::CancelHandle
- item: promptforge::cancel::Cancelled

</page-cancel>

<page-timestamp>

Purpose: Teach a Rust developer to give each run its start time and to keep that time with the run's record.
Core idea: A run never reads a clock; your program hands it the start time.
Need this when: you start runs, or store and compare their start times.
Builds on: lib.md
Primer sources: none

### Tour: Stamp a run's start
- How: How do I give a run its start time from the system clock?
- What if: What happens when I pass seconds where the start time expects milliseconds?
- Why: Why does the run take its start time from my program instead of reading the clock?
- Example: stamp a run from the system clock, and print the start time as the prompt reads it.
- Diagram: none

Owns:
- item: promptforge::timestamp::Timestamp

</page-timestamp>

<page-metrics>

Purpose: Teach a Rust developer to read the token counts and timings of each model call.
Core idea: Each model call carries what every source measured, and any source may be missing.
Need this when: you report cost, speed, or usage.
Builds on: model.md, event.md
Primer sources: none

### Tour: Read a call's metrics
- How: How do I read the token counts and timings of one model call?
- What if: What happens when the server reports no token counts?
- Why: Why does one call carry separate timings from the server and from my own client?
- Example: read the metrics from a canned reply event, and print one summary line.
- Diagram: none

Owns:
- item: promptforge::metrics::CallMetrics
- item: promptforge::metrics::ClientTiming
- item: promptforge::metrics::LlamaTimings
- item: promptforge::metrics::ToolCallEvent
- item: promptforge::metrics::Usage
- item: promptforge::metrics::VllmMetrics

</page-metrics>

<page-replay>

Purpose: Teach a Rust developer to store a run's behavior flags with its record and hand them back unchanged.
Core idea: Flags are a small set of bits your program keeps with each run and passes back as they were.
Need this when: you keep a record of each run.
Builds on: lib.md
Primer sources: none

### Tour: Keep a run's flags
- How: How do I store a run's flags with its record and restore them later?
- What if: What happens when a stored record has flag bits this version does not know?
- Why: Why does the crate carry a flag set when it defines no flags yet?
- Example: save a flag set as its number, read it back, and test one bit.
- Diagram: none

Owns:
- item: promptforge::replay::Flags

</page-replay>
