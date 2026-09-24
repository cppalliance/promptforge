//! Prepare-pass tests that reach engine-only items: the per-run store
//! mount's claims isolation, and the host's prepare-run path refusing an
//! unsatisfiable prompt with today's model-readable notice or running a
//! satisfiable one. The rest of the prepare suite runs against the
//! `promptforge` facade.

use std::num::NonZeroU32;

use crate::parser::Prompt;
use crate::test_support::{RunHost, run_with_host};
use crate::{Environment, RunErrorKind, RunResult};
use promptforge_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge_vfs::Origin;

use super::support::context;

/// A prompt declaring no capabilities at all.
const DECLARES_NOTHING: &str = concat!(
    "---\n",
    "name: declares-nothing\n",
    "description: d\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Parses a fixture prompt.
fn parse(source: &str, execution: &str) -> Prompt {
    Prompt::parse(source, execution)
        .0
        .expect("the fixture prompt parses")
}

#[test]
fn two_runs_writing_the_same_store_path_do_not_conflict() {
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let env = Environment::new();
    let (ctx_a, _) = env.prepare(&prompt, context("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, context("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    let path = format!("{}/paper.md", promptforge_vfs::STORE_MOUNT);
    // Both writes proceed while both accesses are live: each run's store
    // is its own storage under its own claims table.
    access_a.write(&path, b"from a").expect("run a writes");
    access_b.write(&path, b"from b").expect("run b writes");
    assert_eq!(access_a.read(&path).expect("run a reads"), b"from a");
    assert_eq!(access_b.read(&path).expect("run b reads"), b"from b");
}

/// A prompt declaring one model role with a hard keyword and a context
/// minimum.
const DECLARES_ANALYST: &str = concat!(
    "---\n",
    "name: declares-analyst\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  analyst:\n",
    "    keywords: [frontier, thinking]\n",
    "    min_context: 200000\n",
    "    description: Deep analysis\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// Builds the host's one current model with the given context window and
/// thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("current").expect("the id is valid"),
        "The host's current model",
        NonZeroU32::new(context).expect("the context window is non-zero"),
        thinking,
    )
}

#[tokio::test]
async fn env_run_refuses_an_unsatisfiable_prompt_with_a_model_readable_notice() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    let result = run_with_host(
        &env,
        &prompt,
        "",
        context("refuse").model(current_model(32_000, ThinkingMode::Never)),
        RunHost::new(),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("an unsatisfiable prompt is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    // The notice is written to be read by a model: it names the role,
    // each failed check, and required versus actual - today's text,
    // unchanged by the catalog moving to the host.
    assert!(
        notice.starts_with("the environment cannot satisfy this prompt:"),
        "the notice opens with the standing refusal line: {notice}"
    );
    assert!(
        notice.contains(
            "role 'analyst': requires a context of at least 200000 tokens; \
             the current model provides 32000"
        ),
        "the notice gives required versus actual context: {notice}"
    );
    assert!(
        notice.contains(
            "role 'analyst': requires 'thinking'; \
             the current model's thinking capability is Never"
        ),
        "the notice gives required versus actual keywords: {notice}"
    );
}

#[tokio::test]
async fn env_run_prepares_implicitly_and_runs_a_satisfiable_prompt() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // `run_with_host` prepares implicitly, and the declared role's
    // requirements are met by the current model.
    let result = run_with_host(
        &env,
        &prompt,
        "",
        context("implicit").model(current_model(200_000, ThinkingMode::Always)),
        RunHost::new(),
    )
    .await;
    let RunResult::Ok(text) = result else {
        panic!("a satisfiable prompt runs through implicit prepare: {result:?}");
    };
    assert_eq!(text, "done");
}

/// A prompt declaring one tool slot whose capability is not declared at
/// all.
const DECLARES_ORPHAN_SLOT: &str = concat!(
    "---\n",
    "name: declares-orphan-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// The step's second test: an unmet requirement found at prepare refuses
/// the run with today's model-readable notice text, line for line.
#[tokio::test]
async fn an_unmet_requirement_produces_todays_model_readable_notice() {
    let prompt = parse(DECLARES_ORPHAN_SLOT, "declares-orphan-slot");
    // An empty catalog: the slot's capability contributed nothing, which
    // prepare reports as the missing capability.
    let result = run_with_host(
        &Environment::new(),
        &prompt,
        "",
        context("notice"),
        RunHost::new(),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("an unfilled required slot is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - missing required capability: promptforge/web"
    );
}
