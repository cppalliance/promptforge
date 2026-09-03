# Errors and cancellation

This chapter teaches you what happens when a call fails or the run is cancelled, and how your agent responds. Most failures in this system are answers, not ambushes: they come back where the call was made, and your program chooses what to do next.

## Catch call failures with pcall

````lua
local ok, result = pcall(models.chat, messages, { model = 'writer' })
if not ok then
  log(result)
end
````

Wrap host calls in `pcall` to catch argument-validation and dispatch failures. These failures come back as the call's answer. They do not fail the run. A failed host call raises a Lua error that carries exactly the host's message, so the value your `pcall` catches is the message the host sent.

## Errors that name things

The error messages are built to be read. A `tool_call` or an `opts.tools` entry that names an unregistered alias fails, and the error names the in-scope aliases. An `opts.model` outside the agent's catalog fails, and the error names the model. A bad chat message fails with the 1-based index of the offending entry in your own list, as in `messages[2] role "wizard" is unknown`.

A `models.chat` tool-call round fails when the model truncates it. Your program never resumes with a partial batch of tool calls. You get the failure instead.

## Runtime errors in your code

When your Lua code itself fails, the error names the location and the absolute line, prefixed as `{location}:{line}:`. In an agent run the location renders as ``agent `<name>` ``, with your agent's file stem as the name, so the message points back at your file.

## Cancellation

A host-fired cancel interrupts the run, even while a host call is suspended or Lua code is running. The run ends with the typed error "interrupted".

Every tool call is raced against the cancel signal. On cancel, the tool future is dropped, so a slow or stuck tool cannot hold the run. Your program does not see a partial tool result. The run simply ends.

