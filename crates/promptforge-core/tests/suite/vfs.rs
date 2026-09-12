//! The executor's `VfsRef` host contract: an end-to-end run over the stock
//! handle, and the papergate-shaped seed-run-extract round trip a production
//! host drives with no real files - seed the declared input through
//! `vfs.store()`, run, extract the declared output, and charge a missing
//! output to the prompt's promise as an explicit contract error.

use promptforge_core::parser::Prompt;
use promptforge_core::store::{Store, StoreError, StoreExt};
use shared_vfs::VfsRef;

use super::support::{RunOptions, parse_execution_fixture, run};
use crate::support::Recorder;
use std::sync::Arc;

const ROUND_TRIP: &str = concat!(
    "---\n",
    "name: papergate-round-trip\n",
    "description: Seed, run, extract\n",
    "promptforge: 0\n",
    "input:\n",
    "  path: paper.md\n",
    "  description: The input paper\n",
    "output:\n",
    "  path: report.md\n",
    "  description: The output report\n",
    "---\n\n",
    "# Review\n\n",
    "## Summarize\n\n",
    "```lua\n",
    "local paper = store.read('paper.md')\n",
    "store.write('report.md', 'report on: ' .. paper)\n",
    "return 'done'\n",
    "```\n",
);

const MISSING_OUTPUT: &str = concat!(
    "---\n",
    "name: papergate-missing-output\n",
    "description: Never writes its promised report\n",
    "promptforge: 0\n",
    "input:\n",
    "  path: paper.md\n",
    "  description: The input paper\n",
    "output:\n",
    "  path: report.md\n",
    "  description: The output report\n",
    "---\n\n",
    "# Review\n\n",
    "## Summarize\n\n",
    "```lua\n",
    "local paper = store.read('paper.md')\n",
    "return 'read: ' .. paper\n",
    "```\n",
);

/// The production host's extraction rule (pattern: papergate's app.rs): a
/// declared output the run did not leave behind is an explicit contract
/// error naming the prompt's promise, never a bare not-found.
fn extract_declared_output(store: &Store, prompt: &Prompt) -> Result<String, String> {
    let output = prompt
        .frontmatter()
        .output()
        .expect("the fixture declares an output");
    store.read(output.path()).map_err(|error| match error {
        StoreError::NotFound { .. } => format!(
            "the prompt promised output '{}' ({}) but the run left no such file",
            output.path(),
            output.description()
        ),
        other => format!("extracting the declared output failed: {other}"),
    })
}

/// Seeds the prompt's declared input through the stock handle's store
/// facade. The seeding access drops here - its claims release - so the
/// run's own identity never meets the host's.
fn seed_declared_input(vfs: &VfsRef, prompt: &Prompt, contents: &str) {
    let input = prompt
        .frontmatter()
        .input()
        .expect("the fixture declares an input");
    let access = vfs.acquire();
    vfs.store(&access)
        .write(input.path(), contents)
        .expect("the declared input seeds");
}

fn offline_run(
    prompt: &Prompt,
    vfs: &VfsRef,
    execution: &'static str,
) -> impl std::future::Future<Output = Result<String, promptforge_core::execute::RunError>> {
    let recorder = Arc::new(Recorder::default());
    let prompt = prompt.clone();
    let vfs = vfs.clone();
    async move {
        run(
            &prompt,
            "",
            &[],
            &vfs,
            RunOptions {
                execution,
                observer: recorder,
            },
        )
        .await
    }
}

#[tokio::test]
async fn an_end_to_end_run_threads_one_vfs_ref_through_every_section() {
    // The store survives the context-clearing section transition: one
    // section's write is the next section's read, over the stock handle.
    let source = "\
---\nname: vfs-end-to-end\ndescription: d\npromptforge: 0\n---\n\n\
# Title\n\n\
## First\n\n\
```lua\n\
store.write('handoff.txt', 'across the reset')\n\
```\n\n\
## Second\n\n\
```lua\n\
return store.read('handoff.txt')\n\
```\n";
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(source, "vfs-end-to-end", "vfs-e2e", recorder.as_ref());
    let vfs = promptforge_vfs::empty();
    let result = run(
        &prompt,
        "",
        &[],
        &vfs,
        RunOptions {
            execution: "vfs-e2e",
            observer: recorder,
        },
    )
    .await
    .expect("the run threads the stock handle through both sections");
    assert_eq!(result, "across the reset");
    // Extraction after the run takes a fresh access: the run's identities
    // dropped with it, so nothing the run touched can conflict here.
    let access = vfs.acquire();
    assert_eq!(
        vfs.store(&access)
            .read("handoff.txt")
            .expect("the run's write persists on the handle"),
        "across the reset"
    );
}

#[tokio::test]
async fn a_host_seeds_and_extracts_through_the_stock_handle_with_no_real_files() {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(
        ROUND_TRIP,
        "papergate-round-trip",
        "vfs-round-trip",
        recorder.as_ref(),
    );
    let vfs = promptforge_vfs::empty();
    seed_declared_input(&vfs, &prompt, "the paper body");
    let result = offline_run(&prompt, &vfs, "vfs-round-trip")
        .await
        .expect("the seeded run executes offline");
    assert_eq!(result, "done");
    let access = vfs.acquire();
    let report = extract_declared_output(&vfs.store(&access), &prompt)
        .expect("the run left its promised output");
    assert_eq!(report, "report on: the paper body");
}

#[tokio::test]
async fn a_missing_declared_output_is_a_contract_error_naming_the_prompts_promise() {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(
        MISSING_OUTPUT,
        "papergate-missing-output",
        "vfs-missing-output",
        recorder.as_ref(),
    );
    let vfs = promptforge_vfs::empty();
    seed_declared_input(&vfs, &prompt, "the paper body");
    // The executor does not enforce the declaration; the run succeeds and
    // the host's extraction is where the broken promise surfaces.
    let result = offline_run(&prompt, &vfs, "vfs-missing-output")
        .await
        .expect("the run itself succeeds");
    assert_eq!(result, "read: the paper body");
    let access = vfs.acquire();
    let error = extract_declared_output(&vfs.store(&access), &prompt)
        .expect_err("the missing output is a contract error");
    assert!(
        error.contains("report.md"),
        "the error names the promised path: {error}"
    );
    assert!(
        error.contains("The output report"),
        "the error names the promise's description: {error}"
    );
}
