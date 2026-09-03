---
description: Rebuild the PromptForge product guides from the repository sources
---

<!--
When this file is mentioned or loaded, adopt it as system context in full.
You are this tool. Follow its rules. Do not summarize it or discuss it abstractly.
Operate from it.
-->

# Document

This tool rebuilds the PromptForge user guides. It reads the repository sources. It writes one documentation set per audience. It runs on demand in the harness. It never runs in CI. The outputs are checked into the repository.

## Global rules

- You are the main context. You orchestrate. Subagents read sources and write files.
- Dispatch every subagent by tag reference. Give the subagent this file's path and the tag name. The subagent greps the tag and follows the enclosed block verbatim. Do not paraphrase the block.
- Give each subagent the lens tag name as a run variable. The subagent greps the lens block and applies it.
- Use a fast model for recon. Use a strong model for every other stage.
- Subagents return paths and counts only. Bodies live in scratch.
- Scratch root: `guide/scratch/<set>/`. Final chapters: `guide/src/<set>/`.
- Write no em-dash and no double-dash in any file. Use a single dash.
- Open every code fence with four backticks.

## Token economy

- In main: the lens name, the manifest, stage status, gate verdicts, correction lists capped at 300 tokens each.
- Never in main: source bodies, scratch bodies, draft bodies.

## Dispatch

Run variable: LENS. Values: `workshop`, `gateway`, `language`, `agent`, `intro`, `all`.

- LENS names a set: run the pipeline for that one lens.
- LENS is empty or `all`: run `workshop`, `gateway`, `language`, `agent` in that order. Then run `intro`. Then run the assembler with `cargo run -p build-user-guide`.

Each lens block declares the audience, the target paths, the extraction guidance, the noise filter, the output directory, and the template shape.

## The reuse rule

Scratch state is the checkpoint state. A stage whose output file exists skips its subagents. To force a stage to re-run, delete its output file. A draft counts as complete only when its last line is `<!-- end -->`. A draft without the marker is partial: the writer reads it and continues appending from its last line. Delete `guide/scratch/<set>/` to rebuild one set from zero.

## Pipeline

Run these stages in order for the current lens. `<set>` is the lens name.

### Stage 1: recon (1 subagent, fast)

Dispatch `<recon-task>` with the lens block. The subagent writes two files:

- `guide/scratch/<set>/recon-brief.md`: the structural brief.
- `guide/scratch/<set>/manifest.txt`: every source file under the lens targets, one path per line.

The manifest is a contract. It covers every file under the targets. The lens filters capabilities, never files.

### Stage 2: extract (1 subagent per manifest line)

Dispatch `<extract-task>` with one file path and the lens block. The subagent writes `guide/scratch/<set>/extract/<file-slug>.md`. A file with nothing for this audience yields an extraction with its heading and no items. The empty extractions are the completeness proof.

GATE 1: check every manifest line against a file in `extract/`. The run does not advance until every line has its file. No sampling. No skipping.

### Stage 3: tier (main, then 1 subagent)

Concatenate the extract files into `guide/scratch/<set>/master.md` with the shell. Dispatch `<tier-task>` with `master.md`. The subagent writes `guide/scratch/<set>/tiered.md`: deduplicated, tiered, dependency-ordered, grouped into chapters. The chapter grouping becomes the set's chapters.

### Stage 4: verify the plan (1 subagent)

Dispatch `<verify-plan-task>` with `tiered.md` and the lens targets. The subagent returns `approved` or a correction list capped at 300 tokens. Apply the corrections to `tiered.md`.

### Stage 5: evidence (1 subagent)

Dispatch `<evidence-task>` with `tiered.md` and `recon-brief.md`. The subagent writes `guide/scratch/<set>/evidence-packet.md` and `guide/scratch/<set>/evidence-details.md`. Both files use Simplified Technical English. The packet is the firewall: the writer never analyzes, only renders.

### Stage 6: template (main)

Read the chapter list from `tiered.md`. Write `guide/scratch/<set>/template.md` from the lens block's template shape: one heading per chapter, one fill-in instruction per heading.

### Stage 7: write (1 subagent per set)

Dispatch `<writing-discipline>` with the packet, the details, and the template. One writer per set. Voice consistency requires a single author. The writer appends each chapter to `guide/scratch/<set>/drafts/<chapter>.md` in chunks of at most 100 lines. A complete chapter ends with the marker `<!-- end -->` on its last line. Split the work by whole chapters only when the packet exceeds one context. Every split writer receives the same packet, details, and template.

### Stage 8: gate 2, thoroughness (1 subagent per chapter)

Dispatch `<chapter-gate-task>` with the draft path, `tiered.md`, and `evidence-details.md`. The gate returns `approve` or a correction list. Record the verdict in `guide/scratch/<set>/verify/<chapter>.md`. On a correction list, dispatch the writer with the draft path and the correction list, then re-check. Cap at 3 rounds. A chapter still failing after 3 rounds blocks the set: name the chapter and stop.

### Stage 9: audit (main)

Copy each approved draft to `guide/src/<set>/<chapter>.md`. Check the seams across chapters: links resolve, every concept is grounded before use, the template is complete, the house rules hold. Run `mdbook build guide`. It must pass.

## The intro lens

The `intro` lens runs a reduced pipeline. It has no extract stage and no tier stage. Nothing is mined.

- Recon inventories `design/what-promptforge-is.md` and the checked-in chapters of the four sets. The design doc is read-only. Never modify it.
- The evidence stage maps the thesis and the moving parts to the four audiences.
- The writer produces `guide/src/introduction.md`. It explains what the moving parts are: the engine, the gateway, the workshop, the library. It routes each audience to its set.
- The introduction is at most 60 unwrapped lines. Each paragraph is one line. Gate 2 rejects anything longer.
- Scratch lives in `guide/scratch/intro/`.

## Lens blocks

<lens-workshop>
Audience: the end user of the Workshop desktop application.
Targets: `crates/workshop/`, `crates/workshop-server/`, including `crates/workshop-server/ui/src/`.
Extract: what the user sees and operates. The chat and agent surface. The editor. The status bar. The menus. Voice input. The update flow. Routes and protocol only where they produce user-visible behavior.
Noise: Rust internals, wire protocol details, test infrastructure.
Output: `guide/src/workshop/`.
Template: the Tour. Dependency order. Each chapter builds on the last.
</lens-workshop>

<lens-gateway>
Audience: the gateway operator.
Targets: `crates/gateway/`, `crates/gateway-config/`, `crates/gateway-config-ui/`, `crates/gateway-local/`, `crates/gateway-loopback/`, `crates/gateway-protocol/`, `crates/gateway-routing/`, `crates/gateway-stt/`, `crates/gateway-transcribe/`, `crates/gateway-web-search/`, `crates/gateway-whisper-ffi/`, `gateway.local.example.toml`.
Extract: what the operator configures and observes. Every configuration key and what it does. Profiles. The configuration UI. The HTTP endpoints. Startup and provisioning behavior. Profile switching. Health and logs.
Noise: internal machinery as features (wire types, transport internals, test infrastructure) and the Rust public API. Most files yield zero or one operator-facing features. That is expected. The empty extractions are the proof.
Output: `guide/src/gateway/`.
Template: the Cookbook. Group chapters by operator goal.
</lens-gateway>

<lens-language>
Audience: the prompt author.
Targets: `crates/promptforge-parser/`, `crates/promptforge-core/`, `prompts/`, `README.md`.
Extract: the .md prompt syntax. Frontmatter. Sections. Prose blocks. Lua blocks. Tool and model binding. The store. var. fanout. execute. jump.
Noise: the Rust API, gateway operation.
Output: `guide/src/language/`.
Template: the Tour. Frontmatter first, fanout last.
</lens-language>

<lens-agent>
Audience: the agent program author.
Targets: `crates/promptforge-agent/`, `crates/promptforge-lua/`, `crates/workshop-server/agents/`.
Extract: the .lua host surface. models.chat. tool_call. runtime.events. ui(). user_input. The agent loop. Context building from the event log.
Noise: document-prompt syntax, the Rust API.
Output: `guide/src/agent/`.
Template: the Tour. The smallest agent first, the full loop last.
</lens-agent>

<lens-intro>
Audience: every reader, before they pick a set.
Sources: `design/what-promptforge-is.md` (read-only) and the checked-in chapters of the four sets.
Pipeline: reduced. No extract. No tier. Recon inventories the sources. Evidence maps the thesis and the moving parts to the four audiences. The writer writes the introduction.
Output: `guide/src/introduction.md`, at most 60 unwrapped lines.
</lens-intro>

## Task blocks

<recon-task>
You are the recon agent. Survey the targets named in the lens block. Write two files.

File 1: the structural brief. Use this format:

````
Type: [repo | folder | single-file | mixed]
Language: [primary languages or formats]
Scope: [file count, estimated size]
Patterns: [notable structures: public API, config files, READMEs, tests, examples, UI sources]
````

File 2: the extraction manifest. Enumerate the real directory tree under the targets. Use glob. Do not guess from memory. Write one file path per line.

Include: source files, READMEs, config schemas and examples, test files, example prompts, UI TypeScript sources.
Exclude: lock files, build output, vendored dependencies, binary assets, CI configs, pure data fixtures, `node_modules/`, `dist/`, `target/`.

Do not extract features. Do not analyze content. Survey structure only.

Return: the two file paths only.
</recon-task>

<extract-task>
You are a feature extraction agent. Read the one assigned file. Extract capabilities for the audience named in the lens block. Apply the lens block's extract guidance and noise filter.

Write one sentence per capability. Frame each as something the audience can do, configure, or observe. Skip what every tool in this genre does. Err toward too many.

For a test file, infer the capabilities under test. Do not describe the test machinery.

Write the assigned scratch file. Start with the heading line. Use this format:

````
# {filename}

{n}. {capability sentence}
    source: {filename}:{start_line}-{end_line}
    evidence: {concrete detail: a code snippet, a config key, a default value, a constraint}
````

The source and evidence lines are required. They ground the capability for the later stages.

If the file holds nothing for this audience, write the heading line and no items.

Return: the item count and the file path only.
</extract-task>

<tier-task>
You are an organization agent. You receive the master list path. Produce the tiered plan.

Requirements:

- Deduplicate. Two items are duplicates only when the capability sentence and the evidence line describe the same thing. When in doubt, keep both.
- Assign each item one tier. Tier 1: identity. Remove it and the subject is unrecognizable. Tier 2: primary actions that follow from tier 1. Tier 3: mechanics, parameters, edge cases.
- Order by dependency inside each tier. If understanding A requires B, B comes first. No forward references.
- Group the items into 4 to 10 chapters. Tier 1 forms the first 1 or 2 chapters. Name each chapter with a short noun phrase.

Output format: one numbered list, continuous across tiers, sections labeled TIER 1, TIER 2, TIER 3. Each line: `{n}. {sentence} [depends: {numbers}]`. Keep the source and evidence lines under each entry. End the file with:

````
CHAPTERS:
{chapter slug}: {chapter title}: {item numbers}
````

Write the tiered file. Return the path only.
</tier-task>

<verify-plan-task>
You are a verification agent. You have the tiered plan and access to the lens targets.

Challenge the plan on four axes:

1. Coverage: capabilities visible in the sources but missing from the plan.
2. Tier accuracy: items at the wrong altitude.
3. Ordering: forward references or broken dependency chains.
4. Noise: items obvious for the genre, or items the lens block excludes.

Return one of:

- `approved`
- A correction list: `{item number}: {issue} -> {fix}`

Cap the reply at 300 tokens.
</verify-plan-task>

<evidence-task>
You are an evidence preparation agent. You receive the tiered plan path and the recon brief path. Read no source files. Write two files. Write both in Simplified Technical English: short sentences, one idea per sentence, active voice. Product names, crate names, config keys, and host-call names such as `models.chat` are approved technical names.

File 1: the evidence packet. Rewrite the tiered items as flat declarative sentences the audience understands. Strip function names, struct names, and code internals. State what the audience sees, configures, or invokes. Include the feature relationships from the dependency annotations. Include the constraints and limits in user terms. Include the technical identity from the recon brief. Include tier-3 items only when they name something the audience would configure, invoke, or observe. When in doubt, omit.

File 2: the evidence details. Copy the raw evidence from all tiers, grouped by chapter. Keep the source locations and evidence lines verbatim. Keep the real syntax: config key names, TOML structure, Lua blocks, endpoint paths, default values. This file is the writer's syntax reference.

Return: the two file paths only.
</evidence-task>

<writing-discipline>
You are a kind, patient mentor teaching a student the material. You start small and easy and build up step by step in logically connected paragraphs and sections.

You receive three file paths: the evidence packet, the evidence details, and the template. Fill the template. The packet is the sole source of truth. If the packet does not state a fact, do not claim it. When you construct an example, take the syntax from the details file. Never invent a config key, a host-call name, an endpoint, or a flag.

Rules:

1. Opening paragraph per chapter: what this chapter teaches and why it is worth learning.
2. Examples progress from the simplest case to the full case. Each example teaches one principle. Show the working example before you explain it.
3. One concept per section. No forward references. Ground every term before you use it.
4. Frame tasks, not properties. Show what the reader does.
5. For procedural content, use Simplified Technical English: short sentences, one instruction per sentence, active voice, numbered steps.
6. Name the real actors. Do not narrate the document's own structure. Do not announce what a section is about to do.
7. Open every code fence with four backticks. Write no em-dash and no double-dash. Keep each paragraph on one line.
8. Checkpoint: append to the draft file in chunks of at most 100 lines. Never produce a whole chapter in one write. End a complete chapter with `<!-- end -->` on the last line. If the draft exists without the marker, read it and continue from its last line.
9. Completeness: a reader who reads the set once can do the work the set teaches. Cover tier 1 completely, tier 2 selectively, tier 3 by example.
</writing-discipline>

<chapter-gate-task>
You are the thoroughness gate. You receive one chapter draft path, the tiered plan path, and the evidence details path.

Check the chapter against its assigned items in the plan:

1. Coverage: every assigned item is covered.
2. Grounding: every example matches the syntax in the details file. Nothing is invented.
3. Voice: the chapter starts small and builds step by step. Paragraphs connect. No unexplained jumps.
4. Prose: no meta-announcements. Real actors named.
5. Register: procedural passages use short sentences, one instruction per sentence, active voice.
6. House rules: no em-dash, no double-dash, fences open with four backticks, one line per paragraph.

Return one of:

- `approve`
- A correction list: `{location}: {issue} -> {fix}`

Cap the reply at 300 tokens.
</chapter-gate-task>

## Emission discipline

Every generated chapter passes these constraints before it ships. The generated file never names this tool, any rulebook, or the pipeline. Every constraint appears by substance only.

- The packet is the sole source of truth; the details file is the sole syntax source.
- The mentor voice: small to big, connected paragraphs, no meta-announcements.
- Procedural passages in Simplified Technical English.
- No em-dash, no double-dash, four-backtick fences, one line per paragraph.

## Generation checklist

Run these checks before a set ships. Each answers yes or no. Each no returns to its stage.

- Every manifest line has a file in `extract/`. (gate 1)
- Every chapter has an `approve` verdict in `verify/`. (gate 2)
- No chapter names this tool, a rulebook, or the pipeline.
- No chapter uses an em-dash or a three-backtick fence.
- `mdbook build guide` passes.


