# The agent loop

This chapter teaches you the loop shape every agent follows. It is worth learning because the loop is how your program and the host talk: once you understand one request in flight, every host call reads the same way.

## The smallest loop

````lua
models.use('writer')

while true do
  local text = models.infer('Write one sentence about the sea.')
  log(text)
end
````

Run this program and it asks the model for one sentence about the sea, records the answer, and starts again. It keeps going until the host cancels the run.

Work through the lines. `models.use('writer')` selects the catalog model named `writer` for the run. `while true do` starts a loop with no exit of its own. `models.infer('Write one sentence about the sea.')` runs one direct, tool-free text completion on a fresh conversation and resumes with the completed text. `log(text)` records that text in the run's event stream.

## One request in flight

Each host call suspends your program with exactly one request in flight. When your program calls `models.infer`, it stops at that line and the host takes over. The host dispatches the one request and resumes your program at the same line with that request's answer as the return value.

Write your program as if each host call were an ordinary synchronous call. There is no callback and no second request to track: the program carries exactly one request in flight, and it always resumes with that request's answer.

## The two round calls

Two calls carry almost every agent. `models.chat(messages, opts)` runs one stateless model round over a message list your program builds, and the round is tool-capable. `tool_call(alias, args)` dispatches any tool in the agent's catalog by its wire name, and every tool in the catalog is in scope under its alias.

Both calls follow the loop rule: one request in flight, resumed with the answer. Everything else about them is detail on top of that rule.

## When the loop ends

A loop that never returns is legal. The host lets it run, and only the run's cancel flag stops it. Your program chooses its own shape: return when the work is done, or loop until the run is cancelled.

