# promptforge-tool-picker: a library that resolves a plain-English capability need to one tool from a catalog, or refuses to guess

## Executive summary

This crate answers one question for a caller: given a set of tools described only in prose, which one does this sentence mean? It answers with a decision, not a score - a single bound tool, a fault report naming one server's own copy-pasted twins, a shortlist of candidates it could not separate, or an abstention. Abstention is an answer and not an error, and that distinction is the load-bearing part of the contract: a caller must be able to tell "no tool fits" from "the engine could not run".

The engine is self-contained by construction. The catalog of tool descriptors is the sole input, the embedding model is compiled into the library, and nothing is read from disk or the network at run time. It carries no scripting language, no tool-calling protocol, and no client: mapping a chosen descriptor onto something callable is the caller's job. The same catalog, need, and configuration always produce the same answer, on the same machine and across processes, because every ordering in the pipeline is a total order and every text is embedded on its own rather than in a batch.

Two numbers set the caller's expectations. The default similarity floor of 0.825 was calibrated to hold false bindings at or under five percent, and it earns that budget by binding only near-restatements of a tool's own description: a need phrased as a person actually speaks ("what will the weather be like in Paris this week") scores 0.651 against a weather tool that a restatement ("get the weather forecast for a city") carries to 0.865 and a binding. So `resolve` is the entry point for author-register capability descriptions, and conversational traffic should expect abstention far more often; `shortlist` with a lowered floor is the honest entry point for that traffic. Loading the encoder is the one expensive call in the crate's life cycle, and it is paid once: the weights load once per process and an engine is built per catalog over them, through `build_with` for a fresh catalog and `rebuild` for a changed one. A caller whose catalog changes at run time - a watched directory, a reconnected server - therefore pays only one forward pass per tool per change, rather than tens of megabytes of weights again.

## The key design choices

1. **The catalog is the only input contract, and it is prose plus identity - nothing callable.** A `ToolDescriptor` carries a `(server, name)` identity, a description as its author wrote it, a JSON Schema for its arguments, and optional behavioural hints. It carries no endpoint, no handle, and no closure. This is what keeps the crate free of a protocol: a catalog producer can be a test, a config file, a future protocol client, or a hand-written literal, and none of them are visible to the resolution engine. Reversing this - taking `dyn Tool` or a protocol type instead - would pull a transport dependency and its error types into every consumer, so the boundary is drawn here deliberately.

2. **Identity is the `(server, name)` pair, kept as a pair.** A tool name is unique only within its server, so the server is part of the identity rather than context around it. `ToolId` compares and hashes structurally over both parts and never concatenates them, so a server or tool name containing whatever delimiter a caller might have picked cannot collide with another identity. Where a single-string key is genuinely needed, `qualified_key` joins the parts with the ASCII unit separator, a C0 control character no protocol or filesystem accepts inside a name - which is the whole reason it was chosen over a readable `/`, `.`, or `:`.

3. **Four outcomes, and one of them is a fault report rather than an answer.** The public `Outcome` is `Bind`, `Duplicate`, `Ambiguous`, `Absent`. `Duplicate` exists to fail loudly: one server publishing two tools that no query can tell apart is a defect in that server's catalog, and the operator is either the caller or someone the caller can reach. `Ambiguous` is the residual bucket for every near-tie the margin could not separate, most typically one capability republished by two servers, which is the ordinary consequence of pointing one engine at overlapping catalogs. Nothing is wrong there, so nothing fails; the engine hands back the candidates.

   That outcome was originally named `ForeignAmbiguous`, and the rename to `Ambiguous` is a contract change, not a cosmetic one: nothing in a descriptor marks a server as the caller's own rather than imported, so the definition the data actually supports is same-server twins are a `Duplicate` and every other unseparated group is `Ambiguous`, including two merely similar tools on one server. The old name asserted a provenance claim that is false in cases the tests exercise, and a wrong name in a public enum is a wrong contract.

4. **A twin is a property of two tools, never of a query.** `duplicate_threshold` is compared against the cosine similarity between the two tools' own stored embeddings. Two verbatim copies remain copies whatever is asked of them, while two quite different tools can both score highly against one need - so reading the threshold as a query-score level would report the wrong pairs and miss the right ones. This also settles a question that read as open while the threshold was mistaken for a score level: because the duplicate threshold measures tool-to-tool and the similarity floor measures need-to-tool, neither bounds the other, and configuration validation deliberately does not order them.

5. **The thresholds apply in a fixed precedence, and the order is itself the decision.** Absent, then Duplicate, then Bind, then Ambiguous. The floor comes first because if nothing in the catalog fits the need, whether the misses resemble each other is beside the point. Duplicate comes before the margin test on purpose: the outcome exists to fail loudly, and a server's copy-pasted pair is a fault whether or not a narrow configured margin happened to split the two scores this particular need produced. Every comparison is inclusive, so each configured number reads as "this value is enough".

| Order | Condition | Outcome | What is reported |
| --- | --- | --- | --- |
| 1 | top score `< similarity_floor` | `Absent` | nothing |
| 2 | a candidate shares the leader's server and their two stored vectors are `>= duplicate_threshold` alike | `Duplicate` | the leader and its same-server twins, best first |
| 3 | leader minus runner-up `>= margin`, or no runner-up clears the floor | `Bind` | the one tool |
| 4 | otherwise | `Ambiguous` | the leaders the margin could not separate, best first |

6. **Abstention is `Ok(Absent)`; `Err` means the engine could not run.** `resolve` returns `Result<Outcome>` rather than a bare `Outcome`, because embedding the need can fail (tokenization, a forward pass) and collapsing that into "nothing matched" would discard a real fault. The inverse mistake is just as costly: a caller treating `Absent` as an error would be treating the engine's most careful answer as a bug. The two arms are therefore documented as answering different questions, and no error variant ever means "no match".

   The error type carrying that arm never exposes a dependency's error type: each variant carries a `detail` string instead, and both the enum and its variants are `#[non_exhaustive]`. A new release of the tensor library or the tokenizer therefore cannot become a breaking change to this crate's public surface, and a new failure mode or a new field on an existing one is additive. Every variant names the offending value, because a configuration rejected without saying which field was wrong is a worse failure than the misbehaviour it prevented.

7. **The public surface names its expensive operation, and offers a way not to pay it twice.** Ownership is explicit: a constructor consumes the catalog, the value is immutable afterwards, and a changed catalog calls for a new engine rather than a mutation. What is costly is loading the model, not indexing, so the encoder is a shared value a caller may hold: `build_with` takes one and is the single path every engine is indexed by, `build` loads one and calls it, and `rebuild` calls it with this engine's own encoder and configuration. A caller whose catalog changes - a watched directory, a reconnected server - therefore pays the weights once for the life of the process and one forward pass per tool per change. The alternative, a `build` that always loads, made a rebuild cost seconds of CPU on every save; the cost of this one is that the encoder's shape (`Arc<Embedder>`) is now part of the public contract.

```rust
impl ToolPicker {
    pub fn build(catalog: Catalog, config: Config) -> Result<ToolPicker>; // loads the model, then build_with
    pub fn build_with(embedder: Arc<Embedder>, catalog: Catalog, config: Config) -> Result<ToolPicker>;
    pub fn rebuild(&self, catalog: Catalog) -> Result<ToolPicker>;       // same encoder, same configuration
    pub fn embedder(&self) -> &Arc<Embedder>;
    pub fn resolve(&self, need: &str) -> Result<Outcome>;
    pub fn shortlist(&self, need: &str, k: usize) -> Result<Vec<ToolDescriptor>>;
    pub fn tools(&self) -> &[ToolDescriptor];
}
```

   Configuration is validated in `build_with`, so no engine is indexed under a configuration `build` would have refused, and `build` validates once more before loading anything, so a rejected threshold still costs no weights. A rebuild carries the configuration over rather than taking a new one, because a rebuild is a change of data and not of policy; changing a threshold is `build_with` over `embedder()`.

8. **`shortlist` reports where `resolve` decides, and the two can never contradict each other.** A shortlist applies the similarity floor and nothing else - no margin, no twin check - so for any `k` above zero it is empty in exactly the cases `resolve` abstains. Returning the raw top `k` whatever they scored was the alternative, and it lost because it would let one entry point abstain on a need while the other offered three tools for it, and a caller feeding those into a prompt would be offering tools the engine had already judged irrelevant. A caller who wants near-misses lowers the floor, stating that intent in configuration rather than hiding it in a choice of method.

9. **`k` on a shortlist is the caller's request; `top_k` in the configuration is something else.** The two are not clamped against each other, because they answer different questions: `top_k` bounds the group an `Ambiguous` or `Duplicate` outcome reports, while `k` is what this caller asked for on this call. A `k` past the end of the catalog returns what exists, never a padded list; a `k` of zero returns nothing. Resolution independently always ranks at least two candidates however small `top_k` is, since every ambiguity it can report is a statement about a leader and a runner-up, and a `top_k` of one would otherwise turn a server's twin pair into a silent binding.

10. **Configuration is plain public data with a re-runnable `validate`, not a checked constructor.** A caller adjusts one threshold with struct-update syntax over `Config::default` and leaves the rest at their justified values. Because the fields are public, a constructor's check could be bypassed by the next field assignment or by deserialization, so validation is a method that `build` calls and a caller may call again. It rejects a threshold outside the cosine range - including NaN, which compares false against every bound and would silently disable the check - and a `top_k` of zero, which would leave every need with the same answer.

11. **Three of the four defaults are measured; the fourth is labelled unmeasured, in the docs a caller reads.** `similarity_floor` is 0.825, the calibrated point holding false bindings at or under five percent (a one percent budget is 0.863, a ten percent budget 0.805). `duplicate_threshold` is 0.98, above which a pair was found to be near-verbatim republication rather than mere neighbours. `top_k` is 3, because the correct tool was in the top three about ninety percent of the time against roughly seventy-six percent for top one - that gap is the entire reason a shortlist exists. `margin` is 0.05 and is explicitly a starting point to be tuned against a real catalog. Publishing which numbers are evidence and which is a guess is part of the contract; a caller who cannot tell will trust both equally.

12. **The model is compiled into the library, so there is no weights path in the configuration and no network at run time.** The build fetches `BAAI/bge-small-en-v1.5` from a pinned commit (a commit, not a branch, so upstream cannot silently change what ships), verifies each file against a hardcoded digest, downcasts the weights to fp16 to halve what every binary carries, and the library embeds the bytes. A caller therefore configures no path, ships no sidecar file, and cannot be broken by a missing model at run time; the failure mode moves to the build, where the first build needs the Hugging Face Hub and later builds read its cache. Reversing this is expensive in the direction of adding a path - it would reintroduce a run-time failure mode and a deployment step the design exists to remove - but cheap in the direction of precision, since fp16 is a storage decision only and the loader chooses its own compute dtype (f32 today, because CPU coverage for f16 is uneven).

13. **Exactly one model is selectable, expressed as a `#[non_exhaustive]` enum rather than a string.** Only one model's weights are compiled in, so a string or free-form identifier would let a caller name a model the binary cannot satisfy and turn a build-time fact into a run-time failure. The enum shape means embedding a second model later adds a variant rather than reshaping the configuration, and `#[non_exhaustive]` makes that addition non-breaking for callers who already carry a wildcard arm. Mean pooling and a second model were deliberately deferred rather than half-exposed.

14. **Determinism is a published guarantee, and it is bought by total orders and unbatched embedding.** Every ranking sorts by score descending, then - for scores that tie exactly, which happens whenever a tool is republished verbatim - by behavioural hints, then by catalog position, which is the one key guaranteed unique because a catalog is deliberately not deduplicated. A non-finite score is ordered below every real one so the order stays total even for a value the crate cannot currently produce. Embedding runs one text at a time rather than in a padded batch, so a vector never depends on its neighbours to within floating-point noise. Indexing is a once-per-build cost, and reproducibility is worth more there than batch throughput.

15. **Behavioural hints break exactly-tied scores and nothing else, and silence is not a claim.** Where MCP-style `readOnlyHint`, `destructiveHint`, and `idempotentHint` are present, a positive claim promotes: read-only first, then non-destructive, then idempotent. An absent hint is read as "no claim" rather than as a value to compare, because consulting a hint only when both candidates carry it would make the comparison intransitive and cost exactly the determinism above - and reading silence as the weaker claim is the cautious reading anyway. Hints never overturn a decision the scores made: they cannot promote a near-tie into a binding, cannot rescue a `Duplicate`, and cannot recall a candidate that fell outside the ranked window.

## What the caller writes: the catalog as JSON

The wire shape is part of the contract because a catalog is routinely authored or exported as data rather than constructed in Rust. A catalog is an array of descriptors; identity is flat; the schema field accepts its MCP spelling as well as the snake-case one; every optional field defaults.

```json
[
  {
    "server": "files",
    "name": "read_file",
    "description": "Read the contents of a file from the local disk",
    "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } },
    "annotations": { "readOnlyHint": true }
  }
]
```

The same permissiveness applies to configuration: every field is optional in JSON and an absent one takes its documented default, so a caller can override the floor alone without restating the rest.

## The embedded text is a contract, not an internal detail

What gets embedded for a tool is its name with underscores opened into spaces, then its description verbatim, then `parameters: ` followed by its parameter names, with empty parts dropped and the parts joined by a period and a space. Parameter names are sorted, because a JSON object's original key order is not recoverable after parsing and sorted order is the one ordering reproducible across parsers and runs.

This shape is published rather than hidden for one reason: the calibrated thresholds are only meaningful against vectors of text in exactly this shape. Changing the punctuation, the ordering, or the `parameters: ` prefix moves every similarity score and silently invalidates every default in the configuration - including the doubled period that appears when a description already ends in one, which the calibration included and which is therefore preserved rather than tidied away. Parameters are included at all because a name and a one-line description often under-describe a tool while its argument names say concretely what it operates on, and the name is de-underscored because the model reads a snake_case identifier as one opaque token instead of as its words.

Two properties of the encoding follow from the same reasoning. Pooling is the first token's hidden state, which is what this model was trained for; mean pooling over the same weights produces vectors that look entirely plausible - unit length, believable spread - while ranking measurably worse, which is a silent failure and so is stated rather than left to be read out of code. And a need takes the identical path with no instruction prefix: the model publishes a query prefix for asymmetric retrieval of long documents, tool resolution is symmetric one line against one line, and the thresholds were measured with no prefix.

## Named tensions, with the measurements that expose them

The duplicate threshold is sensitive to description **length**, not only to how alike two tools are. Two tools sharing a description word for word, with names differing by one word, measure 0.983 when the description is a paragraph and only 0.960 when it is a single line, because the name difference is a large fraction of a short text. A genuinely paraphrased same-server pair measured 0.811. So the falsifier is sharper than first recorded: it is not only paraphrases that slip past 0.98 and bind silently, it is verbatim copies under short descriptions. The crate's own fixture exhibits both sides of this, which is why it is documented as a tension rather than treated as a bug: raising sensitivity here trades directly against reporting ordinary neighbours as faults.

The floor is stricter than it reads, and that is the design working as calibrated rather than a defect. The consequence for callers is stated in the executive summary and repeated here because it changes which entry point a caller should reach for: `resolve` suits author-register capability text, and end-user phrasing belongs on `shortlist` with a floor the caller has chosen.

Loading the model twice in one process is measurably slower than once, since the loader materializes roughly 133MB of f32 weights, and proving determinism across two builds costs about 6.6 seconds. That is what `build_with` and `rebuild` exist to stop paying: the `Embedder` is public, `Send + Sync`, and shared behind an `Arc`, so a caller holds one and builds an engine per catalog. Reusing an encoder cannot change an answer - two engines over one encoder embed identical text to identical vectors, which the tests assert - so the saving is free of a correctness trade-off, and the test that a rebuild skips the loading path is written as an identity check on the shared encoder rather than as a wall-clock comparison, which would measure the machine.

## Decide by use

- A second embedded model, and with it mean pooling, which the model identifier's shape already accommodates as an added variant.
- A persistent vector cache. None exists: an index is a process-lifetime thing, and a cache keyed on catalog content would have to be invalidated by the same content hash it is keyed on, which costs more correctness risk than embedding a realistic catalog costs time.
- Tuning `margin` against a real catalog, which is the one default carrying no measurement behind it.

*2026-08-02 - claude-opus-5, revised 2026-08-03 for the shared-encoder constructors*
