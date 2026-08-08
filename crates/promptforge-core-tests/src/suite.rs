use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use promptforge_core::Error;
use promptforge_core::execute::{RunOptions, run};
use promptforge_core::lua::{LuaProgram, ToolResolver, bind_tool_declarations};
use promptforge_core::observe::{NullObserver, Observer};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::ToolId;

type Record = (String, String, String);

const LOG_EXECUTION: &str = "fixture-log-checkpoints";
const PREAMBLE_EXECUTION: &str = "fixture-preamble-return";
const STORE_EXECUTION: &str = "fixture-store-fallthrough";
const REPLY_NIL_EXECUTION: &str = "fixture-reply-nil";
const STORE_TRIAD_EXECUTION: &str = "fixture-store-triad";
const REPLY_SUBST_NIL_EXECUTION: &str = "fixture-reply-subst-nil";
const SHIPPED_PARSE: &str = "fixture-shipped-prompts";

const LOG_CHECKPOINTS: &str = include_str!("../prompts/execution/log-checkpoints.md");
const PREAMBLE_RETURN: &str = include_str!("../prompts/execution/preamble-return.md");
const STORE_FALLTHROUGH: &str = include_str!("../prompts/execution/store-fallthrough.md");
const REPLY_NIL_SECTION_ONE: &str = include_str!("../prompts/execution/reply-nil-section-one.md");
const STORE_TRIAD: &str = include_str!("../prompts/execution/store-triad.md");
const REPLY_SUBSTITUTION_NIL: &str = include_str!("../prompts/invalid/reply-substitution-nil.md");

struct ValidFixture {
    name: &'static str,
    source: &'static str,
    verify: fn(&Prompt),
}

const VALID_FIXTURES: &[ValidFixture] = &[
    ValidFixture {
        name: "valid/minimal.md",
        source: include_str!("../prompts/valid/minimal.md"),
        verify: verify_minimal,
    },
    ValidFixture {
        name: "valid/shared-library.md",
        source: include_str!("../prompts/valid/shared-library.md"),
        verify: verify_shared_library,
    },
    ValidFixture {
        name: "valid/preamble-prose-epilog.md",
        source: include_str!("../prompts/valid/preamble-prose-epilog.md"),
        verify: verify_preamble_prose_epilog,
    },
];

#[derive(Clone, Copy, Debug)]
enum ErrorKind {
    Parse,
    LuaCompile,
}

struct InvalidFixture {
    name: &'static str,
    source: &'static str,
    kind: ErrorKind,
    message_fragment: &'static str,
}

#[derive(Clone, Copy, Debug)]
enum ExecutionErrorKind {
    Substitution,
}

struct ExecutionErrorFixture {
    name: &'static str,
    source: &'static str,
    execution: &'static str,
    kind: ExecutionErrorKind,
    message_fragment: &'static str,
}

const INVALID_FIXTURES: &[InvalidFixture] = &[
    InvalidFixture {
        name: "invalid/missing-h1.md",
        source: include_str!("../prompts/invalid/missing-h1.md"),
        kind: ErrorKind::Parse,
        message_fragment: "requires an H1",
    },
    InvalidFixture {
        name: "invalid/removed-lua-prompt.md",
        source: include_str!("../prompts/invalid/removed-lua-prompt.md"),
        kind: ErrorKind::Parse,
        message_fragment: "`lua prompt` fence form was removed",
    },
    InvalidFixture {
        name: "invalid/malformed-epilog.md",
        source: include_str!("../prompts/invalid/malformed-epilog.md"),
        kind: ErrorKind::LuaCompile,
        message_fragment: "section `Transform` epilog",
    },
];

const EXECUTION_ERROR_FIXTURES: &[ExecutionErrorFixture] = &[ExecutionErrorFixture {
    name: "invalid/reply-substitution-nil.md",
    source: REPLY_SUBSTITUTION_NIL,
    execution: REPLY_SUBST_NIL_EXECUTION,
    kind: ExecutionErrorKind::Substitution,
    message_fragment: "nil",
}];

/// A synchronized observer shared by concurrent fixture runs.
#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, detail: &str) {
        self.0
            .lock()
            .expect("the fixture recorder mutex must remain usable")
            .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
    }
}

impl Recorder {
    fn records(&self) -> Vec<Record> {
        self.0
            .lock()
            .expect("the fixture recorder mutex must remain usable")
            .clone()
    }
}

#[derive(Debug)]
struct NoTools;

impl ToolResolver for NoTools {
    fn resolve(&self, capability: &str) -> promptforge_core::Result<ToolId> {
        panic!("tool-free fixture unexpectedly requested capability {capability:?}")
    }
}

fn parse_execution_fixture(
    source: &str,
    name: &str,
    execution: &str,
    observer: &dyn Observer,
) -> Prompt {
    let prompt = Prompt::parse(source, execution, observer)
        .unwrap_or_else(|error| panic!("fixture {name} failed to parse: {error}"));
    if let Some(shared) = &prompt.shared {
        let bindings = bind_tool_declarations(shared, &NoTools, execution, observer, &prompt.title)
            .unwrap_or_else(|error| {
                panic!("fixture {name} failed deterministic declaration binding: {error}")
            });
        assert!(
            bindings.bindings().is_empty(),
            "fixture {name} must remain tool-free"
        );
    }
    prompt
}

fn checkpoints(records: &[Record], execution: &str) -> Vec<Record> {
    records
        .iter()
        .filter(|(record_execution, _, detail)| {
            record_execution == execution && detail.starts_with("Lua: ")
        })
        .cloned()
        .collect()
}

fn collect_markdown(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("read repository prompt directory") {
        let path = entry.expect("read repository prompt entry").path();
        if path.is_dir() {
            collect_markdown(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

#[test]
fn valid_prompt_files_parse_through_the_public_api() {
    for fixture in VALID_FIXTURES {
        let prompt = Prompt::parse(fixture.source, fixture.name, &NullObserver)
            .unwrap_or_else(|error| panic!("fixture {} failed to parse: {error}", fixture.name));
        let verification = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (fixture.verify)(&prompt);
        }));
        assert!(
            verification.is_ok(),
            "fixture {} did not match its expected public structure",
            fixture.name
        );
    }
}

#[test]
fn invalid_prompt_files_report_public_error_contracts() {
    for fixture in INVALID_FIXTURES {
        let Err(error) = Prompt::parse(fixture.source, fixture.name, &NullObserver) else {
            panic!("fixture {} unexpectedly parsed", fixture.name);
        };

        let variant_matches = matches!(
            (fixture.kind, &error),
            (ErrorKind::Parse, Error::Parse(_)) | (ErrorKind::LuaCompile, Error::LuaCompile { .. })
        );
        assert!(
            variant_matches,
            "fixture {} returned the wrong error variant: expected {:?}, got {error:?}",
            fixture.name, fixture.kind
        );
        assert!(
            error.to_string().contains(fixture.message_fragment),
            "fixture {} error did not contain {:?}: {error}",
            fixture.name,
            fixture.message_fragment
        );
    }
}

fn verify_minimal(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter.name, "test");
    assert_eq!(prompt.frontmatter.description, "minimum valid");
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(prompt.title, "Test");
    assert!(prompt.shared.is_none());
    assert!(prompt.description_text.is_empty());
    assert_eq!(prompt.sections.len(), 1);
    assert_eq!(prompt.entry().name, "Run");
    assert_eq!(prompt.entry().level, 2);
    assert_eq!(prompt.entry().prose, "Done.");
    assert!(prompt.entry().preamble.is_none());
    assert!(prompt.entry().epilog.is_none());
}

fn verify_shared_library(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter.name, "shared_library");
    assert_eq!(
        prompt.frontmatter.description,
        "Exercise an H1 shared library and nested author prose"
    );
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(prompt.title, "Shared Library");
    assert_eq!(
        prompt.shared.as_ref().map(LuaProgram::source),
        Some("function normalize(value)\n    return string.lower(value)\nend")
    );
    assert_eq!(
        prompt.description_text,
        "The shared helper is available to each executable section."
    );
    assert_eq!(prompt.sections.len(), 2);

    let prepare = &prompt.sections[0];
    assert_eq!(prepare.name, "Prepare");
    assert_eq!(prepare.level, 2);
    assert_eq!(prepare.prose, "Normalize the supplied subject.");
    assert!(prepare.preamble.is_none());
    assert!(prepare.epilog.is_none());
    assert_eq!(prepare.children.len(), 1);
    assert_eq!(prepare.children[0].name, "Author note");
    assert_eq!(prepare.children[0].level, 3);
    assert_eq!(
        prepare.children[0].prose,
        "This nested prose remains attached to Prepare."
    );

    let finish = &prompt.sections[1];
    assert_eq!(finish.name, "Finish");
    assert_eq!(finish.prose, "Return the normalized subject.");
    assert!(finish.children.is_empty());
}

fn verify_preamble_prose_epilog(prompt: &Prompt) {
    assert_eq!(prompt.frontmatter.name, "phase_boundaries");
    assert_eq!(
        prompt.frontmatter.description,
        "Exercise an author-shaped preamble, prose, and epilog"
    );
    assert_eq!(prompt.frontmatter.promptforge, Some(1));
    assert_eq!(
        prompt.frontmatter.default_return.as_deref(),
        Some("fallback")
    );
    assert_eq!(prompt.frontmatter.max_tool_iterations, Some(3));
    assert_eq!(prompt.title, "Phase Boundaries");
    assert!(prompt.shared.is_none());
    assert_eq!(prompt.description_text, "Transform one model response.");
    assert_eq!(prompt.sections.len(), 2);

    let transform = prompt.entry();
    assert_eq!(transform.name, "Transform");
    assert_eq!(
        transform.preamble.as_ref().map(LuaProgram::source),
        Some("var.subject = args")
    );
    assert_eq!(transform.prose, "Write about {{ var.subject }}.");
    assert_eq!(
        transform.epilog.as_ref().map(LuaProgram::source),
        Some("return reply")
    );
    assert!(transform.children.is_empty());

    let fallback = &prompt.sections[1];
    assert_eq!(fallback.name, "Fallback");
    assert_eq!(fallback.prose, "This section has prose only.");
    assert!(fallback.preamble.is_none());
    assert!(fallback.epilog.is_none());
}

#[tokio::test]
async fn log_fixture_reports_exact_author_checkpoints() {
    let recorder = Recorder::default();
    let prompt = parse_execution_fixture(
        LOG_CHECKPOINTS,
        "execution/log-checkpoints.md",
        LOG_EXECUTION,
        &recorder,
    );
    let store = StoreRef::memory();
    let result = run(
        &prompt,
        "",
        &[],
        &store,
        RunOptions {
            execution: LOG_EXECUTION,
            observer: &recorder,
            client: None,
            debug: None,
        },
    )
    .await
    .expect("the log checkpoint fixture must execute offline");

    assert_eq!(result, "logged");
    assert_eq!(
        store
            .read_lines("state.txt")
            .expect("the prepare section writes state"),
        "1| prepared"
    );
    assert_eq!(
        checkpoints(&recorder.records(), LOG_EXECUTION),
        [
            (
                LOG_EXECUTION.to_owned(),
                "Log Checkpoints".to_owned(),
                "Lua: shared loaded".to_owned(),
            ),
            (
                LOG_EXECUTION.to_owned(),
                "Prepare".to_owned(),
                "Lua: shared loaded".to_owned(),
            ),
            (
                LOG_EXECUTION.to_owned(),
                "Prepare".to_owned(),
                "Lua: prepare started".to_owned(),
            ),
            (
                LOG_EXECUTION.to_owned(),
                "Prepare".to_owned(),
                "Lua: prepare finished".to_owned(),
            ),
            (
                LOG_EXECUTION.to_owned(),
                "Finish".to_owned(),
                "Lua: shared loaded".to_owned(),
            ),
            (
                LOG_EXECUTION.to_owned(),
                "Finish".to_owned(),
                "Lua: finish started".to_owned(),
            ),
        ]
    );
}

#[tokio::test]
async fn preamble_return_fixture_skips_model_and_epilog() {
    let recorder = Recorder::default();
    let prompt = parse_execution_fixture(
        PREAMBLE_RETURN,
        "execution/preamble-return.md",
        PREAMBLE_EXECUTION,
        &recorder,
    );
    let store = StoreRef::memory();
    let result = run(
        &prompt,
        "early result",
        &[],
        &store,
        RunOptions {
            execution: PREAMBLE_EXECUTION,
            observer: &recorder,
            client: None,
            debug: None,
        },
    )
    .await
    .expect("the preamble return fixture must execute without a model");

    assert_eq!(result, "early result");
    assert!(
        store.read_lines("unreachable.txt").is_err(),
        "the epilog after a scalar preamble return must not run"
    );
    assert_eq!(
        checkpoints(&recorder.records(), PREAMBLE_EXECUTION),
        [(
            PREAMBLE_EXECUTION.to_owned(),
            "Stop Early".to_owned(),
            "Lua: returning early".to_owned(),
        )]
    );
}

#[tokio::test]
async fn store_fixture_persists_state_across_fall_through() {
    let recorder = Recorder::default();
    let prompt = parse_execution_fixture(
        STORE_FALLTHROUGH,
        "execution/store-fallthrough.md",
        STORE_EXECUTION,
        &recorder,
    );
    let store = StoreRef::memory();
    let result = run(
        &prompt,
        "carried value",
        &[],
        &store,
        RunOptions {
            execution: STORE_EXECUTION,
            observer: &recorder,
            client: None,
            debug: None,
        },
    )
    .await
    .expect("the store fall-through fixture must execute offline");

    assert_eq!(result, "1| carried value");
    assert_eq!(
        store
            .read_lines("handoff.txt")
            .expect("the handoff remains stored"),
        "1| carried value"
    );
    assert_eq!(
        checkpoints(&recorder.records(), STORE_EXECUTION),
        [
            (
                STORE_EXECUTION.to_owned(),
                "Write".to_owned(),
                "Lua: writing state".to_owned(),
            ),
            (
                STORE_EXECUTION.to_owned(),
                "Read".to_owned(),
                "Lua: reading state".to_owned(),
            ),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_runs_keep_execution_ids_separate() {
    const FIRST: &str = "fixture-concurrent-first";
    const SECOND: &str = "fixture-concurrent-second";

    let recorder = Arc::new(Recorder::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let first_prompt = Arc::new(parse_execution_fixture(
        PREAMBLE_RETURN,
        "execution/preamble-return.md",
        FIRST,
        recorder.as_ref(),
    ));
    let second_prompt = Arc::new(parse_execution_fixture(
        PREAMBLE_RETURN,
        "execution/preamble-return.md",
        SECOND,
        recorder.as_ref(),
    ));

    let first_recorder = Arc::clone(&recorder);
    let first_barrier = Arc::clone(&barrier);
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        run(
            first_prompt.as_ref(),
            "first result",
            &[],
            &StoreRef::memory(),
            RunOptions {
                execution: FIRST,
                observer: first_recorder.as_ref(),
                client: None,
                debug: None,
            },
        )
        .await
    });

    let second_recorder = Arc::clone(&recorder);
    let second = tokio::spawn(async move {
        barrier.wait().await;
        run(
            second_prompt.as_ref(),
            "second result",
            &[],
            &StoreRef::memory(),
            RunOptions {
                execution: SECOND,
                observer: second_recorder.as_ref(),
                client: None,
                debug: None,
            },
        )
        .await
    });

    assert_eq!(
        first
            .await
            .expect("the first fixture task must join")
            .expect("the first fixture run must succeed"),
        "first result"
    );
    assert_eq!(
        second
            .await
            .expect("the second fixture task must join")
            .expect("the second fixture run must succeed"),
        "second result"
    );

    let records = recorder.records();
    assert_eq!(
        records
            .iter()
            .map(|(execution, _, _)| execution.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([FIRST, SECOND]),
        "the shared recorder must retain only the two caller-provided ids"
    );
    for execution in [FIRST, SECOND] {
        assert_eq!(
            checkpoints(&records, execution),
            [(
                execution.to_owned(),
                "Stop Early".to_owned(),
                "Lua: returning early".to_owned(),
            )],
            "each concurrent run must retain its own checkpoint"
        );
        assert!(
            records
                .iter()
                .filter(|(record_execution, _, _)| record_execution == execution)
                .any(|(_, section, detail)| {
                    section == "Preamble Return" && detail == "Run succeeded"
                }),
            "{execution} must retain its own terminal run record"
        );
    }
}

#[tokio::test]
async fn reply_nil_in_section_one() {
    let recorder = Recorder::default();
    let prompt = parse_execution_fixture(
        REPLY_NIL_SECTION_ONE,
        "execution/reply-nil-section-one.md",
        REPLY_NIL_EXECUTION,
        &recorder,
    );
    let result = run(
        &prompt,
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: REPLY_NIL_EXECUTION,
            observer: &recorder,
            client: None,
            debug: None,
        },
    )
    .await
    .expect("the reply nil fixture must execute offline");

    assert_eq!(result, "section one done");
}

#[tokio::test]
async fn store_triad_numbered_vs_verbatim_vs_inject() {
    let recorder = Recorder::default();
    let prompt = parse_execution_fixture(
        STORE_TRIAD,
        "execution/store-triad.md",
        STORE_TRIAD_EXECUTION,
        &recorder,
    );
    let result = run(
        &prompt,
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: STORE_TRIAD_EXECUTION,
            observer: &recorder,
            client: None,
            debug: None,
        },
    )
    .await
    .expect("the store triad fixture must execute offline");

    assert_eq!(result, "1| alpha\n2| beta|alpha\nbeta");
}

#[tokio::test]
async fn reply_substitution_nil_errors() {
    for fixture in EXECUTION_ERROR_FIXTURES {
        let recorder = Recorder::default();
        let prompt =
            parse_execution_fixture(fixture.source, fixture.name, fixture.execution, &recorder);
        let error = run(
            &prompt,
            "",
            &[],
            &StoreRef::memory(),
            RunOptions {
                execution: fixture.execution,
                observer: &recorder,
                client: None,
                debug: None,
            },
        )
        .await
        .expect_err(&format!("fixture {} must fail at execution", fixture.name));

        let variant_matches = matches!(
            (fixture.kind, &error),
            (ExecutionErrorKind::Substitution, Error::Substitution(_))
        );
        assert!(
            variant_matches,
            "fixture {} returned the wrong error variant: expected {:?}, got {error:?}",
            fixture.name, fixture.kind
        );
        assert!(
            error.to_string().contains(fixture.message_fragment),
            "fixture {} error did not contain {:?}: {error}",
            fixture.name,
            fixture.message_fragment
        );
    }
}

#[test]
fn every_shipped_prompt_parses_offline() {
    let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
    let mut files = Vec::new();
    collect_markdown(&prompts, &mut files);
    files.sort();
    assert_eq!(files.len(), 5, "every shipped markdown prompt is covered");

    for path in files {
        let source = fs::read_to_string(&path).expect("read shipped prompt");
        assert!(
            !source.contains("web_search") && !source.contains("web_fetch"),
            "{} must declare semantic capabilities, not concrete tools",
            path.display()
        );
        Prompt::parse(&source, SHIPPED_PARSE, &NullObserver)
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
    }
}
