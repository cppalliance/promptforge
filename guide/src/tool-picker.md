# Tool Picker

A sentence-embedding resolver that turns "read a file from disk" into the tool that does it - no LLM call, no network, no guessing. You describe your tools in prose, build a picker over the catalog, and ask it which tool a need refers to. It answers with a decision: one bound tool, a duplicate report, an ambiguous shortlist, or an abstention. The model is compiled into the library, so there is no path to configure and no weights to ship. Querying is a dot product, not an API call. Determinism is structural: the same inputs always produce the same answer.

## Identity, Descriptors, and Catalogs

### Tool Identity

Every tool is identified by a `(server, name)` pair. The pair is structural - never concatenated - so a server or name containing any delimiter stays unambiguous.

```rust
use promptforge_tool_picker::{ToolId, ToolDescriptor, ToolAnnotations, Catalog};
use serde_json::json;

let id = ToolId::new("files", "read_file");
assert_eq!(id.server(), "files");
assert_eq!(id.name(), "read_file");
```

### Descriptors

A `ToolDescriptor` carries the identity, a prose description, a JSON Schema for the tool's arguments, and optional behavioral hints:

```rust
let tool = ToolDescriptor::new(
    ToolId::new("files", "read_file"),
    "Read a file from disk",
    json!({"properties": {"path": {"type": "string"}}}),
);

let tool = tool.with_annotations(
    ToolAnnotations::new()
        .with_read_only(true)
        .with_destructive(false),
);

assert_eq!(tool.name(), "read_file");
assert_eq!(tool.description(), "Read a file from disk");
assert_eq!(tool.annotations().read_only(), Some(true));
```

Annotations are optional and advisory. They affect ranking only as a tie-break between candidates that score identically. A positive read-only claim is preferred first, then non-destructive, then idempotent.

### Catalogs

A `Catalog` is an ordered collection of descriptors:

```rust
let catalog = Catalog::new(vec![
    ToolDescriptor::new(
        ToolId::new("files", "read_file"),
        "Read a file from disk",
        json!({"properties": {"path": {"type": "string"}}}),
    ),
    ToolDescriptor::new(
        ToolId::new("net", "fetch_url"),
        "Fetch a web page over HTTP",
        json!({"properties": {"url": {"type": "string"}}}),
    ),
]);

assert_eq!(catalog.len(), 2);

let found = catalog.get(&ToolId::new("net", "fetch_url"));
assert_eq!(found.map(|t| t.name()), Some("fetch_url"));

for tool in &catalog {
    println!("{}: {}", tool.name(), tool.description());
}
```

You can also build a catalog from an iterator or a `Vec`:

```rust
let catalog: Catalog = vec![/* descriptors */].into();
let catalog: Catalog = some_iterator.collect();
```

### JSON Deserialization

With the `serde` feature (enabled by default), catalogs deserialize from JSON. The identity fields are flat on each descriptor. The schema field accepts both `input_schema` and its MCP spelling `inputSchema`:

```json
[
  {
    "server": "files",
    "name": "read_file",
    "description": "Read a file from disk",
    "inputSchema": {
      "properties": { "path": { "type": "string" } }
    },
    "annotations": { "readOnlyHint": true }
  }
]
```

Duplicate identities in a catalog are accepted. Two tools claiming the same identity is a result the engine reports, not an input it refuses.

## Building a Picker

The simplest path loads the model and indexes a catalog in one call:

```rust
use promptforge_tool_picker::{ToolPicker, Catalog, Config};

let picker = ToolPicker::build(catalog, Config::default())?;
assert_eq!(picker.len(), 2);
```

### Sharing a Model

Loading the model is the expensive step - it materializes the compiled-in weights into memory. If you serve several catalogs, load the model once and build each picker against it:

```rust
use promptforge_tool_picker::Model;

let model = Model::load()?;

let files_picker = ToolPicker::build_with_model(&model, files_catalog, Config::default())?;
let weather_picker = ToolPicker::build_with_model(&model, weather_catalog, Config::default())?;
```

`Model` is cheap to clone (it shares the loaded weights through an `Arc`), and it is `Send + Sync + 'static`, so you can pass it across threads.

### Rebuilding

When your catalog changes - a reconnected server, a watched directory - rebuild from the existing picker to preserve its model and configuration:

```rust
let updated = picker.rebuild(new_catalog)?;
```

The original picker is immutable and still answers from its own catalog. The rebuilt picker answers from the new catalog with the same model and config.

You can iterate a picker's tools with `picker.iter()` or `for tool in &picker`, and look up a specific tool with `picker.get(&id)`.

## Resolving a Need

`resolve` takes a plain-English need and returns one of four outcomes:

```rust
use promptforge_tool_picker::Outcome;

match picker.resolve("read a file from disk")? {
    Outcome::Bind(tool) => {
        println!("call {}", tool.name());
    }
    Outcome::Duplicate(group) => {
        println!("{} publishes {} twins", group.first().server(), group.len());
    }
    Outcome::Ambiguous(group) => {
        for tool in &group {
            println!("candidate: {}/{}", tool.server(), tool.name());
        }
    }
    Outcome::Absent => {
        println!("no tool covers this need");
    }
    _ => {}
}
```

`Absent` is a successful answer, not an error. An `Err` from `resolve` means the need could not be embedded (tokenization or inference failed), so no answer was produced at all.

### Lifetime of Results

Results borrow the picker's descriptors. No schema or descriptor is deep-cloned. If you need to keep a tool identity beyond the picker's lifetime, clone the specific `ToolId`:

```rust
let kept_id: ToolId = match picker.resolve("read a file")? {
    Outcome::Bind(tool) => tool.id().clone(),
    _ => return Ok(()),
};
```

### Candidate Groups

A `CandidateGroup` (from `Duplicate` or `Ambiguous`) always contains at least two entries. You can inspect them with `group.first()`, `group.second()`, `group.get(index)`, `group.len()`, and `group.iter()`.

## Shortlisting

`shortlist` returns candidates above the similarity floor without making a final decision, so the caller can choose:

```rust
let candidates = picker.shortlist("read a file from disk", 3)?;

for tool in &candidates {
    println!("{}: {}", tool.name(), tool.description());
}

if candidates.is_empty() {
    println!("nothing relevant");
}
```

`resolve` and `shortlist` never contradict each other on relevance. If `resolve` abstains, `shortlist` returns nothing. If `resolve` binds a tool, `shortlist` offers exactly that tool.

The solo-candidate exception applies to both: when one candidate sits between the solo floor and the strict similarity floor, and no runner-up reaches the solo floor, that candidate is offered.

A `limit` of zero returns an empty shortlist without embedding the need. The `Shortlist` type offers `.len()`, `.is_empty()`, `.first()`, `.get(index)`, and `.iter()`.

## Configuration

`Config::default()` provides justified defaults. A caller who has not measured their own catalog should change none of them:

| Threshold | Default | Meaning |
|---|---|---|
| `similarity_floor` | 0.825 | Cosine similarity a candidate must reach to be considered |
| `margin` | 0.05 | Gap the leader must clear the runner-up by to bind |
| `duplicate_threshold` | 0.98 | Tool-to-tool similarity at which two tools are treated as twins |
| `solo_floor` | 0.5 | Minimum score for a lone candidate to bind below the strict floor |
| `top_k` | 3 | How many candidates a duplicate or ambiguous outcome reports |

### Adjusting Thresholds

Adjust one threshold at a time with checked consuming setters:

```rust
use promptforge_tool_picker::Config;

let config = Config::default()
    .with_similarity_floor(0.85)?
    .with_top_k(5)?;

assert_eq!(config.top_k().get(), 5);
```

Every `Config` is always valid. Thresholds must be finite and in `0.0..=1.0`. `top_k` must be nonzero. There is no `validate` method because no public operation can produce an invalid value.

A setter that receives an out-of-domain value returns `ConfigError`, which names the rejected field:

```rust
use promptforge_tool_picker::{ConfigError, ConfigField};

let error: ConfigError = Config::default()
    .with_similarity_floor(2.0)
    .expect_err("out of domain");
assert_eq!(error.field(), ConfigField::SimilarityFloor);
```

### JSON Serialization

With the `serde` feature, configuration serializes and deserializes as JSON. Absent fields take their defaults, and checked deserialization rejects invalid wire values:

```json
{"similarity_floor": 0.85, "top_k": 5}
```

### Decision Precedence

Decision precedence is fixed: absent, then duplicate, then bind, then ambiguous. The similarity floor is checked first. Then same-server twins are detected against the duplicate threshold (measured between the tools' own embeddings, not against the query). Then the margin test separates a clear leader from a near-tie. Every threshold boundary is inclusive - a score exactly at the floor is considered.

### The Solo-Candidate Rule

When the top candidate scores at or above `solo_floor` but below `similarity_floor`, and no runner-up reaches the solo floor, the leader binds. Two candidates between the floors abstain.

## Near-Duplicate Detection

`near_duplicates` compares selected tools against the configured duplicate threshold using the picker's stored embeddings. The comparison is tool-to-tool, not need-to-tool - it measures how alike two tools' own descriptions are, independent of any query.

```rust
let pairs = picker.near_duplicates(&[
    ToolId::new("calendar", "create_event"),
    ToolId::new("calendar", "add_event"),
])?;

for pair in &pairs {
    println!(
        "{}/{} and {}/{} are {:.3} similar",
        pair.first().server(), pair.first().name(),
        pair.second().server(), pair.second().name(),
        pair.similarity(),
    );
}
```

Every requested identity must be present in the picker. An absent identity returns `SelectionError` before any comparison happens, naming the first missing `ToolId` via `error.missing_id()`. Repeated identities are idempotent set membership.

Pairs are output in catalog pair order. Each `NearDuplicate` provides `.first()`, `.second()`, and `.similarity()`. The `NearDuplicates` collection provides `.len()`, `.is_empty()`, `.get(index)`, and `.iter()`.

## Error Handling

Each fallible operation returns its own error type. There is no crate-wide error enum.

| Operation | Error Type | Key Accessor |
|---|---|---|
| `Model::load` | `ModelLoadError` | - |
| `ToolPicker::build` | `BuildError` | - |
| `ToolPicker::build_with_model` | `IndexError` | - |
| `ToolPicker::resolve` / `shortlist` | `QueryError` | `.kind()` |
| `ToolPicker::near_duplicates` | `SelectionError` | `.missing_id()` |
| `Config::with_*` | `ConfigError` | `.field()` |

`QueryError::kind()` returns a `QueryErrorKind` that classifies the failure without exposing dependency types:

```rust
use promptforge_tool_picker::{QueryError, QueryErrorKind};

match error.kind() {
    QueryErrorKind::Tokenization => { /* the need text could not be tokenized */ }
    QueryErrorKind::Inference => { /* the model's forward pass failed */ }
    QueryErrorKind::InvalidEmbedding => { /* the produced vector could not be normalized */ }
    _ => {}
}
```

`BuildError` wraps either a `ModelLoadError` or an `IndexError`, and implements `From` for both. All error types are `Send + Sync + 'static`.

## Determinism and the Embedded Model

The crate promises deterministic results: the same model bytes, dependency versions, target, execution environment, catalog, configuration, and need always produce the same outcome. Cross-platform byte-identical vectors at floating-point boundaries are not promised.

The embedding model (BAAI/bge-small-en-v1.5, 384 dimensions) is compiled into the library. There is no model path in the configuration, no weights file to deploy, and no network call at runtime. The build script fetches the model from the Hugging Face Hub at a pinned immutable commit, verifies every file against a hardcoded SHA-256 digest, and downcasts the fp32 weights to fp16 to halve binary size. Subsequent builds reuse the Hugging Face cache.

At load time, the crate verifies that the embedded weights' provenance metadata matches the pinned repository and revision. A mismatched or substituted checkpoint fails loudly rather than silently altering rankings.

The first build requires network access to the Hugging Face Hub (about 130 MB download). Set `HF_HUB_CACHE` or `HF_HOME` to point at an existing cache, or `HF_ENDPOINT` to a reachable mirror.
