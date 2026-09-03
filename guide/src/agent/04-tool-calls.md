# Tool calls

This chapter teaches you how your agent invokes tools. You will advertise tools to the model, read the calls the model asks for, dispatch them yourself, and answer them in the next round. Tools are how your agent reaches past the model, so the rules around them matter as much as the calls.

## Advertise tools to the model

````lua
local result = models.chat(messages, { tools = { 'echo' } })
````

List the tool aliases the model may ask for in `opts.tools`. The default is no tools. Pass no `tools` option and the round advertises none; the driver never adds any on its own.

## Read the requested calls

````lua
local result = models.chat(messages, { tools = { 'echo' } })
if result.tool_calls then
  for i = 1, #result.tool_calls do
    local call = result.tool_calls[i]
    log(call.name)
  end
end
````

A round with advertised tools can come back with requested tool calls. The requested calls arrive unexecuted. Running them is your program's decision, never the driver's.

Read each requested call from the 1-based entries of `result.tool_calls`. Each entry carries three fields: `id`, `name`, and `arguments`. The `arguments` field is already a Lua table, so you can pass it straight on.

## Dispatch a call

````lua
local output = tool_call('echo')
````

`tool_call(alias, args)` dispatches any tool in your agent's catalog by its wire name. Every tool in the catalog is in scope under its alias. Omit `args`, or pass nil, to call a tool without arguments; the tool receives the empty argument object.

The call resumes with the tool's result. A tool that declares structured output returns its result as a Lua table. Every other tool returns plain text. A structured tool that returns invalid JSON fails the call, and the error names the alias.

## Answer a tool call

````lua
local result = models.chat(messages, { tools = { 'echo' } })
if result.tool_calls then
  local call = result.tool_calls[1]
  local output = tool_call(call.name, call.arguments)
  messages[#messages + 1] = { role = 'assistant', content = '', tool_calls = { { id = call.id } } }
  messages[#messages + 1] = { role = 'tool', tool_call_id = call.id, content = output }
  result = models.chat(messages, { tools = { 'echo' } })
end
````

The model asked for the call, so the next round must report what happened. Append two messages to your list. First replay the assistant's tool-calling round with an `assistant` message that carries the round's `tool_calls` array. Then answer the call with a `tool` role message that carries the string `tool_call_id` of the call it answers, with the tool's output as its content.

## Count the dispatches

````lua
tool_call('echo')
tool_call('echo')
if tools.calls['echo'] == 2 then
  log('echo ran twice')
end
````

The read-only `tools.calls` table shows how many times each alias has been dispatched in the run. The count increments on every attempted dispatch, even when the tool goes on to fail.

## Trust and the envelope

````lua
local wrapped = untrusted('a < b')
````

Tool output reaches your program under a trust flag. Trusted output passes to the next model turn or to your script verbatim. Untrusted output arrives inside an `<untrusted_input_...>` envelope, wrapped by the host before it can reach the next model turn or your script.

Mark your own strings with `untrusted(s)`. The call wraps the string in an envelope tagged with the run's guard nonce. The envelope has a fixed preface. Every literal `<` in the string is escaped as `&lt;`, so exactly one live open tag and one live close tag remain. Every `untrusted()` call in a run shares one nonce, so identical content produces a byte-identical envelope.

## No MCP tools yet

There are no MCP server tools yet. The mcp request shape is reserved.

