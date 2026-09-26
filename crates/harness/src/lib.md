The public API of the PromptForge harness family. Workshop and other clients depend on this crate alone: it holds the harness configuration, the gateway binding a client pushes at startup and on every gateway replacement, the session, event, and delta types a client renders, the awaitable [`cancel::CancelHandle`] a client selects over, and [`display_chain`], the renderer that turns a harness error and its cause chain into one line for a person. Every other harness crate is private to the family and reachable only through this one.

# The harness handle and its bindings

[`Harness`] is built from a [`HarnessConfig`] and owns the sessions it serves. A client pushes three bindings into it, always as data and never as a handle into the client:

- [`Harness::set_gateway`] takes a [`GatewayBinding`]. The client calls it at startup and on every gateway replacement, and the harness rebuilds its capability registry and model client when the generation changes.
- [`Harness::set_catalog`] takes a [`CatalogBinding`], the client's chat-capable model list.
- [`Harness::set_host`] takes a [`HostSnapshot`], the client's model selection and workspace roots.

A gateway bearer key is never written to logs or `Debug` output.

# Sessions

The session vocabulary clients speak and render is ids, launch requests, durable events, and ephemeral deltas. The live [`Session`] handle is what a client launches, sends input to, cancels, closes, and subscribes to events and deltas through. A session's transcript is the harness run log: subscribe first, then read [`Session::transcript`] past the last seen index.

A session announces its input waits with [`WaitFrame`] values, and a refused answer returns a [`WaitError`]. A session's failure reports include a [`FailureKind`] a client matches on beside the display message; the sentence is never the classifier.

# Errors

A harness error's `Display` holds only its own message. A client that shows one to a person renders the cause chain through [`display_chain`].
