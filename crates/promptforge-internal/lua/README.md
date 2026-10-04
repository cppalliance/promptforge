# Engine Lua runtime

This crate runs each section's Lua code in a fresh, restricted sandbox and installs the Engine globals for the run's store, models, tools, messages, and tasks. An Engine call that waits on the outside world suspends the Lua code and hands a request to the executor, so the sandbox itself performs no I/O. Capabilities can add Lua globals of their own, checked against a reserved-name list.
