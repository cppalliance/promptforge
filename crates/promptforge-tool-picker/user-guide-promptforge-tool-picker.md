# Semantic Tool Binding in PromptForge

PromptForge decides which tools a prompt can use. You describe each tool in plain prose. PromptForge matches the intent of a prompt against those descriptions with a small embedding model that runs on your machine. There is no LLM call. There is no network access at runtime. There is no keyword matching. The right tools reach the model. The wrong ones stay out. This guide shows you how to declare tools, tune the match, and read the results.

## What Tool Binding Does

You write a need in plain prose, for example "read a file from disk". PromptForge resolves that need to the tool that performs it. The match uses sentence embeddings. The whole embedding model is compiled into the library. You configure no model path. You ship no weights file. You make no runtime network call.

You describe your tools in a catalog. Each entry in the catalog is a tool descriptor. A descriptor pairs a tool identity with a natural-language description and a JSON Schema for its arguments. The identity has two parts: a server name and a tool name. The pair identifies a tool without ambiguity. Two tools with the same name on different servers never collide. Delimiter characters inside either part stay unambiguous.

You build a picker over the catalog. You then ask the picker which tool a given need refers to. You build the picker once. You ask it about as many needs as you like.

Every query returns one of four outcomes. The picker can bind one tool. It can report a group of duplicate tools published by one server. It can return a shortlist of candidates it could not separate. It can abstain when nothing fits.

## The Four Outcomes of a Match

Each need you resolve ends in exactly one outcome. You handle each case in your own code.

**Bound.** One tool cleared the similarity floor (the minimum score a candidate must reach) and left the runner-up behind by at least the configured margin. You call the chosen tool immediately.

**Duplicate.** One server publishes two tools that are near-verbatim copies of each other. This is a catalog fault. The picker fails loudly and names the pair. The group always holds at least two members, in ranked order. You fix the catalog.

**Ambiguous.** Two or more tools sit within the decision margin and the picker cannot separate them. This happens most often when one tool is republished across two servers. The group always holds at least two members, in ranked order. You choose for yourself, or you sharpen the descriptions.

**Absent.** Nothing in the catalog matched the need well enough to offer. An abstention is a successful answer, not an error. You can tell an abstention apart from an engine failure. An abstention means the policy answered. An error means the engine could not run.

One rule sits between binding and abstention. The solo floor is a second, lower score bar. A lone candidate that scores at or above the solo floor, but below the similarity floor, still binds when no runner-up reaches the solo floor. There is nothing to confuse it with. Two such candidates cause an abstention instead. Section "Setting Match Thresholds" gives the defaults for both floors.

The same tool republished on one server is a duplicate. The same tool republished across two servers is ambiguous. The distinction is the server name in the identity.

## Declaring Tools for Matching

Tools enter the catalog as descriptors. You write each descriptor. A descriptor carries four things: the server name, the tool name, your description, and a JSON Schema for the arguments. A descriptor carries nothing that could invoke the tool. Mapping a resolved descriptor onto something callable is your job.

The engine matches against three parts of each tool: the tool name with underscores removed, the description, and the parameter names in sorted order. Your wording directly steers the match. Parameter names in the schema affect semantic matching.

A minimal descriptor in JSON looks like this:

````json
{
  "server": "files",
  "name": "read_file",
  "description": "Read a file from disk",
  "inputSchema": {
    "properties": {
      "path": { "type": "string" }
    }
  }
}
````

The schema field accepts both `input_schema` and the MCP spelling `inputSchema`. Optional fields can be omitted. A missing schema becomes null on load. Missing annotations become the default, with every hint absent.

You can attach MCP behavioral hints to a descriptor: read-only, destructive, idempotent. Each hint is optional and absent by default. An absent hint never changes a ranking. Hints act only as a tie-break between candidates with identical scores. A positive read-only claim wins first, then a non-destructive claim, then an idempotent claim.

````json
{
  "server": "files",
  "name": "read_file",
  "description": "Read a file from disk",
  "inputSchema": {
    "properties": {
      "path": { "type": "string" }
    }
  },
  "annotations": { "readOnlyHint": true }
}
````

You assemble descriptors into a catalog. The catalog is the sole input contract of the picker. Order is preserved. Duplicate identities are accepted, not refused. Two tools claiming the same identity is a result the engine reports, not an input it rejects. You can look up the first descriptor that matches a given identity. You can ask the catalog its size. You can iterate its descriptors in the order you gave them.

A catalog serializes as a plain JSON array of descriptors. It round-trips losslessly. You can commit a catalog as data and load it back.

## Setting Match Thresholds

Five thresholds steer which of the four outcomes a need receives. The defaults are pre-calibrated. They were measured against a real catalog. You can resolve tools without tuning anything.

| Key | Default | Effect |
|---|---|---|
| `similarity_floor` | 0.825 | The minimum cosine similarity a candidate must reach to be considered at all. Raise it to bind less often. Lower it to consider weaker matches. |
| `margin` | 0.05 | The score gap the top candidate must clear the runner-up by before the engine binds. Raise it to demand a clearer winner. Set it to zero to let annotation hints choose between tied tools. |
| `duplicate_threshold` | 0.98 | The similarity at or above which two tools are treated as twins. The comparison uses the tools' own embeddings, not the query. |
| `solo_floor` | 0.5 | The minimum score at which a lone candidate still binds. Set it equal to `similarity_floor` to disable the solo rule. |
| `top_k` | 3 | How many candidates an ambiguous or duplicate outcome reports back. Must be nonzero. |

You tune the thresholds with checked setters that start from the defaults. Each threshold must be finite and within 0.0..=1.0. `top_k` must be nonzero. An out-of-domain value produces a configuration error that names the rejected field. Every stored configuration is always valid. There is no separate validation step to remember.

You can persist or transmit a configuration as JSON. Every field is optional. Absent fields are filled from the calibrated defaults.

````json
{
  "similarity_floor": 0.85,
  "top_k": 5
}
````

Invalid values in a configuration file are rejected, not silently accepted. A document with `similarity_floor` set to 2.0 fails. A document with `top_k` set to 0 fails.

Every threshold boundary is inclusive. A score exactly at the floor is considered. A gap exactly equal to the margin binds. A pair exactly at the duplicate threshold is a twin.

## Reading the Shortlist

You can ask for a shortlist instead of a decision. Use this when you would rather choose for yourself, for example when an end user picks the tool.

A shortlist is the best N tools for a need, ranked best first. You choose the cap. A shortlist lists candidates above the similarity floor without making a final decision. The solo-candidate exception is preserved: a lone leader between the solo floor and the strict floor is offered. A limit of zero returns an empty shortlist without paying for an embedding. A shortlist never drops either side of a tie. Even with `top_k` set to 1, a tie yields two entries.

Resolve and shortlist never contradict each other. If resolve abstains, shortlist returns nothing. If resolve binds a tool, shortlist offers exactly that tool.

You can inspect an ambiguous or duplicate group the same way. You can ask its length. You can take the first or second candidate. You can index into it. You can iterate it.

You can also detect near-duplicate pairs inside a chosen set of tool identities. The analysis compares the selected tools' own embeddings against the duplicate threshold. It runs independent of any query. Every requested identity is validated before any pair is compared. A missing identity fails the whole analysis and names the first absent one. Repeated identities collapse to set membership. Each detected pair exposes the two tools and their exact cosine similarity score. Pairs come back in deterministic catalog order, regardless of the order you requested.

Use these views to act. Tune descriptions. Split overloaded tools. Delete copy-pasted duplicates. Resolve ambiguity before it reaches the model.

## Writing Better Tool Descriptions

The engine reads the de-underscored tool name, the description, and the sorted parameter names. Write all three for the match.

- State the action in the description. A need that restates one tool's capability binds that tool. "Read a file from disk" binds a need phrased as "read the contents of a file from disk".
- Name parameters with meaningful words. Parameter names are part of the matched text. A schema full of `arg1` and `arg2` tells the engine nothing.
- Keep sibling tools distinct. Two tools that differ only by a copy-pasted name will surface as duplicates. Two tools that genuinely cover the same ground will surface as ambiguous. Both outcomes tell you to sharpen the wording.
- Attach behavioral hints to otherwise identical tools. On an exact score tie, a positive read-only claim wins first, then a non-destructive claim, then an idempotent claim. Catalog position decides when hints are absent or equal.
- Treat abstention as a wording signal. If a need you expect to match comes back absent, the description does not cover that phrasing. Broaden the description or lower `similarity_floor`.
- Treat ambiguity as an overlap signal. If two tools tie, their descriptions claim the same capability. Differentiate the descriptions, or attach hints so the tie breaks your way.

## Building and Rebuilding the Index

You build a picker from a catalog in a single call. The build loads the compiled-in model and indexes every tool. A build error is reported if the model cannot load or the catalog cannot be indexed. An empty catalog builds without error and reports every need as absent.

Loading the model is the expensive step. It happens once. You keep the returned handle and reuse it. Cloning a loaded handle is cheap. Several pickers share the same weights instead of reloading them. You can serve several catalogs from one loaded model. Each picker resolves only its own catalog's tools.

You can replace a picker's catalog with a new one. The rebuild preserves the model and the configuration. The original picker is left unchanged and still answers from its own catalog.

You can observe a picker. You can ask how many tools it indexes. You can ask whether it is empty. You can iterate its tools in catalog order, including reverse. You can look up a tool by identity. You can read back the configuration it was built with. Debug output shows the index size and shape. It never dumps raw embedding vectors.

Results borrow the picker's descriptors. No schema or descriptor is deep-cloned. To keep a resolved tool identity beyond the picker's lifetime, clone just the identity.

While the model loads, you can watch byte-level progress through an optional progress handle. While indexing, the handle advances one step per embedded tool. It completes even for an empty catalog.

## The Embedding Model Asset

The library embeds the BAAI/bge-small-en-v1.5 model, 384 dimensions, compiled into the binary. It runs locally on CPU. The finished binary needs no runtime download.

The first build needs network access. It downloads about 130 MB from the Hugging Face Hub. Later builds reuse the cache. Every downloaded file is pinned to one immutable commit and checked against a hardcoded SHA-256 digest before use. If a checksum fails, the build error names the expected and actual digests and the cache path. You delete the corrupt or tampered cached copy and rebuild.

To build offline or behind a proxy, point `HF_HUB_CACHE` or `HF_HOME` at a warm Hugging Face cache, or set `HF_ENDPOINT` to a reachable mirror.

The build downcasts the model weights from fp32 to fp16 before embedding them. The shipped binary is smaller. Repeated rebuilds skip the download and conversion work. A stamp file records the pinned revision, the conversion version, and the digests of the generated outputs. Corrupted, truncated, or replaced outputs are detected and regenerated. All generated artifacts land under the build output directory inside `target/`. Nothing is written into the source tree. At compile time you can inspect provenance: the pinned revision and the source repository are recorded alongside the embedded assets.

If the Hugging Face Hub is unreachable, the build error states which file could not be obtained, why network access is needed, and the full cause chain.

## Errors and What They Mean

Each fallible operation reports its own narrow failure category. There is no single catch-all error.

- **Model-load failure.** The compiled-in weights, tokenizer, or configuration could not be turned into a usable encoder. Every cause is a build defect. There is nothing to fix at the call site. The message names the category: configuration, dimension mismatch, provenance, tokenizer, truncation, weights, or architecture.
- **Index failure.** The catalog could not be indexed. Model-load and index failures flow into the single build error when you use the one-call build.
- **Query failure.** The need itself could not be embedded. The failure classifies into a stable category: tokenization, inference, or invalid embedding. A query failure is not an abstention. An abstention is a successful policy answer.
- **Selection failure.** A near-duplicate analysis referenced a tool not in the catalog. The error names the first missing tool identity. Selection analysis is validation, so an absent identity fails loudly rather than being silently dropped.
- **Configuration failure.** A threshold or `top_k` fell outside the supported domain. The error names the rejected field.

You can walk the underlying dependency cause of any failure through the standard error source chain. Error messages are compact lowercase noun phrases. They display transparently without wrapper noise.

## Guarantees at a Glance

- Determinism: the same model bytes, dependency versions, target, environment, catalog, configuration, and need always produce the same outcome. Cross-platform byte-identical vectors at floating-point boundaries are not promised.
- Thread safety: the model handle and the picker move or share across threads. They work in static and async contexts.
- Shared weights: two pickers over one model produce byte-identical embeddings for identical text.
- Zero-copy results: query results, shortlists, and duplicate pairs borrow the picker's descriptors. No schema or descriptor is deep-cloned.
- Model reuse: load once, clone cheaply, serve many catalogs.
- Stable text embedding: the same text always embeds to the identical vector. Cached or persisted vectors stay valid.
