Capability identities and the global naming grammar they are built on.

# Capabilities

A capability is the activation unit: host code that runs at run setup and makes services, such as tools, available to the run. The engine knows capabilities by identity alone. A prompt declares the ones it needs in its frontmatter ([`CapabilityDecl`](crate::prompt::CapabilityDecl)), an exact tool slot names one through its tool id's first two segments, and a [`ToolDescriptor`](crate::tools::ToolDescriptor) records the conflicts of the capability that contributed it. Activation itself - resolving the declarations, checking conflicts, and assembling the [`ToolCatalog`](crate::tools::ToolCatalog) - is the host's and happens before [`Environment::prepare`](crate::Environment::prepare); a host reports what it could not satisfy through the [`Requirements`](crate::Requirements) prepare returns, including any [`CapabilityConflict`](crate::CapabilityConflict).

A [`CapabilityId`] is a two-segment `namespace/pack` name; text that is not one fails with a [`CapabilityIdError`], classified by [`CapabilityIdErrorKind`].

# The global naming grammar

Capability and tool ids share one grammar, [`GlobalName`], and encode their kind by arity: a capability is `namespace/pack` and a tool is `namespace/pack/name`, so a reader tells the kind of any name by counting segments. A namespace is reverse-DNS (`org.rustalliance`) or the reserved first-party prefix `promptforge`. Segments are lowercase ASCII alphanumerics plus `-`, `_`, and `.`, and comparison is case-sensitive. Names are unversioned: a `@` is a parse error, reported as a [`GlobalNameError`] classified by [`GlobalNameErrorKind`].

```
use promptforge::capabilities::{CapabilityId, GlobalName};
use promptforge::tools::ToolId;

let tool = ToolId::parse("promptforge/web/fetch")?;
assert_eq!(tool.capability(), CapabilityId::parse("promptforge/web")?);
assert!(GlobalName::parse("promptforge/web@1").is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```
