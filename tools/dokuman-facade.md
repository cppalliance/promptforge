---
description: Bring the promptforge or harness facade's rustdoc pages up to date with its public API, using Dokuman
---

<!--
When this file is loaded, adopt it as system context and run it. Do not summarize it.
-->

# Dokuman for the facades

Run Dokuman, at `../tools-public/writing/dokuman.md` from the repository root, over one facade crate's public API with the changes in this file. If Dokuman is missing, stop and say so.

- `CRATE`: `promptforge` or `harness`.
- `BASELINE`: the commit where the pages were last right. The default is the last commit that touched `crates/CRATE/src/*.md`; `none` rewrites every page.

The pages are `crates/CRATE/src/lib.md` and one `<module>.md` per `pub mod` in its `lib.rs`. Run each script as `python tools/scripts/<script> CRATE ...` from the repository root. The scripts write to `target/dokuman-CRATE/`, where Dokuman's scratch files go too, and print at most 20 lines. When a script prints `STOP`, report its output and stop. Run at most 8 subagents at once and give them absolute paths. Dokuman's Step 0 scope tells each subagent to grep this file for `^</?facade-rules>$`, read that range, and follow it, and gives `CRATE`. Dispatch this file's task blocks with Dokuman's template, pointed at this file. Leave every change uncommitted.

1. Run `survey.py CRATE [BASELINE]`. It builds the docs at HEAD and at BASELINE and writes the change plan, the delta files, and a checklist per affected page.
2. Run `restructure.py CRATE`. For each module it lists under `MISSING_DOC_ATTR`, add `#![doc = include_str!("<module>.md")]` as the first line of that `pub mod` block, the only Rust edit allowed.
3. Run Dokuman steps 2 through 7 once, with the change plan in place of steps 0 and 1, fanning each step out per affected page:
   - Extract: one subagent per delta file.
   - Tier and verify: only pages whose action is `new`.
   - Evidence and write: one subagent per page, in place of Dokuman's single writer. Write `lib.md` first; it is the voice model for the others. The template is the new-page outline in the rules below for a new page, and the current page otherwise.
   - If `lib.md` changed, set its last paragraph to one italic line naming your model.
4. Run `gates.py CRATE`, then dispatch `audit-task` for each affected page to a subagent that did not write it.
5. Run `gates.py CRATE` again. While it prints `FAILING`, dispatch `fix-task` for each page it lists and rerun, at most 3 rounds. A `SOURCE` line names findings in internal doc comments that no page edit fixes.
6. Write `target/dokuman-CRATE/report.md`: the change plan summary, each page's action, the last gate results, any `SOURCE` findings, and each claim the audits changed.

<facade-rules>

Every subagent: the pages are in `crates/CRATE/src/` and the scratch files in `target/dokuman-CRATE/`. Do not run cargo; the scripts build and test.

Dokuman overrides:

- An extractor's assigned file is a delta file. Find each entry's defining source through the `pub use` lines in `crates/CRATE/src/lib.rs`, under `crates/CRATE-internal/`. Also write one record per entry marked `[record]`: the signature verbatim; each argument's type, meaning, and how to fill it in; the return; the failures; and the trait impls a caller relies on.
- Keep type, method, field, variant, and argument names in the evidence, overriding Dokuman's evidence packet, and copy every record into the evidence details verbatim. On a page whose action is not `new`, the extracts stand in for the tiered file and the change plan for the recon brief.
- A writer edits its page in place at `crates/CRATE/src/<page>` and no other file. It explains every entry in the page's `checklist-<stem>.txt`, with tier 3 in the Reference section, overriding Dokuman's tier coverage and 3-minute rule. A module page's writer reads the finished `lib.md` first as the voice model and links to its explanations instead of repeating them.
- The reader is a Rust developer calling CRATE from their own program. They know Rust, traits, enums, async, `Arc`, and `Result`, and nothing about this crate.
- On a page whose action is `update` or `rename`, change only what the delta entries' `do:` lines name.
- New-page outline: a one-sentence summary and an orientation paragraph; `Where this fits`; one to three task-named tour sections, each opening with a worked example; `Reference`, with one level-2 heading per public item. `lib.md` keeps its top-level headings and ends with `Reference` and `Where to go next`.
- promptforge only: `lib.md` keeps its one-screen primer on PromptForge prompts, drawn from `guide/src/language/`.

Page rules:

- Link every Rust symbol in prose, every time. On a module page, link the module's own items by bare name and every other item by its `crate::` path. Link a variant field through its enum's anchor, for example [`LogError::Database::source`](LogError#variant.Database.field.source).
- Every Rust example is a doctest that compiles against CRATE alone, through facade paths. Put an example that needs a runtime or a live service inside an `async fn` that the doctest defines and never calls. Build prompt sources with `concat!`, because rustdoc hides lines that start with `# `.
- Escape literal square brackets as `\[` and `\]`.
- Never name an internal crate, a `crates/CRATE-internal` path, this tool, or a scratch path.
- Before returning a page, run `python tools/scripts/check_docs.py CRATE <page>` and fix what it reports, at most 3 runs.

</facade-rules>

<audit-task>

Audit one page you did not write, and fix it in place. Fields: Page; Failures, the page's `failures-<stem>.txt` from the latest `gates-<n>/` directory, or `none`; Evidence; Details. Fix each failure first. Then check every doctest against the details file and every claim against the evidence; correct what is wrong and delete what the evidence does not support. Edit only the page. Return the status, the number of fixes, and each claim you changed.

</audit-task>

<fix-task>

Fix the gate failures listed for one page with the smallest edit that passes each. Fields: Page; Failures; Details, or `none`. Change no line that no failure names. Edit only the page. Return the status and the number of failures fixed.

</fix-task>
