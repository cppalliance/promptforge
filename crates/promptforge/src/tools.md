Tool descriptors, catalogs, identities, output, and errors.

Some tools run in the host's own process, such as fetching and rendering a web page, and others proxy through a gateway so a shared credential never leaves the server. The engine sees neither kind: it binds and advertises tools as data and issues each call as an effect naming the tool's stable identity. The implementations stay with the host.

# Describing tools

A [`ToolDescriptor`] is one tool as data: its [`ToolId`], the wire name advertised to a model, the one-sentence description the model reads, the JSON Schema its arguments must match, whether its output is structured JSON, and the co-activation conflicts of the capability that contributed it. A host assembles the descriptors of the capabilities it activated into a [`ToolCatalog`], which rejects a repeated id or a wire name a transport would reject with a [`ToolCatalogError`], and installs it with [`Environment::tools`](crate::Environment::tools).

A [`ToolId`] is a three-segment `namespace/pack/name` [`GlobalName`](crate::capabilities::GlobalName); its first two segments name the [`CapabilityId`](crate::capabilities::CapabilityId) that contributes it. Text that is not a valid id fails with a [`ToolIdError`].

# How a run binds and calls tools

A prompt declares tool slots in its frontmatter, each a prompt-local alias for a tool id. [`Environment::prepare`](crate::Environment::prepare) fills each slot by identity against the catalog and journals every fill into the run's [`ToolBindings`], which resolve alias to id to descriptor and never hold an implementation. A slot whose capability contributed nothing to the catalog is reported in [`Requirements::missing_required`](crate::Requirements::missing_required); a slot whose capability is present but contributed no such tool stays unfilled, and advertising it fails at run time with the alias named.

The run installs the filled tool and model slots into each section VM. The prompt-wide aliases and a section's additions form the scope the model sees, whose tools are advertised under their local aliases from the descriptor each binding holds; the model never sees a tool's global id. A call is issued as an [`Effect::ToolCall`](crate::effect::Effect::ToolCall) naming the tool's id and the alias it was called by, and the host resolves the implementation.

```
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{Environment, Prompt, RunContext};

let source = "---\nname: reader\ndescription: reads a page\npromptforge: 0\ncapabilities:\n  - example/web\ntools:\n  fetch: example/web/fetch\n---\n\n# Reader\n\n## Only\n\nDone.\n";
let (prompt, _parse_events) = Prompt::parse(source, "reader");
let prompt = prompt?;
let id = ToolId::parse("example/web/fetch")?;
let fetch = ToolDescriptor::new(
    id.clone(),
    "fetch",
    "Fetch a web page over HTTP.",
    serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
);
let env = Environment::new().tools(ToolCatalog::new(&[fetch])?);
let (ctx, requirements) = env.prepare(&prompt, RunContext::new("reader", 1, Timestamp::UNIX_EPOCH));
assert!(requirements.is_satisfied());
assert_eq!(ctx.tool_bindings().alias_id("fetch"), Some(&id));
# Ok::<(), Box<dyn std::error::Error>>(())
```

# Answering a tool call

A host answers a tool call with the tool's [`ToolOutput`] or its [`ToolError`]. Trust is part of the output, so it cannot be forgotten: [`ToolOutput::trusted`] marks text the host vouches for, and [`ToolOutput::untrusted`] marks external data, which the engine wraps in a nonce-guarded envelope before it reaches model input ([`OutputTrust`]). A [`ToolError`]'s message is handed back to the model, so it is written to be safe there; an underlying cause stays behind [`std::error::Error::source`], and [`ToolErrorKind`] classifies the failure for code.
