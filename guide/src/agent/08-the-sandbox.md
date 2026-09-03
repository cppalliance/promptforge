# The sandbox

This chapter teaches you the limits the host enforces on your agent and what you can rely on inside them. Your code runs in a sandboxed Lua runtime, isolated from the host. The sandbox is what lets the host run author code safely, so it shapes every program you write.

## A fresh VM for every program

Each program runs in a fresh, restricted VM. Your file is compiled and run as Lua 5.5. Scripts execute isolated from the host: the only way in or out is a host call.

## What you have

Only the `string`, `table`, and `math` standard libraries are loaded, plus safe base functions. That is the whole standard library surface.

## What is removed

The code-loading and reflection globals are removed: `load`, `loadstring`, `dofile`, `loadfile`, `collectgarbage`, `require`, `getfenv`, `setfenv`, `rawget`, `rawset`, `rawequal`, `rawlen`, `print`, and `warn`. A script cannot load code dynamically, bypass the module system, or print directly.

The `io`, `os`, `package`, `coroutine`, and `debug` libraries are never loaded. Scripts have no file, OS, module-loading, or introspection access. File work goes through `store`, not `io`.

## Time and memory

Long-running and infinite loops are legal. The instruction budget is effectively unlimited, so a script is never killed for executing too many instructions. Only the run's cancel flag aborts a loop.

The VM runs under a Lua heap ceiling configured per run with `lua_memory_bytes`. The default is 64 MiB.

When your code trips a host quota, the error names the exhausted resource: "log event", "log byte", or "instruction".

## No direct yields

Your program cannot yield on its own. The `coroutine` global is stripped from author reach, and a hand-rolled yield fails the run: "scripts may not yield directly". Treat suspending host calls as ordinary synchronous calls, and let the host do the suspending.

