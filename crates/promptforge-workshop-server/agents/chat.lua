-- The built-in chat agent: the workshop's default chat is this program.
-- Frozen minimal on purpose until the direct relay is excised: no tools
-- advertised, no system prompt, a transparent pass-through between the
-- operator and the selected model.
--
-- Every turn rebuilds its message list from the event log, so a relaunch
-- over retained or reloaded history (turn-cancel, restart) resumes the
-- conversation exactly where it stood. models.chat runs under pcall
-- because the current chat survives transport errors and so must this:
-- the session surfaces the failure to the operator, and the loop returns
-- to user_input.
while true do
    tool_call('user_input', {})
    local messages = {}
    local events = runtime.events()
    for index = 1, #events do
        local event = events[index]
        if event.kind == 'user_message' then
            messages[#messages + 1] = { role = 'user', content = event.content }
        elseif event.kind == 'agent_message' then
            messages[#messages + 1] = { role = 'assistant', content = event.content }
        end
    end
    pcall(models.chat, messages, { model = ui().selected_model })
end
