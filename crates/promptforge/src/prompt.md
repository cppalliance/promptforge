What a parsed prompt declares in its frontmatter.

# Parsing

[`Prompt::parse`](crate::Prompt::parse) reads a prompt source into a [`Prompt`](crate::Prompt), returning the parse-time events beside the outcome; a source that is not a valid prompt fails with a [`ParseError`](crate::ParseError) classified by [`ParseErrorKind`](crate::ParseErrorKind). A source is a PromptForge prompt only when its frontmatter declares a `promptforge:` version, and a run refuses a prompt whose version is missing or unsupported. The parsed [`Frontmatter`] is read through [`Prompt::frontmatter`](crate::Prompt::frontmatter); unknown frontmatter keys fail the parse rather than being ignored.

# Declarations

- Identity: the prompt's `name`, its one-line `description`, and the `promptforge` engine major it targets.
- Args: an [`ArgsDecl`] maps each arg name to its [`ArgDecl`] - an [`ArgType`], whether a call may omit it, a default, and a description. A prompt with no `args:` key has the default declaration, one optional string named `prose`.
- Files: an optional input and output [`FileDecl`], each a store path with a description.
- Capabilities: the [`CapabilityDecl`]s the prompt needs, in declaration order, each with its id, whether it is optional (skipped and logged when absent), and any prompt-side configuration. The host activates them before [`Environment::prepare`](crate::Environment::prepare).
- Model roles: [`ModelRoles`] maps each prompt-local label to a [`ModelRole`] - [`ModelKeyword`]s, a context minimum in tokens, and a description. Prepare binds each role to the run's current model and checks its hard keywords and context minimum; the [`model`](crate::model) module covers the binding.
- Tool slots: [`ToolSlots`] maps each prompt-local alias to a [`ToolSlot`] naming an exact tool id. Prepare fills each slot by identity against the host's catalog; the [`tools`](crate::tools) module covers the fill.

```
use promptforge::Prompt;
use promptforge::prompt::ArgType;

let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\nargs:\n  who:\n    type: string\n    description: Who to greet\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";
let (prompt, _parse_events) = Prompt::parse(source, "greeter");
let prompt = prompt?;
let frontmatter = prompt.frontmatter();
assert_eq!(frontmatter.name(), "greeter");
assert_eq!(frontmatter.promptforge(), Some(0));
assert_eq!(frontmatter.args().get("who").map(|arg| arg.kind()), Some(ArgType::String));
assert!(frontmatter.tools().is_empty());
# Ok::<(), promptforge::ParseError>(())
```
