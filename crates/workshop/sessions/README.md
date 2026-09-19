# workshop-sessions

The PromptForge Workshop's agent session subsystem. It discovers `.md` agent prompts from the configured agents directory, launches each one as a `promptforge-api-runtime` run on the unified document runtime, and carries the session around the run: the input broker behind `user_input()`, the `ui()` host-state snapshot, streaming deltas, cancellation, and the in-memory event log.

## Agents

An agent is one `.md` PromptForge prompt file. Discovery lists the `.md` file stems under `agents.path` (the default is `agents/` beside the config file), sorted, plus the built-in `chat`: a Markdown prompt embedded at compile time from `agents/chat.md`, so a fresh install always has a working chat with no agents directory at all. A directory file named `chat.md` shadows the embedded source, and an existing `chat.md` that cannot be read surfaces its error instead of silently serving the built-in. A missing or unreadable directory offers exactly the built-in.

Discovery reads the directory per request, so a newly added agent file shows up in the agent list on the next connect, without a restart. Discovery yields bare file stems only, and launch resolves names through the discovered list: a client-sent name never reaches the filesystem unless it is the stem of a real `.md` file in the configured directory. Launching parses the file with `Prompt::parse`, activates the capabilities its frontmatter declares, prepares the run context, and drives the engine's `Run` on the `test_support::drive_tokio` driver - the interim host until the harness lands - with performers closed over the gateway client, the activated tools, and the session's input broker.

## Sessions

Every session carries the Workshop's input broker behind the script-side `user_input()` - never advertised to a model - a `ui()` snapshot serving the selected model and the first granted workspace root, a model catalog built from the retained gateway catalog, and an in-memory event log of the run's transcript events (nothing persists across a restart until the harness's run log lands). Live deltas ride a dedicated ephemeral channel, each stamped with the reply id of the durable event that will supersede it.

A host-fired cancel interrupts the run, even while a host call is suspended, and a relaunch reruns the program over the retained event log - a stop reason, never an error. Closing the session ends the run for good and drops its transcript.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../../LICENSE).
