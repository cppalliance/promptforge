# `promptforge-cli`: a binary that runs one prompt file in this process and prints what it returned

## Executive summary

`promptforge run <file.md> [input]` reads a prompt from the filesystem, runs H1 once with live capability resolution against the complete tools available to this process, executes its sections in this process, and prints the run's returned value on stdout. There is no configuration file: the binary links the picker and executor rather than calling another PromptForge service. Only a run that makes no model call is self-contained that way; the moment H1 or a section performs inference, the executor builds a gateway client from the environment, so a model turn needs `PROMPTFORGE_GATEWAY_URL`, `PROMPTFORGE_GATEWAY_API_KEY`, and a reachable gateway.

The calling interface is deliberately as small as a shell can express. One subcommand, one positional path, and at most one further argument, which reaches the prompt as the whole of `args`. There are no `key=value` pairs, no flags, and no schema between the terminal and the prompt, so nothing here has to be kept in step with a prompt's declared parameters.

Two facts bound what a run can do. The store backing the run is in memory, so a prompt's filed state lives exactly as long as the process. The live tool registry is host-owned and launch-stable: local `web_fetch` is always present, while gateway-backed `web_search` is present only when both `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` are set. A prompt that needs an unavailable capability fails at that live H1 call before any section executes.

The result is on stdout, everything else is on stderr, and the exit status is success or failure with nothing in between.

## The key design choices

1. **The binary runs prompts in this process, and links the executor to do it.** It is not a terminal client of the MCP server; it opens no connection and speaks no protocol. What it buys is that a prompt author can run a file they just edited with nothing else installed or listening, which is the loop the binary exists to serve. The cost is that everything the server owns - a catalog, admission, a durable run id, progress on the wire - is absent here rather than reachable through a flag, so this is a development tool and not the production path.

2. **The binary is `promptforge` and the package is `promptforge-cli`.** The package name follows the workspace convention and the binary name is what a person types, so the two are allowed to differ. The typed name is the load-bearing one and it is deliberately the product's name unqualified: there is one command, and a second one would be a second binary rather than a longer name.

3. **`run`'s argument is a path, not a name.** The prompt is wherever the user says it is on disk. A named prompt implies a catalog, a catalog implies configuration and a resolution rule, and the binary has neither by choice. The consequence is that shell completion and `..` work on it as they do on any file argument, and that two prompts of the same name in two directories are simply two files.

4. **`run` is the only command word, and any other first word prints the usage line and fails.** There is no `list`, because there is nothing to list without a catalog, and no `validate`, because running a file parses it and reports the same errors. Only the first word is judged: a missing one or an unrecognized one lands on the usage message naming the single legal form. Past that, nothing is validated - the word after the path becomes the input whether or not it looks like a flag, and anything after that is dropped without a word.

5. **Arguments are read from `argv` directly rather than through a parser library.** The grammar is a fixed word, a path, and an optional string, which no parser is needed to express, and the usage line is one `eprintln!`. This is reversible at any time and cheaply, which is why it is decided this way now: the surface a parser would earn is a surface the binary does not have.

6. **The input is one raw string and it becomes the prompt's whole `args`.** The prompt body decides what that text means, and nothing in the binary inspects, splits, or coerces it. This is the design choice most visible to a user and the one that most sharply separates this path from a catalog-based one: a prompt's declared parameters are not consulted, no key is validated, and a prompt that wants structure parses its own. Quoting is therefore the shell's job, and an input containing spaces must be quoted like any other single argument.

7. **A file whose frontmatter declares no `promptforge:` version is refused before it is parsed.** The check reads that one key and nothing else, so it does not depend on the rest of the frontmatter being valid, and its message says that the file is not a promptforge prompt rather than reporting a parse failure. The alternative - parsing first and reporting whatever went wrong - tells someone who pointed the tool at an ordinary markdown file that their document has a syntax error, which sends them to fix the wrong thing. The executor gates the declared major version again on its own; this check answers a different question, which is whether the file is one of ours at all.

8. **Parsing is the core's parser, called rather than reimplemented.** The binary hands the file's text to `Prompt::parse` and does no lexing of its own, so the terminal cannot disagree with the executor about what a prompt file is. A second parser would be a second definition of the language.

9. **The complete live registry and picker catalog come from one owned tool list.** Prompt frontmatter and section code do not select concrete tools. At launch the CLI constructs every tool available in this process, then derives each picker descriptor from that same live instance's stable identity, description, and parameter schema. The registry and catalog therefore have identical entries and order by construction. The executor receives those prepared artifacts and resolves H1 `tools.bind` calls synchronously as live H1 reaches them.

10. **Unavailable gateway tools are omitted, not left as dead descriptors.** `web_fetch` needs no credential and is always included. `web_search` needs the gateway URL and bearer, so without both `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` neither its live instance nor its picker descriptor exists and a matching need produces the ordinary absent-capability resolution error. When the key is present without a URL the CLI fails before execution rather than inventing a gateway location. This tests availability at construction rather than advertising a tool whose first call is guaranteed to fail authentication.

11. **The environment carries the gateway's location and credential, and the command line does not.** There is no `--url`, no `--token-file`, and no `--model`, so a key never appears in `argv` where `ps` and shell history can see it. Two names are read for model turns and gateway-backed tools: `PROMPTFORGE_GATEWAY_URL`, the gateway API root, and `PROMPTFORGE_GATEWAY_API_KEY`, the bearer, both required on the first model turn through `GatewayClient::from_env`. The key alone is not enough for `web_search`; the URL must be set too. Model selection comes from models resolved by live H1, not from the environment.

12. **The run's store is the in-memory sandbox backend, created once and shared by every section.** One store per run gives the sections a place to file state for each other; making it in-memory means a run writes nothing to the user's disk that the user did not ask for. Nothing survives the process, so this path cannot be used to accumulate state across runs, and a prompt that wants a durable artifact needs a caller that gives it a durable store.

13. **Progress is discarded by default through the same observer seam used by every phase.** `main` installs `NullObserver`, and the run function creates one random execution id before parsing and threads that id and observer reference unchanged through parsing and execution. The id correlates reports within one invocation but is not a durable run registry id. A caller inside the binary can install another observer without selecting a second execution path. The command still prints one thing when the run is over, which is the value; a spinner or a live status line would be output that a pipeline has to be taught to ignore. A long run therefore looks silent, which is accepted here and is the reason a rendering client is worth building elsewhere.

14. **The result is on stdout, everything else is on stderr, and no byte is coloured.** The single success path prints the executor's returned value and nothing around it, so `$(promptforge run ...)` captures exactly that value. Every failure - unreadable file, missing version, parse error, unresolvable tool, failed run - is one `error:` line on stderr. Nothing is written to stdout on any failure path, so a pipeline never sees a partial result.

15. **The exit status is success or failure and carries no further discrimination.** Zero means the run completed; one means it did not, whichever of the failure paths above was taken. A script that needs to know which one reads the message. Numbering the conditions would make each number a contract, and the conditions here are still moving; the number that matters, which is whether the run produced a value, is already distinguished. Adding codes later is additive, while changing what an established code means is not, so the narrow answer is the reversible one. `main` returns an exit status rather than a `Result`, because a `Result` returned from `main` prints the debug form of the error, which is not text for a person. It is also async, since the executor is: tokio provides the runtime, and the binary has no reason to own a second one.

## The whole calling surface is one line a shell can hold

```bash
$ promptforge run prompts/hello.md
$ promptforge run prompts/staker.md "Bloomberg"
$ report=$(promptforge run prompts/digest.md "2026-08")
```

A missing command word gets the usage line alone; an unrecognized one gets it preceded by a line naming the word:

```
unknown command: walk
usage: promptforge run <file.md> [input]
```

## Decide by use

- A durable store, which is what a prompt filing an artifact for something later to read would need.
- A rendered progress stream, and with it a real observer, once a run is long enough that silence is indistinguishable from a hang.
- Numbered exit codes, once a caller exists that would branch on one.

*2026-08-06 03:00 - GPT-5.6 Sol*
