# The full loop

This chapter assembles the complete agent. The workshop's default chat is itself an agent program written in Lua, and your own `.lua` agent can take that role. Walk through that program turn by turn, because everything you have learned so far shows up in it, working together.

## The built-in chat agent

The built-in chat agent is a transparent pass-through. It advertises no tools and sets no system prompt. It relays between the operator and the selected model, and nothing else. That restraint is the design: the program adds no behavior the operator did not ask for.

## One turn

The agent is an infinite loop. Each turn does the same five things, in order.

1. Request the operator's next message by invoking the `user_input` tool through `tool_call`.
2. Read the event log with `runtime.events()`.
3. Build the model's message list from the log: map each `user_message` event to `role = 'user'` and each `agent_message` event to `role = 'assistant'`, reading the text from `event.content`.
4. Read the operator's selected model from the `ui()` snapshot's `selected_model` field.
5. Call `models.chat` under `pcall` with that model, then loop back to step 1.

## The full program

````lua
while true do
  tool_call('user_input', {})
  local events = runtime.events()
  local messages = {}
  for i = 1, #events do
    local event = events[i]
    if event.kind == 'user_message' then
      messages[#messages + 1] = { role = 'user', content = event.content }
    elseif event.kind == 'agent_message' then
      messages[#messages + 1] = { role = 'assistant', content = event.content }
    end
  end
  pcall(models.chat, messages, { model = ui().selected_model })
end
````

This is the whole chat surface. Every line is a call you already know.

## Why the log is the state

Notice what the program does not do: it never stores the conversation in a variable. Every turn rebuilds its message list from the event log instead of holding state in the program. The `user_input` call asks the operator for the next message, and that message arrives in the log as a `user_message` event, where the next rebuild picks it up. The agent's own replies sit in the log as `agent_message` events, and the same rebuild maps them to `assistant` messages.

This is what makes the agent restartable. A relaunch over retained or reloaded history resumes the conversation exactly where it stood, because the whole conversation is in the log and the program rebuilds from it on every turn. A turn-cancel or a restart loses nothing.

## Why pcall wraps the model call

The loop runs `models.chat` under `pcall` because the current chat survives transport errors, and so must this one. A failed call does not kill the agent. The session surfaces the failure to the operator, and the loop returns to `user_input` for the next turn.

## Grow from here

Start from this program and add one capability at a time. Advertise a tool with `opts.tools` and answer the requested calls. Save notes with `store.write`. Keep a counter in `var`. The loop does not change. The turns just do more.

