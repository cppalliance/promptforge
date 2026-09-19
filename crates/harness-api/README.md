# harness-api

The public door into the PromptForge harness family. Workshop and other clients depend on this crate alone: it carries the harness configuration, the gateway binding a client pushes at startup and on every gateway replacement, and the session, event, and delta types a client renders. Everything under `crates/harness/` is private to the family and reachable only through this crate.
