# harness-api

The public API of the PromptForge harness family. Workshop and other clients depend on this crate alone: it holds the harness configuration, the gateway binding a client pushes at startup and on every gateway replacement, and the session, event, and delta types a client renders. Everything under `crates/harness/` is private to the family and reachable only through this crate.
