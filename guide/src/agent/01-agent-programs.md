# Agent programs

This chapter teaches you what an agent is, the file you write, and how the Workshop runs it. Learn it first, because an agent is an ordinary PromptForge prompt document: everything the prompt language gives a prompt - sections, Lua blocks, model rounds, tools, the store - an agent has too. What makes it an agent is only where the file lives and who is listening.

## Write the smallest working agent

````markdown
---
name: hello
description: The smallest working agent.
promptforge: 0
---

# Hello

## Speak

```lua
log('hello from my agent')
```
````

Save that file as `hello.md` in the agents directory. The file is the whole agent: frontmatter that makes it a prompt, one title, one section, one Lua block. There is no manifest, no registration step, and no second file. When the host runs it, the `log` call records the message `hello from my agent` in the run's event stream, and the prompt runs to its end.

## How the Workshop runs an agent

The Workshop discovers agents by reading the agents directory: every `.md` file there is a launchable agent, listed under its file-stem name in a sorted list. Discovery reads the directory per request, so a file you add shows up in the agent list on the next connect, with no restart. A missing or unreadable directory is a state, not an error: the list simply offers the built-in chat alone.

Launching an agent parses the file as a PromptForge prompt and runs it on the unified document runtime, the same runtime that runs every other prompt. One launch is one prompt run: sections walk in order, Lua blocks suspend on host calls and resume with their answers, and the run ends when the document ends - or when the operator cancels it.

The directory itself is a configuration value: `agents.path` in `workshop.toml`. The default is `agents/` beside the config file.

## The agent's name

The agent's name is the `.md` file stem. Save the prompt as `hello.md` and the agent's name is `hello`. Discovery yields bare stems only, so a launch request can never be coaxed into naming a path.

The name follows the run everywhere it leaves a trace: the agent list, the session panel, and the persisted event log all key on it.

## The built-in chat and the shadow

A fresh install always offers a working chat agent, even when there is no agents directory at all. The built-in `chat` is a Markdown prompt embedded in the Workshop at compile time, and discovery always lists it.

Save your own prompt as `chat.md` in the agents directory and it shadows the embedded source: the list still shows one `chat`, but launching it runs your file. That is how your own agent takes over the chat role. An existing `chat.md` that cannot be read surfaces its error instead of silently serving the embedded source.

## The session surface

An agent prompt runs with two extras an unattached prompt does not have, both installed by the session. `user_input()` suspends the run until the operator types an answer, and returns the answer text together with an availability flag. `ui()` returns a fresh snapshot of host state on every call; its `selected_model` field names the model currently selected in the interface, so an agent that re-reads it each turn follows the operator's menu choice.

Everything else is the prompt language, exactly as the Prompt Language set teaches it: `models.infer` and `models.loop` run model rounds, `tools.add` brings tools into scope, `store` reads and writes files, `var` holds per-run state, and `log` records messages in the event stream.

## The moving parts

An agent run takes two products. The harness, reached through `harness-api`, owns discovery, launch, and the session extras: the input broker behind `user_input()`, the `ui()` snapshot, and the persisting run log. `promptforge-api-runtime` is the unified runtime that parses and runs the prompt itself. The final chapter of this set walks through the built-in chat program, the one agent every install already has.
