# Plan for harness

<plan-reader>

A Rust developer building a program that runs prompts through the Harness: a chat server, a desktop app, a command-line tool, or a batch job, often with a person at a screen. They know async Rust, traits, `Arc`, and `Result`. They have read what a PromptForge prompt is, but they have never run one from their own program.

</plan-reader>

<plan-example>

`desk`, a small Host that runs prompts for one operator, one Harness per run. Each tour adds one idea: run a prompt and read its report, answer the operator through an input broker, stop a round and cancel a run, and stream a reply from your own broker. A real `desk` hands each run's Harness a broker that reaches the Gateway. Each example builds the Harness on a hidden offline broker and a hidden timer that sleeps on tokio instead, so it compiles against the facade alone. Where a tour needs a model, its broker lists `stub-model`, and a model that answers writes `You said: ` and the last message.

</plan-example>

<plan-terms>

- run: one execution of one prompt, from its start to its end; one Harness drives one run. Owner: lib.md
- broker: the object your program hands the Harness to answer every model call a run makes and to list the models a run can bind. Owner: lib.md
- round: one model call a run makes through the broker. Owner: lib.md
- timer: the object your program hands the Harness that a run sleeps on for a timed wait. Owner: lib.md
- operator: the person your program puts in front of a run to answer its questions. Owner: lib.md
- input broker: the object your program supplies to carry a run's question to the operator and bring back the answer. Owner: lib.md
- run control: the handle your program keeps to stop a run's round or cancel the run from outside its future. Owner: lib.md
- piece: one small part of a reply, sent while the model is still writing. Owner: lib.md
- recorder: the object your program gives the Harness, which writes every record a run makes to it. Owner: record.md
- capability: a named set of tools and Lua the Harness adds to a run when the prompt declares it. Owner: capability.md
- service: an object your program provides under a named id for a capability to read. Owner: capability.md
- store: the set of files a run reads and writes. Owner: vfs.md

</plan-terms>

<plan-links>

- PromptForge language guide: https://cppalliance.github.io/promptforge/language/

</plan-links>

<page-lib>

Purpose: Teach a Rust developer to run PromptForge prompts through the Harness, one Harness per run.
Core idea: Your program builds a Harness for each run from the objects it owns, awaits the run's one future, and steers it from outside.
Need this when: always; start here.
Builds on: none
Primer sources: guide/src/language/01-what-a-prompt-is.md, guide/src/language/04-how-a-prompt-runs.md, guide/src/language/05-lua-environment.md, guide/src/language/10-models.md, guide/src/language/11-conversations.md, guide/src/language/16-limits-and-errors.md

### Tour: Run a prompt
- How: How do I build a Harness for one run, hand it a prompt's text, and read the run's report?
- What if: What happens when the broker cannot list its models, or its list lacks the selected model?
- Why: Why does each run get its own Harness instead of one Harness serving every run?
- Example: build a Harness on `desk`'s recorder, offline broker, and timer, run the `greet` prompt with one line of input text and a fresh store, and assert the completed outcome and the output text.
- Diagram: none

### Tour: Answer the operator
- How: How do I carry a run's question to the operator and send back their answer?
- What if: What happens when a prompt requires `promptforge/user-input` and my program supplies no input broker?
- Why: Why does the input broker travel with each run's services instead of being built into the Harness?
- Example: implement an input broker that answers with canned operator text, register `UserInput`, provide the broker under `INPUT_BROKER`, run the `asks` prompt, and assert its final text.
- Diagram: none

### Tour: Stop a round and cancel a run
- How: How do I stop the round in flight without ending the run, and how do I cancel the run?
- What if: What happens when I raise a stop while nothing is in flight, or cancel before the run begins?
- Why: Why does a stopped round reach the prompt as an error it can catch, instead of starting the prompt over?
- Example: take the run's control before the run, stop the round of the `patient` prompt whose broker never answers, and assert the `pcall` caught the drop; then cancel another run before it begins and assert it ends cancelled with no run id.
- Diagram: none

### Tour: Stream from your own broker
- How: How do I show a reply as the model writes it, when the Harness hands my broker only whole rounds?
- What if: What happens to a nested `models.infer` round, which the operator is not watching?
- Why: Why does each streamed piece carry the id of the round whose finished reply the run records?
- Example: write a broker that sends each piece of a section's own round to `desk`'s window under the round's id, run the `chats` prompt, and assert the pieces carry the round id of the recorded `assistant_reply` event.
- Diagram: none

### Tour: The complete program
- How: How do the pieces from every tour fit into one Host?
- What if: What happens when my program never answers an open question?
- Why: Why does the Harness reach every model, clock, and person through objects your program hands it, rather than finding any of them itself?
- Example: the whole `desk` Host, every line visible: a base registry and base services, a fresh Harness and input broker for each run, a streaming broker, and a run control the window keeps.
- Diagram: the Host loop, from building a run's Harness to streaming, answering, stopping, and reading the report.

Owns:
- item: harness::BoxFuture
- item: harness::CurrentModelError
- item: harness::Harness
- item: harness::HarnessError
- item: harness::HostSnapshot
- item: harness::InferenceBroker
- item: harness::OutputError
- item: harness::RunControl
- item: harness::RunReport
- item: harness::RunRequest
- item: harness::Timer
- item: harness::USER_INPUT_ASK_TOOL
- item: harness::display_chain

</page-lib>

<page-capability>

Purpose: Show how to install the capabilities a run's prompt declares, and provide the services those capabilities read.
Core idea: Your program registers each capability it offers and provides each service under a named id, and every run resolves its prompt's declarations against them.
Need this when: a prompt lists a capability under `capabilities:` in its frontmatter.
Builds on: lib.md
Primer sources: guide/src/language/12-tools.md, guide/src/language/13-web-fetch-and-search.md

### Tour: Install capabilities and their services
- How: How do I register the capabilities my prompts declare, and provide the services they read, for each run?
- What if: What happens when a prompt requires a capability I never registered, or one whose service I did not provide?
- Why: Why does the Harness add no capability of its own, not even the ones PromptForge ships?
- Example: declare a `u32` service key for `desk`'s word limit, implement a `Limits` capability that reads it into its prelude, register it beside `UserInput`, provide the limit and an input broker, and build a run's Harness with both.
- Diagram: none

Owns:
- item: harness::capability::Capability
- item: harness::capability::CapabilityError
- item: harness::capability::CapabilityErrorKind
- item: harness::capability::CapabilityId
- item: harness::capability::CapabilityRegistry
- item: harness::capability::Contribution
- item: harness::capability::HostServices
- item: harness::capability::INPUT_BROKER
- item: harness::capability::InputBroker
- item: harness::capability::InputError
- item: harness::capability::RegistryError
- item: harness::capability::RegistryErrorKind
- item: harness::capability::RunServices
- item: harness::capability::ServiceError
- item: harness::capability::ServiceId
- item: harness::capability::ServiceKey
- item: harness::capability::Tool
- item: harness::capability::UserInput

</page-capability>

<page-record>

Purpose: Show how to record every run a Harness drives in a store of your own, and how to read a run back.
Core idea: The Harness writes each run's records to a recorder your program supplies and keeps nothing itself, so the recorder holds the only copy.
Need this when: your program keeps a history of runs, or wants to see what one run did step by step.
Builds on: lib.md
Primer sources: none

### Tour: Record runs in a store of your own
- How: How do I write a recorder that keeps each run's records in my own store?
- What if: What happens when my recorder returns an error partway through a run?
- Why: Why does the Harness stop the run instead of skipping a record that failed?
- Example: implement the three recorder calls for `desk`, hand the recorder to a run's Harness, run a prompt that needs no model, and assert the first and last lines and an event between them.
- Diagram: none

### Tour: Keep runs in memory
- How: How do I keep runs in memory for a test and read them back?
- What if: What happens when I write to a run that has already ended?
- Why: Why does the memory recorder issue the run ids instead of the Harness?
- Example: begin and end a run by hand on a memory recorder, and assert the issued id, the outcome, and the refused second end.
- Diagram: none

### Tour: Read a run's events back
- How: How do I read the events a run reported after it has ended?
- What if: What happens when I read the records of a run my recorder never began?
- Why: Why does the Harness keep none of a run's events for me to read later?
- Example: run a prompt over a memory recorder, read its records by the report's run id, and assert the section-start event and the run's name in every event's `execution`.
- Diagram: none

Owns:
- item: harness::record::MemoryRecorder
- item: harness::record::Record
- item: harness::record::RecordKind
- item: harness::record::RecorderError
- item: harness::record::RecorderFuture
- item: harness::record::RunId
- item: harness::record::RunMeta
- item: harness::record::RunOutcome
- item: harness::record::RunRecorder

</page-record>

<page-vfs>

Purpose: Show how to give a run files of your own instead of the fresh empty store each run starts with.
Core idea: A run reads and writes through the file handle in its request, and you choose what sits behind it.
Need this when: a prompt must read your files, or you want to keep what it writes.
Builds on: lib.md
Primer sources: guide/src/language/09-the-store.md

### Tour: Give a run your own files
- How: How do I hand a run a file store of my own in its request?
- What if: What happens when the prompt never writes the output file it declares, or leaves an old one in place?
- Why: Why is the output file collected only after the run ends?
- Example: pass `desk`'s store in the request's `vfs` with the declared notes file already seeded, and read the output back from the report and from `desk`'s own clone.
- Diagram: none

### Tour: Guard and watch a run's files
- How: How do I guard and watch the files my program shares with a running prompt?
- What if: What happens when my program's access and the run's access touch the same path?
- Why: Why does the store hold claims on the paths a run reads and writes?
- Example: build the `desk` store with a policy and an operation sink, label each Host access with an `Origin`, tell the Host's operations from the run's in the sink, and handle the conflict when the input broker's edit touches the notes file the run read.
- Diagram: the notes file claimed by the run's read and then touched by the input broker's edit, a Host access, ending in a conflict.

Owns:
- item: harness::vfs::Origin
- item: harness::vfs::VfsError
- item: harness::vfs::VfsRef

</page-vfs>
