# promptforge-tool-picker API redesign

## Executive decision

`promptforge-tool-picker` remains one resolver crate, not a general embedding library. Its supported product surface is:

- immutable tool identity and descriptor values;
- an ordered catalog;
- validated resolution policy;
- an opaque reusable model handle;
- an immutable picker built from a catalog;
- borrowing resolution, shortlist, and near-duplicate results;
- narrow, opaque errors for configuration, model loading, indexing, querying, and selected-scope validation.

The redesign removes raw assets, raw vectors, model dimensions, direct encoder access, public enriched-text mechanics, public field mutation, unchecked configuration, deep descriptor cloning in query results, and the crate-wide error enum. It preserves the current decision policy, including abstention, same-server duplicate classification, inclusive thresholds, deterministic ordering within one fixed execution environment, the solo-candidate exception, empty-catalog support, catalog order, duplicate identities, and model reuse.

This is the smallest coherent break because the crate is unpublished, private, and pre-stability. Compatibility shims would preserve the invalid states and backend commitments the redesign is intended to remove.

## Effective current public API

The current crate root exposes the following surface.

```rust
pub mod assets {
    pub static WEIGHTS_SAFETENSORS: &[u8];
    pub static TOKENIZER_JSON: &[u8];
    pub static CONFIG_JSON: &[u8];
    pub const SOURCE_REPO: &str;
    pub const SOURCE_REVISION: &str;
}

pub const QUALIFIED_KEY_SEPARATOR: char;

pub struct ToolId { /* private fields */ }
impl ToolId {
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Self;
    pub fn server(&self) -> &str;
    pub fn name(&self) -> &str;
    pub fn qualified_key(&self) -> String;
}

pub struct ToolAnnotations {
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
}

pub struct ToolDescriptor {
    pub id: ToolId,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub annotations: ToolAnnotations,
}
impl ToolDescriptor {
    pub fn new(
        id: ToolId,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self;
    pub fn with_annotations(self, annotations: ToolAnnotations) -> Self;
    pub fn server(&self) -> &str;
    pub fn name(&self) -> &str;
    pub fn parameter_names(&self) -> Vec<&str>;
    pub fn enriched_text(&self) -> String;
}

pub struct Catalog { /* Vec<ToolDescriptor> */ }
impl Catalog {
    pub fn new(tools: Vec<ToolDescriptor>) -> Self;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn tools(&self) -> &[ToolDescriptor];
    pub fn iter(&self) -> std::slice::Iter<'_, ToolDescriptor>;
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor>;
}
impl IntoIterator for Catalog { /* ToolDescriptor */ }
impl IntoIterator for &Catalog { /* &ToolDescriptor */ }
impl FromIterator<ToolDescriptor> for Catalog;

#[non_exhaustive]
pub enum ModelId {
    BgeSmallEnV15,
}
impl ModelId {
    pub fn as_str(self) -> &'static str;
}

pub struct Config {
    pub model_id: ModelId,
    pub similarity_floor: f32,
    pub margin: f32,
    pub duplicate_threshold: f32,
    pub solo_floor: f32,
    pub top_k: usize,
}
impl Config {
    pub fn validate(&self) -> Result<()>;
}

pub const EMBEDDING_DIMENSIONS: usize;

pub struct Embedder { /* backend state */ }
impl Embedder {
    pub fn new() -> Result<Self>;
    pub fn embed(&self, text: &str) -> Result<Vec<f32>>;
    pub fn embed_all<I, S>(&self, texts: I) -> Result<Vec<Vec<f32>>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>;
}

#[non_exhaustive]
pub enum Error {
    ThresholdOutOfRange { /* private fields */ },
    EmptyShortlist,
    ToolNotInCatalog { /* private fields */ },
    ModelLoad { /* private fields */ },
    Tokenize { /* private fields */ },
    Embed { /* private fields */ },
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub struct NearDuplicate {
    pub first: ToolDescriptor,
    pub second: ToolDescriptor,
    pub similarity: f32,
}

pub struct ToolPicker { /* private fields */ }
impl ToolPicker {
    pub fn build(catalog: Catalog, config: Config) -> Result<Self>;
    pub fn build_with(
        embedder: std::sync::Arc<Embedder>,
        catalog: Catalog,
        config: Config,
    ) -> Result<Self>;
    pub fn rebuild(&self, catalog: Catalog) -> Result<Self>;
    pub fn embedder(&self) -> &std::sync::Arc<Embedder>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn tools(&self) -> &[ToolDescriptor];
    pub fn config(&self) -> &Config;
    pub fn resolve(&self, need: &str) -> Result<Outcome>;
    pub fn shortlist(&self, need: &str, k: usize) -> Result<Vec<ToolDescriptor>>;
    pub fn near_duplicates(&self, ids: &[ToolId]) -> Result<Vec<NearDuplicate>>;
    pub fn vector(&self, index: usize) -> Option<&[f32]>;
}

pub enum Outcome {
    Bind(ToolDescriptor),
    Duplicate(Vec<ToolDescriptor>),
    Ambiguous(Vec<ToolDescriptor>),
    Absent,
}
```

Serde implementations are unconditional. `ToolId` serializes as `{server, name}`. `ToolDescriptor` flattens that identity and accepts both `input_schema` and `inputSchema`. `Catalog` serializes as an array. `Config` fills absent fields from defaults.

## Proposed public API

All public structs and enums below derive `Debug` and the additional value traits shown where semantically valid. Constructors, consuming setters, and pure transforms carry `#[must_use]`. Every public iterator has a named type. Every public error is an opaque wrapper over a private representation that retains dependency errors as sources.

### Identity, annotations, and descriptors

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ToolId {
    server: String,
    name: String,
}

impl ToolId {
    #[must_use]
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Self;

    #[must_use]
    pub fn server(&self) -> &str;

    #[must_use]
    pub fn name(&self) -> &str;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolAnnotations {
    read_only: Option<bool>,
    destructive: Option<bool>,
    idempotent: Option<bool>,
}

impl ToolAnnotations {
    #[must_use]
    pub fn new() -> Self;

    #[must_use]
    pub fn read_only(&self) -> Option<bool>;

    #[must_use]
    pub fn destructive(&self) -> Option<bool>;

    #[must_use]
    pub fn idempotent(&self) -> Option<bool>;

    #[must_use]
    pub fn with_read_only(self, value: bool) -> Self;

    #[must_use]
    pub fn with_destructive(self, value: bool) -> Self;

    #[must_use]
    pub fn with_idempotent(self, value: bool) -> Self;
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolDescriptor {
    id: ToolId,
    description: String,
    input_schema: serde_json::Value,
    annotations: ToolAnnotations,
}

impl ToolDescriptor {
    #[must_use]
    pub fn new(
        id: ToolId,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self;

    #[must_use]
    pub fn with_annotations(self, annotations: ToolAnnotations) -> Self;

    #[must_use]
    pub fn id(&self) -> &ToolId;

    #[must_use]
    pub fn server(&self) -> &str;

    #[must_use]
    pub fn name(&self) -> &str;

    #[must_use]
    pub fn description(&self) -> &str;

    #[must_use]
    pub fn input_schema(&self) -> &serde_json::Value;

    #[must_use]
    pub fn annotations(&self) -> ToolAnnotations;
}
```

`ToolId` accepts all strings. It does not promise a concatenated representation, so embedded delimiters create no collision. `ToolDescriptor` retains `serde_json::Value` intentionally because an arbitrary JSON Schema is part of the catalog input, not an embedding-backend detail. The crate does not validate or execute that schema.

`parameter_names` and `enriched_text` become crate-private. They define the calibrated internal encoding and are not caller operations. Their internal implementation may use a private named iterator or direct writing into one destination string.

### Catalog

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Catalog {
    tools: Vec<ToolDescriptor>,
}

pub type CatalogIter<'a> = std::slice::Iter<'a, ToolDescriptor>;
pub type CatalogIterMut<'a> = std::slice::IterMut<'a, ToolDescriptor>;
pub type CatalogIntoIter = std::vec::IntoIter<ToolDescriptor>;

impl Catalog {
    #[must_use]
    pub fn new(tools: Vec<ToolDescriptor>) -> Self;

    #[must_use]
    pub fn len(&self) -> usize;

    #[must_use]
    pub fn is_empty(&self) -> bool;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CatalogIter<'_>;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter_mut(&mut self) -> CatalogIterMut<'_>;

    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor>;
}

impl IntoIterator for Catalog {
    type Item = ToolDescriptor;
    type IntoIter = CatalogIntoIter;
}

impl<'a> IntoIterator for &'a Catalog {
    type Item = &'a ToolDescriptor;
    type IntoIter = CatalogIter<'a>;
}

impl<'a> IntoIterator for &'a mut Catalog {
    type Item = &'a mut ToolDescriptor;
    type IntoIter = CatalogIterMut<'a>;
}

impl FromIterator<ToolDescriptor> for Catalog;
impl From<Vec<ToolDescriptor>> for Catalog;
```

`tools()` is removed because `iter`, `IntoIterator`, `len`, and `get` cover the collection contract without exposing the backing slice as a second interface. Mutable iteration is safe before the catalog is consumed by a picker. A picker owns its catalog and exposes no mutable access.

Duplicate identities remain accepted. `get` continues to return the first matching descriptor. Catalog order remains significant.

### Validated configuration

```rust
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Config {
    similarity_floor: f32,
    margin: f32,
    duplicate_threshold: f32,
    solo_floor: f32,
    top_k: std::num::NonZeroUsize,
}

impl Default for Config;

impl Config {
    #[must_use]
    pub fn similarity_floor(&self) -> f32;

    #[must_use]
    pub fn margin(&self) -> f32;

    #[must_use]
    pub fn duplicate_threshold(&self) -> f32;

    #[must_use]
    pub fn solo_floor(&self) -> f32;

    #[must_use]
    pub fn top_k(&self) -> std::num::NonZeroUsize;

    #[must_use]
    pub fn with_similarity_floor(self, value: f32) -> Result<Self, ConfigError>;

    #[must_use]
    pub fn with_margin(self, value: f32) -> Result<Self, ConfigError>;

    #[must_use]
    pub fn with_duplicate_threshold(self, value: f32) -> Result<Self, ConfigError>;

    #[must_use]
    pub fn with_solo_floor(self, value: f32) -> Result<Self, ConfigError>;

    #[must_use]
    pub fn with_top_k(self, value: usize) -> Result<Self, ConfigError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigField {
    SimilarityFloor,
    Margin,
    DuplicateThreshold,
    SoloFloor,
    TopK,
}

#[derive(Debug, thiserror::Error)]
pub struct ConfigError(/* private */);

impl ConfigError {
    #[must_use]
    pub fn field(&self) -> ConfigField;
}
```

Every successful `Config` construction is valid. There is no public `validate` method because no public operation can create an invalid value. Threshold setters accept only finite values in `0.0..=1.0`. This is documented as the supported and calibrated policy domain, not the mathematical cosine range. `top_k` is stored as `NonZeroUsize`.

`ModelId` and `model_id` are removed. One compiled model is an implementation fact, not a caller choice.

### Reusable model and picker lifecycle

```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Model {
    /* Arc-backed private model state */
}

impl Model {
    #[must_use]
    pub fn load() -> Result<Self, ModelLoadError>;
}

#[derive(Debug)]
#[non_exhaustive]
pub struct ToolPicker {
    /* catalog, config, Model, validated vector index */
}

impl ToolPicker {
    #[must_use]
    pub fn build(catalog: Catalog, config: Config) -> Result<Self, BuildError>;

    #[must_use]
    pub fn build_with_model(
        model: &Model,
        catalog: Catalog,
        config: Config,
    ) -> Result<Self, IndexError>;

    #[must_use]
    pub fn rebuild(&self, catalog: Catalog) -> Result<Self, IndexError>;

    #[must_use]
    pub fn len(&self) -> usize;

    #[must_use]
    pub fn is_empty(&self) -> bool;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> ToolIter<'_>;

    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor>;

    #[must_use]
    pub fn config(&self) -> &Config;

    pub fn resolve(&self, need: &str) -> Result<Outcome<'_>, QueryError>;

    pub fn shortlist(&self, need: &str, limit: usize) -> Result<Shortlist<'_>, QueryError>;

    pub fn near_duplicates(
        &self,
        ids: &[ToolId],
    ) -> Result<NearDuplicates<'_>, SelectionError>;
}

pub struct ToolIter<'a>(/* private named iterator */);
impl<'a> Iterator for ToolIter<'a> {
    type Item = &'a ToolDescriptor;
}
impl DoubleEndedIterator for ToolIter<'_>;
impl ExactSizeIterator for ToolIter<'_>;
impl std::iter::FusedIterator for ToolIter<'_>;
```

`Model` is the only public model-lifecycle concept. It is cheap to clone, but `build_with_model` borrows it so callers need not clone. The internal `Arc` and concrete tokenizer or tensor backend remain replaceable.

`ToolPicker::build` preserves the convenient one-call path. `build_with_model` supports several catalogs without exposing `Embedder` or `Arc`. `rebuild` remains the preferred path for replacing one picker's catalog while preserving model and policy.

There is no `model` getter. A caller that needs several independent policies retains the `Model` it loaded. There is no vector getter.

### Borrowing outcomes and invariant-bearing groups

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Outcome<'a> {
    Bind(&'a ToolDescriptor),
    Duplicate(CandidateGroup<'a>),
    Ambiguous(CandidateGroup<'a>),
    Absent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateGroup<'a> {
    /* at least two catalog positions, private */
}

impl<'a> CandidateGroup<'a> {
    #[must_use]
    pub fn len(&self) -> usize;

    #[must_use]
    pub fn first(&self) -> &'a ToolDescriptor;

    #[must_use]
    pub fn second(&self) -> &'a ToolDescriptor;

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a ToolDescriptor>;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CandidateIter<'a, '_>;
}

pub struct CandidateIter<'a, 'group>(/* private */);
impl<'a> Iterator for CandidateIter<'a, '_> {
    type Item = &'a ToolDescriptor;
}
impl DoubleEndedIterator for CandidateIter<'_, '_>;
impl ExactSizeIterator for CandidateIter<'_, '_>;
impl std::iter::FusedIterator for CandidateIter<'_, '_>;

#[derive(Debug, Clone, PartialEq)]
pub struct Shortlist<'a> {
    /* zero or more catalog positions, private */
}

impl<'a> Shortlist<'a> {
    #[must_use]
    pub fn len(&self) -> usize;

    #[must_use]
    pub fn is_empty(&self) -> bool;

    #[must_use]
    pub fn first(&self) -> Option<&'a ToolDescriptor>;

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a ToolDescriptor>;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> CandidateIter<'a, '_>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct NearDuplicate<'a> {
    /* two distinct catalog positions and similarity, private */
}

impl<'a> NearDuplicate<'a> {
    #[must_use]
    pub fn first(&self) -> &'a ToolDescriptor;

    #[must_use]
    pub fn second(&self) -> &'a ToolDescriptor;

    #[must_use]
    pub fn similarity(&self) -> f32;
}

#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicates<'a> {
    /* zero or more pairs, private */
}

impl<'a> NearDuplicates<'a> {
    #[must_use]
    pub fn len(&self) -> usize;

    #[must_use]
    pub fn is_empty(&self) -> bool;

    #[must_use]
    pub fn get(&self, index: usize) -> Option<NearDuplicate<'a>>;

    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> NearDuplicateIter<'a, '_>;
}

pub struct NearDuplicateIter<'a, 'pairs>(/* private */);
impl<'a> Iterator for NearDuplicateIter<'a, '_> {
    type Item = NearDuplicate<'a>;
}
impl DoubleEndedIterator for NearDuplicateIter<'_, '_>;
impl ExactSizeIterator for NearDuplicateIter<'_, '_>;
impl std::iter::FusedIterator for NearDuplicateIter<'_, '_>;
```

`CandidateGroup` has no public constructor and always contains at least two candidates. `NearDuplicate` has no public constructor and always refers to two distinct catalog entries whose stored-vector similarity met the configured inclusive threshold. Query results borrow the picker, so descriptors and schemas are never deep-cloned. Callers that must retain data after the picker can clone the specific `ToolId`, description, annotations, or descriptor explicitly.

`shortlist` preserves the solo-candidate exception. It returns the lone leader when the leader is at or above `solo_floor`, below `similarity_floor`, and no runner-up reaches `solo_floor`. With two such candidates it returns an empty shortlist, matching `Outcome::Absent`. A zero limit returns an empty shortlist without embedding the need.

### Narrow opaque errors

```rust
#[derive(Debug, thiserror::Error)]
pub struct ModelLoadError(/* private */);

#[derive(Debug, thiserror::Error)]
pub struct IndexError(/* private */);

#[derive(Debug, thiserror::Error)]
pub struct BuildError(/* private */);

#[derive(Debug, thiserror::Error)]
pub struct QueryError(/* private */);

#[derive(Debug, thiserror::Error)]
pub struct SelectionError(/* private */);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    Tokenization,
    Inference,
    InvalidEmbedding,
}

impl QueryError {
    #[must_use]
    pub fn kind(&self) -> QueryErrorKind;
}

impl SelectionError {
    #[must_use]
    pub fn missing_id(&self) -> &ToolId;
}

impl From<ModelLoadError> for BuildError;
impl From<IndexError> for BuildError;
```

Each wrapper has a private operation-specific representation. Dependency errors are retained behind `#[source]` as concrete private fields or boxed `Error + Send + Sync + 'static`. No Candle, tokenizer, serde, or safetensors error appears in a public signature. Display text is a lowercase noun phrase. Compile-time tests assert `Send + Sync + 'static`.

There is no crate-wide `Error` and no public `Result` alias. Signatures state the actual failure unit directly.

### Serde feature

The manifest adds an additive `serde` feature and keeps it enabled by default for wire compatibility:

```toml
[features]
default = ["serde"]
serde = ["dep:serde"]
```

`ToolId`, `ToolAnnotations`, `ToolDescriptor`, `Catalog`, and `Config` use `cfg_attr(feature = "serde", derive(...))` or equivalent manual implementations. `Config` deserialization goes through a private raw form and checked conversion, so successful deserialization cannot produce an invalid value. Existing JSON spellings remain unchanged:

- identity remains flat on descriptors;
- `inputSchema` remains accepted as an alias;
- annotation fields retain MCP spellings;
- catalogs remain arrays;
- absent configuration fields retain defaults.

`serde_json` remains a core dependency because `ToolDescriptor::input_schema` intentionally accepts arbitrary JSON Schema. Disabling `serde` removes serialization implementations, not the schema value type. This is a deliberate product boundary.

## Removals and visibility reductions

Remove from the external surface:

- `assets` and all five asset values;
- `QUALIFIED_KEY_SEPARATOR`;
- `ToolId::qualified_key`;
- public fields on `ToolAnnotations`, `ToolDescriptor`, `Config`, and `NearDuplicate`;
- `ToolDescriptor::parameter_names`;
- `ToolDescriptor::enriched_text`;
- `Catalog::tools`;
- `ModelId`;
- `Config::model_id`;
- `Config::validate`;
- `EMBEDDING_DIMENSIONS`;
- `Embedder`;
- `Embedder::new`, `embed`, and `embed_all`;
- crate-wide `Error` and `Result`;
- `ToolPicker::build_with`;
- `ToolPicker::embedder`;
- `ToolPicker::tools`;
- `ToolPicker::vector`;
- owned `Outcome` payloads;
- raw `Vec<ToolDescriptor>` shortlist and outcome payloads;
- publicly constructible `NearDuplicate`.

Keep crate-private:

- generated assets and provenance;
- concrete backend model and vector dimensions;
- calibrated enriched-text construction and parameter-name extraction;
- vector rows, ranking candidates, catalog positions, and validated vector layout;
- result constructors;
- error representations and dependency sources.

No public trait is introduced. There is one model implementation and no caller-supplied backend requirement, so a trait would create an abstraction without a second implementation. Consequently no sealing mechanism is needed. If a backend trait is introduced later, it must be sealed unless downstream implementations become an explicit supported product.

## Ownership and responsibility moves

- Model loading moves from public `Embedder` into opaque public `Model`; tokenization, inference, pooling, dimensions, and storage remain private.
- Model reuse moves from caller-managed `Arc<Embedder>` to borrowed `&Model` and `ToolPicker::rebuild`.
- Asset provenance and validation remain in the build and private model modules. They are not resolver API.
- Configuration validation moves from temporal `Config::validate` calls into construction, consuming setters, and checked deserialization.
- Descriptor text derivation moves fully behind picker indexing. Callers provide data, not calibrated embedding instructions.
- Decision result ownership stays with `ToolPicker`; query results borrow descriptors from it.
- Group cardinality belongs to `CandidateGroup`, not documentation around a mutable `Vec`.
- Pair-analysis implementation moves from `picker.rs` into a sibling selected-scope module. `ToolPicker::near_duplicates` remains a thin domain entry point because only the picker owns the vectors needed to answer it.
- Ranking owns validated catalog positions and vector layout. Policy no longer accepts arbitrary indices or malformed row views.
- Each failure unit owns its public opaque error. The facade re-exports only those errors used by public operations.
- Serde wire adapters remain attached to domain types behind the `serde` feature. Invalid wire configuration is rejected during deserialization.

## Invariants

The implementation sweep must make these structural:

1. Every `Config` value is valid. Thresholds are finite and in the supported `0.0..=1.0` policy domain. `top_k` is nonzero.
2. Every picker contains exactly one vector row per catalog entry, with the model's private nonzero dimension.
3. Invalid row width, partial rows, and catalog-to-index count mismatches fail during private index construction. Query and policy code do not recover from impossible internal layouts by abstaining.
4. Every ranked candidate carries a private catalog-position type produced only by ranking over that picker.
5. Every `CandidateGroup` contains at least two valid catalog entries.
6. Every `NearDuplicate` contains two distinct valid catalog entries in catalog order and a finite similarity meeting the inclusive configured threshold.
7. `Outcome::Absent` is a successful policy answer. Query errors mean no answer was produced.
8. `Duplicate` means the leader and one or more same-server twins. `Ambiguous` is every other unresolved near-tie.
9. Decision precedence remains absent, duplicate, bind, ambiguous, including the solo-candidate rule before strict-floor abstention.
10. Floor, margin, duplicate threshold, and solo-floor boundaries remain inclusive as currently implemented.
11. Exact-score hints only refine ordering among candidates already retained by ranking. Read-only, non-destructive, and idempotent positive claims remain the preference order.
12. Catalog order remains the final total-order key. Duplicate identities are allowed and `Catalog::get` returns the first.
13. `shortlist` and `resolve` agree on relevance, including the solo exception. A shortlist may contain one below-strict-floor candidate only when resolution would bind that same solo candidate.
14. Repeated IDs passed to `near_duplicates` are idempotent set membership. The first missing requested ID is reported before comparison. Output is in catalog pair order.
15. Determinism is promised only for the same model bytes, dependency versions, target, execution environment, catalog, configuration, and need. Cross-platform byte-identical vectors and decisions at floating-point boundaries are not promised.
16. No dependency error type, tensor type, tokenizer type, raw model byte representation, raw vector, or reference-counting container crosses the public boundary.

## Compatibility decisions

- Make one intentional breaking API replacement now. The package is `publish = false`, versioned before stability, and all 22 known downstream paths are in the same workspace.
- Do not deprecate old items first. A compatibility layer would keep invalid `Config` values, mutable descriptor fields, owned clone-heavy outcomes, broad errors, and embedding internals reachable.
- Preserve default serde behavior by enabling the new feature by default. Also verify the crate with default features disabled.
- Preserve the existing descriptor and catalog JSON shape.
- Preserve `serde_json::Value` in `ToolDescriptor` as an explicit stable commitment. JSON Schema is fundamental input data even though serde trait implementations are optional.
- Preserve `ToolId::new` as infallible and remove the faulty concatenated key contract. No evidence requires parsing or persistence of the old key.
- Preserve `ToolPicker::build`, `rebuild`, `resolve`, `shortlist`, `near_duplicates`, `len`, `is_empty`, and `config`, with changed result and error types where necessary.
- Preserve empty-catalog success.
- Preserve the current solo-candidate behavior and correct all documentation to describe it.
- Narrow determinism wording rather than introducing quantization. Cross-platform boundary stabilization requires separate measurement and is not necessary to correct the API.

## Old-to-new migration map

### API constructs

- `ToolId::qualified_key()` and `QUALIFIED_KEY_SEPARATOR` -> use `ToolId` directly as a map key, or store `server()` and `name()` as separate fields.
- `ToolAnnotations { read_only, destructive, idempotent }` -> `ToolAnnotations::new().with_read_only(...).with_destructive(...).with_idempotent(...)`.
- `annotations.read_only` -> `annotations.read_only()`, likewise for other hints.
- `descriptor.id` -> `descriptor.id()`.
- `descriptor.description` -> `descriptor.description()`.
- `descriptor.input_schema` -> `descriptor.input_schema()`.
- `descriptor.annotations` -> `descriptor.annotations()`.
- `descriptor.enriched_text()` in downstream tests -> retain the original capability source string in the fixture or construct the expected calibrated phrase locally only when the test specifically targets picker behavior.
- `catalog.tools()` -> `catalog.iter()` or `for tool in &catalog`.
- `Config { field: value, ..Default::default() }` -> `Config::default().with_<field>(value)?`, chaining one checked setter per override.
- `config.validate()` -> remove. Construction already proves validity.
- `ModelId` -> remove. The model is not selectable.
- `Embedder::new()` plus `Arc` -> `Model::load()`.
- `ToolPicker::build_with(embedder, catalog, config)` -> `ToolPicker::build_with_model(&model, catalog, config)`.
- `picker.embedder()` -> retain the `Model` before building, or use `picker.rebuild` for the same policy.
- `picker.tools()` -> `picker.iter()`.
- `picker.vector(index)` and `EMBEDDING_DIMENSIONS` -> remove caller introspection. Test observable ranking, lookup, and pair behavior.
- `Result<T>` -> `std::result::Result<T, SpecificError>` only where a named return type is needed.
- `Error` variant matching -> use the operation-specific error, `QueryError::kind`, `ConfigError::field`, or `SelectionError::missing_id`.
- `Outcome::Bind(tool)` owned use -> borrowed `tool`; clone only the needed `ToolId` or descriptor when it must outlive the picker.
- `Outcome::Duplicate(vec)` and `Ambiguous(vec)` -> `CandidateGroup`; use `iter`, `len`, `first`, `second`, and `get`.
- `shortlist(...): Vec<ToolDescriptor>` -> `Shortlist`; iterate borrowed descriptors.
- `NearDuplicate { first, second, similarity }` -> accessor calls on each borrowed pair.

### All 22 known downstream paths

1. `crates/promptforge-cli/src/main.rs`
   - `ToolPicker::build` remains. Its error type becomes `BuildError`; existing display-only handling remains valid.

2. `crates/promptforge-core/src/error.rs`
   - `ToolAnnotations` fields in `NearDuplicateDiagnostic` may remain typed as `ToolAnnotations`.
   - Construct and inspect annotations through methods.

3. `crates/promptforge-core/src/model.rs`
   - Replace `tool.id` with `tool.id()`.
   - Iterate `CandidateGroup` for duplicate and ambiguous model IDs.
   - Add a wildcard arm for non-exhaustive `Outcome`.
   - `rebuild` errors become `IndexError`; `resolve` errors become `QueryError`.

4. `crates/promptforge-core/src/execute/tests.rs`
   - Replace public-field `NearDuplicate` fixtures with catalog-backed picker results or a test helper at the core layer that accepts diagnostic values directly.
   - Build non-default configuration through checked setters.
   - Replace descriptor field access with accessors.
   - Replace annotation literals with consuming setters.
   - Stop using public `enriched_text`; retain fixture capability text explicitly.

5. `crates/promptforge-dev/src/run.rs`
   - `ToolPicker::build` remains. Existing contextual conversion accepts `BuildError`.

6. `crates/promptforge-mcp-server/src/server/bind.rs`
   - Change constructor return type from the old crate error to `BuildError`.
   - Change rebuild return type to `IndexError`, or map both into the server's own error at this boundary.
   - Update outcome tests for borrowed `Bind` and non-exhaustive matching.

7. `crates/promptforge-core/src/execute.rs`
   - Iterate `NearDuplicates`.
   - Read pair values through `first()`, `second()`, and `similarity()`.
   - `near_duplicates` errors become `SelectionError`.

8. `crates/promptforge-core/src/resolve.rs`
   - `CachedDecision::from_picker` accepts `Result<Outcome<'_>, QueryError>`.
   - Clone only the selected `ToolId` or descriptor needed by the cache.
   - Iterate candidate groups and add the non-exhaustive wildcard.
   - Replace descriptor field access with accessors.

9. `crates/promptforge-mcp-server/src/progress.rs`
   - `ToolPicker::build` remains. No other migration is required.

10. `crates/promptforge-core-tests/src/suite.rs`
    - `ToolPicker::build` remains. No other migration is required.

11. `crates/promptforge-core-tests/src/scenarios.rs`
    - Replace configuration literals with checked setters.
    - Replace descriptor field access with accessors where present.

12. `crates/promptforge-dev/src/tools.rs`
    - Catalog and descriptor constructors remain.
    - Update outcome tests to borrowed values and include a wildcard arm.

13. `crates/promptforge-cli/src/tools.rs`
    - Catalog and descriptor constructors remain.
    - Update outcome tests to borrowed values and include a wildcard arm.

14. `crates/promptforge-cli/Cargo.toml`
    - No dependency declaration change is required because `serde` remains a default feature.

15. `crates/promptforge-core/Cargo.toml`
    - No dependency declaration change is required because `serde` remains a default feature.

16. `crates/promptforge-core-tests/Cargo.toml`
    - No dependency declaration change is required because `serde` remains a default feature.

17. `crates/promptforge-dev/Cargo.toml`
    - No dependency declaration change is required because `serde` remains a default feature.

18. `crates/promptforge-mcp-server/src/retrieval/fixture.rs`
    - Replace `Arc<Embedder>` fixture state with cloneable `Model`.
    - Load it through `Model::load`.

19. `crates/promptforge-mcp-server/src/retrieval/tests.rs`
    - Replace picker test setup using the old embedder with `Model` where shared model reuse is required.
    - Picker behavior calls otherwise remain unchanged.

20. `crates/promptforge-mcp-server/src/retrieval/index.rs`
    - Change test-only `build_with` to accept `&Model`.
    - Build configuration with `Config::default().with_similarity_floor(value)?`.
    - Call `ToolPicker::build_with_model`.

21. `crates/promptforge-mcp-server/Cargo.toml`
    - No dependency declaration change is required because `serde` remains a default feature.

22. `crates/promptforge-mcp-server/src/retrieval.rs`
    - Change test-only `install_with` from `Arc<Embedder>` to `&Model`.

No downstream path requires raw assets, dimensions, raw vectors, `embed_all`, `qualified_key`, or direct model selection.

## Required tests

### Public API and compile-time contracts

- Compile examples for every retained public type and substantive method.
- Assert `Send + Sync + 'static` for `Model`, `ToolPicker`, and every public error.
- Assert the value traits promised by identity, descriptor, catalog, configuration, outcome, and result-view types.
- Compile external-style matches proving `Outcome` requires a wildcard.
- Compile external-style use proving public structs cannot be exhaustively constructed or destructured.
- Test default features and disabled default features. Under `serde`, test every retained wire shape.

### Identity, descriptor, and catalog

- Prove two IDs containing any control or printable delimiter remain distinct as structural values.
- Remove concatenated-key tests.
- Test annotation builders and accessors for absent, true, and false values.
- Test descriptor accessors and unchanged JSON field aliases.
- Test byte-exact internal enriched text, including sorted parameters, malformed schemas, empty descriptions, underscores, and doubled punctuation.
- Test `Catalog` owned, shared, and mutable iteration, order preservation, duplicate identities, first-match `get`, `From<Vec<_>>`, and `FromIterator`.

### Configuration

- Test every threshold at `0.0`, `1.0`, just outside each bound, NaN, and both infinities.
- Test `top_k` zero rejection and positive acceptance.
- Test that a failed consuming setter leaves the original caller-owned value available through normal Rust move discipline only when the caller cloned it explicitly.
- Test checked deserialization rejects every invalid state and fills omitted fields from defaults.
- Test accessor values and exact default bits.

### Model, index, and errors

- Test one model can build several pickers without reloading.
- Test `rebuild` preserves the exact non-default configuration and model allocation.
- Test model and picker stay `Send + Sync`.
- Test private index construction rejects zero dimensions, partial rows, non-finite rows, wrong row counts, and catalog/index mismatches.
- Replace silent malformed-layout ranking tests with invariant-rejection tests.
- Test each error's lowercase display, source chain, classification or accessor, and auto traits.
- Verify dependency source types remain absent from rustdoc signatures.

### Resolution and result views

- Preserve end-to-end bind, duplicate, ambiguous, absent, determinism, hint-ordering, same-server classification, empty-catalog, and inclusive-boundary tests.
- Add direct solo-shortlist tests: one leader between floors returns one; two peers between floors return none; equality at each floor is covered.
- Assert `CandidateGroup::len() >= 2` for every constructible path.
- Assert callers cannot construct invalid groups or pairs.
- Assert outcomes and shortlists borrow the picker's exact descriptor addresses and perform no descriptor clone.
- Assert zero shortlist limit returns immediately and does not invoke embedding through an internal test seam.
- Assert all query methods preserve catalog order as the final tie-break.

### Near-duplicate analysis

- Preserve first-missing-ID order, repeated-ID idempotence, catalog pair order, server-independent comparison, inclusive threshold, and no-embedding behavior.
- Test dense selected sets without descriptor cloning.
- Test selection membership uses one validated set rather than repeated linear scans.
- Test every emitted pair contains distinct valid entries and finite similarity.

### Documentation and build behavior

- Update crate docs to focus on resolver use and the opaque reusable model.
- State the solo-candidate rule once as the canonical policy contract and link method docs to it.
- State the narrowed determinism guarantee everywhere.
- Keep build acquisition and asset provenance in build documentation, not public resolver rustdoc.
- Add docs.rs feature metadata for the `serde` feature.
- Preserve the single integration-test binary.
- Split oversized policy and picker test modules by cohesive concern without creating additional integration-test binaries.

## Finding disposition

Every API-related finding is accepted unless explicitly narrowed below. Findings that concern implementation, build integrity, formatting, or test organization remain required implementation work but do not add public API.

### Manifest and build findings

- `PF-TP-MANIFEST-001`: accepted, implementation sweep sets the private package version to `0.0.0`.
- `PF-TP-MANIFEST-002`: accepted for package documentation, but repository metadata belongs at workspace scope and must not be improvised in this crate sweep.
- `PF-TP-MANIFEST-003`: accepted as build-system work. Move acquisition out of ordinary Cargo build or provision immutable local input. No public API is added.
- `PF-TP-MANIFEST-004`: accepted as workspace CI work, not this crate's public API.
- Build findings `1` through `4`: accepted as implementation work. Hermetic inputs, strong output digests, atomic staging, contextual source chains, and directly testable build helpers remain private.

### Asset findings

- `PF-TP-ASSETS-001` and `PF-TP-ASSETS-002`: resolved by making the module and payloads private.
- `PF-TP-ASSETS-003` and `PF-TP-ASSETS-005`: accepted as private build-integrity and test work.
- `PF-TP-ASSETS-004`: resolved with one private generated provenance record owned by the asset workflow.

### Catalog findings

- `F-01`: resolved by removing concatenated keys and the separator while retaining structural `ToolId`.
- `F-02`: resolved with private descriptor fields and borrowed or copied accessors.
- `F-03`: resolved with non-exhaustive public domain structs.
- `F-04`: resolved by feature-gating serde. `serde_json::Value` is retained intentionally as core schema input.
- `F-05`: resolved with `iter_mut` and mutable `IntoIterator`.
- `F-06` and `F-07`: accepted. Add doctests to retained items and correct summaries.
- `F-08`: accepted as private allocation cleanup only after byte-exact tests preserve calibration.
- `F-09`: resolved by deriving `Eq` for descriptor and catalog.

### Configuration findings

- `CFG-001` and `CFG-002`: resolved with private fields, checked consuming setters, `NonZeroUsize`, checked deserialization, and non-exhaustive shape.
- `CFG-003`: resolved with the additive default `serde` feature.
- `CFG-004`: resolved by naming `0.0..=1.0` the supported calibrated policy domain.
- `CFG-005` and `CFG-006`: accepted with doctests and crate-root integration coverage for defaults, setters, accessors, and serde behavior.

### Embedding findings

- `PF-TP-EMBED-001`: resolved through opaque sourced errors.
- `PF-TP-EMBED-002` and `PF-TP-EMBED-007`: accepted as private implementation performance work, guarded by correctness tests and measurement.
- `PF-TP-EMBED-003`: resolved by narrowing determinism claims.
- `PF-TP-EMBED-004`: superseded. Concrete `Embedder` is removed; opaque `Model` is non-exhaustive.
- `PF-TP-EMBED-005` and `PF-TP-EMBED-006`: applied to retained `Model::load`; removed embedding methods need no public annotations or doctests.
- `PF-TP-EMBED-008`: resolved by removing `embed_all` and all raw embedding API.
- `PF-TP-EMBED-009`: accepted as private test correction. Pin the complete vector digest or narrow the claim.

### Error findings

- `PF-TP-ERROR-001`, `PF-TP-ERROR-002`, and `PF-TP-ERROR-004`: resolved by narrow opaque sourced errors with stable classifications or accessors.
- `PF-TP-ERROR-003`: accepted for every new error display.
- `PF-TP-ERROR-005`: resolved through comparable public kinds and structured accessors. Opaque source-bearing wrappers themselves are not forced to implement equality.
- `PF-TP-ERROR-006` and `PF-TP-ERROR-010`: accepted with doctests, focused same-module tests, source-chain tests, and auto-trait assertions.
- `PF-TP-ERROR-007`: resolved by documenting policy abstention as successful while query embedding remains fallible.
- `PF-TP-ERROR-008`: resolved with accurate supported-domain terminology.
- `PF-TP-ERROR-009`: resolved by removing the public alias entirely.

### Facade findings

- `PF-TP-LIB-001`: resolved by documenting a duplicate group of at least two.
- `PF-TP-LIB-002` and `PF-TP-LIB-003`: resolved by private assets and opaque `Model`.
- `PF-TP-LIB-004` through `PF-TP-LIB-006`: resolved by validated configuration, private data fields, invariant-bearing result types, and non-exhaustive shapes.
- `PF-TP-LIB-007`: resolved by serde gating while retaining JSON Schema as an explicit core type.
- `PF-TP-LIB-008` and `PF-TP-LIB-009`: resolved by the narrow opaque error design.
- `PF-TP-LIB-010`: resolved by borrowing outcomes, shortlists, and pairs.
- `PF-TP-LIB-011`: resolved by removing public parameter-name derivation. Internal code may use a private iterator.
- `PF-TP-LIB-012`: accepted for every retained public item and docs.rs feature metadata.
- `PF-TP-LIB-H01`: resolved by removing the alias.
- `PF-TP-LIB-H02`: resolved by moving backend lifecycle and build details out of facade guidance.

### Picker findings

- `PF-TP-PICKER-001`: resolved by making the solo exception the explicit shared contract and adding boundary tests.
- `PF-TP-PICKER-002`: resolved with non-exhaustive `ToolPicker` and private, non-exhaustive result types.
- `PF-TP-PICKER-003`: resolved with `BuildError`, `IndexError`, `QueryError`, and `SelectionError`.
- `PF-TP-PICKER-004`: resolved with opaque `Model` and no raw vector or model getter.
- `PF-TP-PICKER-005`: accepted as private implementation work using one validated hash set while preserving first-missing order.
- `PF-TP-PICKER-006`: resolved by borrowing pair results.
- `PF-TP-PICKER-007`: accepted. Move selected-scope analysis to its own sibling module.
- `PF-TP-PICKER-008`: accepted for retained items; removed accessors need no doctests.

### Picker test findings

- `PF-TP-PICKER-TESTS-001`: accepted. Split cohesive child modules under the existing unit-test tree.
- `PF-TP-PICKER-TESTS-002` and `PF-TP-PICKER-TESTS-003`: accepted as mechanical rulebook conformance.
- `PF-TP-PICKER-TESTS-004`: resolved through one shared `Model` fixture and `build_with_model`.
- `PF-TP-PICKER-TESTS-005`: accepted. Replace the overclaim with complete solo and strict-floor boundary coverage.

### Policy findings

- `PF-POL-001`: resolved with non-exhaustive `Outcome`.
- `PF-POL-002`: resolved with private `CandidateGroup` construction and immutable iteration.
- `PF-POL-003`: resolved by preserving and documenting the solo exception consistently.
- `PF-POL-004`: accepted with an outcome doctest including a wildcard.
- `PF-POL-005`: resolved with borrowing outcomes and shortlists.
- `PF-POL-006`: accepted with direct policy boundary tests.
- `PF-POL-007`: accepted by moving tests to cohesive child modules.

### Ranking findings

- `RANK-001`: resolved with a validated private index representation that rejects malformed layouts at construction.
- `RANK-002`: accepted for every retained private constructor and pure transform.
- `RANK-003`: resolved with private fields and a private validated catalog-position type produced only by ranking.

### Integration-test findings

- `PFTP-BEHAVIOR-001`: accepted as mechanical import ordering.
- `PF-TP-PA-001`: accepted. Assert exact shortlist identity and exclusion.
- `PF-TP-PA-002`: accepted. Rebuild from a non-default validated configuration and assert preservation.

## One-sweep implementation boundary

The design is implementable in one crate-wide sweep:

1. Introduce private validated index and position types.
2. Replace configuration representation and errors.
3. Introduce opaque `Model` and narrow sourced errors.
4. Privatize descriptors and annotations, then add accessors and builders.
5. Replace owned query results with borrowing result views.
6. Narrow the facade and add the serde feature.
7. Update crate docs and tests.
8. Update only the 22 listed downstream paths.

These changes must land together because each intermediate state would either fail to compile or temporarily preserve invalid public contracts. No proposal requires a new crate, caller-supplied trait implementation, persistence format migration, model recalibration, asynchronous API, or change to the resolution algorithm.

## Self-check

- The proposed surface can be implemented using the current catalog, model, policy, and ranking code in one coordinated sweep.
- Every changed public result has a concrete Rust signature and a downstream migration.
- Every removed item has either no downstream use or an explicit replacement.
- All 22 known downstream paths are dispositioned.
- All API-related findings are dispositioned, and non-API findings are identified as private implementation, build, documentation, or test work.
- Section 6 requirements are applied: future-proofed public shapes, private invariant-bearing fields, named public iterators, `From` rather than `Into`, no public return-position `impl Trait`, optional serde derives, opaque dependency-free errors, `#[must_use]`, and no unnecessary trait.
- No compatibility shim reintroduces the invalid states or backend leakage being removed.

*2026-08-09 15:53 - GPT-5.6 Sol*
