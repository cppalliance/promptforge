//! Prepare-pass tests that reach engine-only items: the per-run store
//! mount's claims isolation, and the test driver's prepare-then-run path
//! refusing an unsatisfiable prompt with the model-readable notice or
//! running a satisfiable one. The rest of the prepare suite runs against
//! the `promptforge` facade.

use std::num::NonZeroU32;

use crate::parser::Prompt;
use crate::test_support::{RunFixture, run_prepared, run_with_fixture};
use crate::{Environment, RunErrorKind, RunResult};
use promptforge_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge_vfs::Origin;

use super::super::{ScriptedChat, gateway_client, resp_text};
use super::support::context;

/// A prompt declaring no Plugins at all.
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
    let store_a =
        promptforge_vfs::detail::store_view(&access_a).expect("run a's handle declares a store");
    let store_b =
        promptforge_vfs::detail::store_view(&access_b).expect("run b's handle declares a store");
    // Both writes proceed while both accesses are live: each run's store
    // is its own storage under its own claims table.
    store_a.write("paper.md", b"from a").expect("run a writes");
    store_b.write("paper.md", b"from b").expect("run b writes");
    assert_eq!(store_a.read("paper.md").expect("run a reads"), b"from a");
    assert_eq!(store_b.read("paper.md").expect("run b reads"), b"from b");
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

/// Builds the one current model the caller chose, with the given context
/// window and thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("current").expect("the id is valid"),
        "The current model",
        NonZeroU32::new(context).expect("the context window is non-zero"),
        thinking,
    )
}

#[tokio::test]
async fn env_run_refuses_an_unsatisfiable_prompt_with_a_model_readable_notice() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    let result = run_with_fixture(
        &env,
        &prompt,
        "",
        context("refuse").model(current_model(32_000, ThinkingMode::Never)),
        RunFixture::new(),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("an unsatisfiable prompt is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    // The notice is written to be read by a model: it names the role,
    // each failed check, and required versus actual.
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
    // `run_with_fixture` prepares implicitly, and the declared role's
    // requirements are met by the current model.
    let result = run_with_fixture(
        &env,
        &prompt,
        "",
        context("implicit").model(current_model(200_000, ThinkingMode::Always)),
        RunFixture::new(),
    )
    .await;
    let RunResult::Ok(text) = result else {
        panic!("a satisfiable prompt runs through implicit prepare: {result:?}");
    };
    assert_eq!(text, "done");
}

/// A prompt declaring one `no-thinking` role whose section asks it one
/// question.
const DECLARES_NO_THINKING: &str = concat!(
    "---\n",
    "name: declares-no-thinking\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer:\n",
    "    keywords: [no-thinking]\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "models.use('writer')\n",
    "return models.infer('ping')\n",
    "```\n",
);

#[tokio::test]
async fn a_no_thinking_role_on_a_switchable_model_prepares_and_asks_for_thinking_off() {
    let prompt = parse(DECLARES_NO_THINKING, "declares-no-thinking");
    let gateway = ScriptedChat::new(vec![resp_text("pong")]);
    let (ctx, requirements) = Environment::new().prepare(
        &prompt,
        context("switchable").model(current_model(32_000, ThinkingMode::Switchable)),
    );
    assert!(
        requirements.is_satisfied(),
        "a switchable model can turn thinking off: {requirements:?}"
    );
    let fixture = RunFixture::new().client(gateway_client(&gateway));
    let result = run_prepared(&prompt, "", ctx, fixture).await;
    let RunResult::Ok(text) = result else {
        panic!("the prepared prompt runs: {result:?}");
    };
    assert_eq!(text, "pong");
    let body = gateway
        .last_request()
        .expect("the round reaches the gateway");
    assert_eq!(body.options.model(), "current");
    assert_eq!(body.options.thinking(), Some(false));
}

#[tokio::test]
async fn a_no_thinking_role_on_an_always_thinking_model_is_refused() {
    let prompt = parse(DECLARES_NO_THINKING, "declares-no-thinking");
    let result = run_with_fixture(
        &Environment::new(),
        &prompt,
        "",
        context("always").model(current_model(32_000, ThinkingMode::Always)),
        RunFixture::new(),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("a model that always thinks cannot satisfy no-thinking: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains(
            "role 'writer': requires 'no-thinking'; \
             the current model's thinking capability is Always"
        ),
        "the notice gives required versus actual keywords: {notice}"
    );
}

/// A prompt declaring one role that needs a long context.
const DECLARES_LONG_CONTEXT: &str = concat!(
    "---\n",
    "name: declares-long-context\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  reader:\n",
    "    min_context: 200000\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// An unmet requirement found at prepare refuses the run with the
/// model-readable notice text, line for line.
#[tokio::test]
async fn an_unmet_requirement_produces_todays_model_readable_notice() {
    let prompt = parse(DECLARES_LONG_CONTEXT, "declares-long-context");
    let result = run_with_fixture(
        &Environment::new(),
        &prompt,
        "",
        context("notice").model(current_model(32_000, ThinkingMode::Switchable)),
        RunFixture::new(),
    )
    .await;
    let RunResult::Failure(error) = result else {
        panic!("a role the current model cannot fill is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - role 'reader': requires a context of at least 200000 tokens; \
         the current model provides 32000"
    );
}
