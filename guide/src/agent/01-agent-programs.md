# Agent programs

This chapter teaches you what an agent program is, the file you write, and how the host runs it. Learn it first, because everything an agent does - talking to a model, calling a tool, reading what happened - is a call made from this one file.

## Write the smallest working agent

````lua
log('hello from my agent')
````

Save that one line in a file named `hello.lua`. The file is the whole agent. When the host runs it, the `log` call records the message `hello from my agent` in the run's event stream, and the program runs to its end.

An agent is one `.lua` program. There is no manifest, no registration step, and no second file. The program you save is the program the host runs.

## How the host runs the program

The host compiles your file as Lua 5.5 and runs it as a single long-running Lua coroutine. One run is one coroutine, driven from the first line to the end of the program.

Your program keeps its own local state across the whole session. A local variable you set early is still there at the end, because the same coroutine runs every line.

## The agent's name

The agent's name is the `.lua` file stem. Save the program as `hello.lua` and the agent's name is `hello`. Agents have no sections, so that name is the whole identity.

Every event your agent emits carries the agent's name as its section label. The workshop UI and the event log both key on that name, so the name in the file stem is the name you see everywhere the run leaves a trace.

## The host surface

Your program reaches the host through a shared set of calls. `models.infer` runs one model completion. `tools.call` dispatches a tool. `store` reads and writes files. `var` holds per-run state. `log` records a message in the event stream. Cooperative cancellation lets the host stop the run.

Three calls do not exist in an agent: `call`, `fanout`, and `jump`. They are absent, not stubbed. An agent that calls one fails on an undefined global.

## The moving parts

Two Rust crates carry the agent surface. `promptforge-agent` is the agent executor that runs your program. `promptforge-lua` is the Lua host runtime your program calls into. The workshop's built-in chat is no longer an agent program: it is an embedded Markdown prompt on the unified document runtime. Save your own program as `chat.lua` in the agents directory and it shadows the built-in, so your agent can take over the chat role.

