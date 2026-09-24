What a model round exchanges, and the catalog and bindings a run resolves models through.

# The catalog

A host describes the models it can serve as a [`ModelCatalog`] of [`ModelDescriptor`]s, typically built from a gateway's model list or a pinned offline entry; building one fails with a [`ModelCatalogError`] when two descriptors share one id. Each descriptor is named by a validated [`ModelId`], a server namespace plus the caller-facing model name, and records the model's description, its context window, and its [`ThinkingMode`]. Invalid id components fail with a [`ModelIdError`].

# Roles and bindings

A prompt declares the model roles it needs in its frontmatter ([`ModelRoles`](crate::prompt::ModelRoles)). The host sets the run's current model with [`RunContext::model`](crate::RunContext::model), and [`Environment::prepare`](crate::Environment::prepare) binds every declared role to it, checking each role's hard keywords and context minimum against the descriptor. A failed check is reported in the prepare's [`Requirements`](crate::Requirements) with what the prompt required and what the model provides; soft keywords only document the author's intent. The result is the run's [`ModelBindings`], which resolve a role label to its id and descriptor. With no current model, declared roles stay unbound and selecting one at run time fails.

Inside the run, a prompt-local alias bound to a model and its frozen invocation parameters is a [`ModelBinding`]: its [`ModelInvocation`] fixes the sampling [`Temperature`], the generation cap, and the thinking switch for every round under that binding. A temperature outside `[0.0, 2.0]` or not finite is rejected with a [`TemperatureError`].

# One round

A model round is an [`Effect::Chat`](crate::effect::Effect::Chat) holding the binding, the conversation as [`Message`]s in wire order, the [`ToolSchema`]s advertised to the model, and the [`CompletionOptions`] built from the binding: the model named on the wire and the optional temperature, generation cap, and thinking switch.

The host answers with a [`Completion`] or a [`CompletionError`]. A completion holds the reassembled turn as a [`CompletionResult`] - reply text, or the [`ToolCall`]s the model requested, each with its id, name, and arguments readable through a typed [`ToolArguments`] view - together with the round's metadata: the model that served it, the finish reason, any reasoning text, and the token usage and timings it reported ([`metrics`](crate::metrics)). A [`CompletionError`] is classified by [`CompletionErrorKind`] and says whether it was a timeout. While a round streams, the host may forward each [`StreamDelta`] of reply text or reasoning to whoever watches the reply; the completion holds the whole turn either way.

The [`transport`](crate::transport) module is the codec that builds the request body and reads the response into a [`Completion`].
