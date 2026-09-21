//! Tests for the frontmatter contract keys: `capabilities`, `tools`,
//! `args`, and `models` (the frontmatter matrix from the plan's Testing
//! Plan; structured error locations are a later step).

use std::num::NonZeroU32;

use super::{ArgType, ModelKeyword, ToolSlot};
use crate::{ParseError, ParseErrorKind, Prompt};

fn parse(yaml: &str) -> Result<Prompt, ParseError> {
    let src = format!("---\n{yaml}---\n\n# T\n\n## S\n\np\n");
    Prompt::parse(&src, "test").0
}

#[test]
fn the_full_contract_declaration_parses_and_round_trips() {
    let prompt = parse(concat!(
        "name: x\n",
        "description: d\n",
        "capabilities:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
        "    config:\n",
        "      servers: [alpha]\n",
        "tools:\n",
        "  search: promptforge/web/search\n",
        "  fetch: promptforge/web/fetch\n",
        "args:\n",
        "  use_mcp:\n",
        "    type: boolean\n",
        "    default: true\n",
        "    description: Search MCP-connected private sources\n",
        "models:\n",
        "  analyst:\n",
        "    keywords: [frontier, thinking]\n",
        "    min_context: 200000\n",
        "    description: deep reasoning\n",
        "  triage:\n",
        "    keywords: [fast, small]\n",
        "    description: quick triage of search results\n",
    ))
    .expect("the full contract matrix must parse");

    let fm = prompt.frontmatter();

    let caps = fm.capabilities();
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].id().to_string(), "promptforge/web");
    assert!(!caps[0].is_optional());
    assert!(caps[0].config().is_none());
    assert_eq!(caps[1].id().to_string(), "io.github.corp/mcp");
    assert!(caps[1].is_optional());
    let config = caps[1].config().expect("the detailed entry has config");
    assert_eq!(
        config["servers"],
        serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::String("alpha".to_owned())])
    );

    let tools = fm.tools();
    assert_eq!(tools.len(), 2);
    match tools.get("search") {
        Some(ToolSlot::Exact(id)) => assert_eq!(id.to_string(), "promptforge/web/search"),
        other => panic!("expected an exact slot, got {other:?}"),
    }
    match tools.get("fetch") {
        Some(ToolSlot::Exact(id)) => assert_eq!(id.to_string(), "promptforge/web/fetch"),
        other => panic!("expected an exact slot, got {other:?}"),
    }

    let arg = fm.args().get("use_mcp").expect("the arg is declared");
    assert_eq!(arg.kind(), ArgType::Boolean);
    assert!(!arg.is_optional());
    assert_eq!(arg.default(), Some(&serde_yaml_ng::Value::Bool(true)));
    assert_eq!(
        arg.description(),
        Some("Search MCP-connected private sources")
    );

    let analyst = fm.models().get("analyst").expect("the role is declared");
    assert_eq!(
        analyst.keywords(),
        &[ModelKeyword::Frontier, ModelKeyword::Thinking]
    );
    assert_eq!(analyst.min_context(), NonZeroU32::new(200_000));
    assert_eq!(analyst.description(), Some("deep reasoning"));
    let triage = fm.models().get("triage").expect("the role is declared");
    assert_eq!(
        triage.keywords(),
        &[ModelKeyword::Fast, ModelKeyword::Small]
    );
    assert_eq!(triage.min_context(), None);
}

#[test]
fn a_prompt_without_contract_keys_behaves_exactly_as_today() {
    let prompt = parse("name: x\ndescription: d\n").expect("a plain prompt must parse");
    let fm = prompt.frontmatter();
    assert!(fm.capabilities().is_empty());
    assert!(fm.tools().is_empty());
    assert!(fm.models().is_empty());
}

#[test]
fn an_omitted_args_key_yields_the_default_prose_declaration() {
    // There are no freeform prompts: an absent `args:` key is the default
    // declaration of one optional string field named `prose`.
    let prompt = parse("name: x\ndescription: d\n").expect("a plain prompt must parse");
    let args = prompt.frontmatter().args();
    assert_eq!(args.len(), 1);
    let prose = args
        .get("prose")
        .expect("the default declaration names `prose`");
    assert_eq!(prose.kind(), ArgType::String);
    assert!(prose.is_optional());
    assert_eq!(prose.default(), None);
    assert_eq!(prose.description(), Some("Freeform input for this prompt"));
}

#[test]
fn an_unknown_key_inside_a_contract_entry_is_rejected() {
    // `deny_unknown_fields` must hold inside each new key's entries too: a
    // typo'd field is an authoring error, not silently ignored.
    for yaml in [
        "name: x\ndescription: d\ncapabilities:\n  - ref: promptforge/web\n    optionl: true\n",
        "name: x\ndescription: d\ntools:\n  wiki:\n    wants: prose\n",
        "name: x\ndescription: d\nargs:\n  flag:\n    tipe: boolean\n",
        "name: x\ndescription: d\nmodels:\n  analyst:\n    keyword: [fast]\n",
    ] {
        let error = parse(yaml).expect_err("an unknown key inside an entry must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    }
}

#[test]
fn a_capability_id_must_have_exactly_two_segments() {
    for id in ["web", "promptforge/web/fetch", "promptforge//web"] {
        let yaml = format!("name: x\ndescription: d\ncapabilities:\n  - {id}\n");
        let error = parse(&yaml).expect_err("a bad capability id must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{id}: {error}");
    }
}

#[test]
fn an_at_sign_in_a_capability_id_is_rejected() {
    // v1 is unversioned: version pins are deferred, so `@` is a parse error.
    let error = parse("name: x\ndescription: d\ncapabilities:\n  - promptforge/web@1\n")
        .expect_err("a `@` version pin must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_capability_id_with_uppercase_is_rejected() {
    let error = parse("name: x\ndescription: d\ncapabilities:\n  - Promptforge/web\n")
        .expect_err("uppercase is outside the segment charset");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_capability_is_required_unless_flagged_optional() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "capabilities:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
    ))
    .expect("capability entries must parse");
    let caps = prompt.frontmatter().capabilities();
    assert_eq!(caps.len(), 2);
    assert!(!caps[0].is_optional(), "a plain string entry is required");
    assert!(caps[1].is_optional());
}

#[test]
fn a_capability_entry_must_be_a_string_or_a_ref_map() {
    let error = parse("name: x\ndescription: d\ncapabilities:\n  - 42\n")
        .expect_err("a numeric capability entry must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_map_valued_tool_slot_is_rejected_naming_the_exact_path_expectation() {
    // Exact paths are the only slot form: the former fuzzy `{ want, optional }`
    // map is no longer a slot, so it fails to parse rather than binding a
    // picker that no longer exists.
    let error = parse(concat!(
        "name: x\ndescription: d\n",
        "tools:\n",
        "  wiki:\n",
        "    want: searches private wikis\n",
        "    optional: true\n",
    ))
    .expect_err("a map-valued tool slot must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    assert!(
        error.to_string().contains("an exact tool path string"),
        "the error must name the exact-path expectation: {error}"
    );
}

#[test]
fn a_malformed_exact_tool_path_is_a_parse_error() {
    for path in [
        "promptforge/web",
        "web",
        "promptforge/Web/fetch",
        "promptforge/web/",
    ] {
        let yaml = format!("name: x\ndescription: d\ntools:\n  search: {path}\n");
        let error = parse(&yaml).expect_err("a malformed exact path must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{path}: {error}");
    }
}

#[test]
fn the_reserved_open_tool_slot_key_is_rejected() {
    // The open host-offered posture is deferred, so `open` is reserved even
    // though it satisfies the alias grammar.
    let error = parse("name: x\ndescription: d\ntools:\n  open: true\n")
        .expect_err("the reserved `open` key must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert!(
        error.to_string().contains("reserved"),
        "the error must name the reservation: {error}"
    );
}

#[test]
fn tool_slot_aliases_must_match_the_alias_grammar() {
    for alias in ["1search", "has space", "has/slash", "has.dot"] {
        let yaml =
            format!("name: x\ndescription: d\ntools:\n  '{alias}': promptforge/web/search\n");
        let error = parse(&yaml).expect_err("a bad alias must be rejected");
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{alias}: {error}"
        );
    }
    // The length boundary: 64 characters pass, 65 fail.
    let longest_ok = format!("a{}", "b".repeat(63));
    let too_long = format!("a{}", "b".repeat(64));
    let yaml = format!("name: x\ndescription: d\ntools:\n  {longest_ok}: promptforge/web/search\n");
    parse(&yaml).expect("a 64-character alias must parse");
    let yaml = format!("name: x\ndescription: d\ntools:\n  {too_long}: promptforge/web/search\n");
    parse(&yaml).expect_err("a 65-character alias must be rejected");
}

#[test]
fn args_declarations_round_trip() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "args:\n",
        "  use_mcp:\n",
        "    type: boolean\n",
        "    default: true\n",
        "    description: Search private sources\n",
        "  limit:\n",
        "    type: integer\n",
        "    optional: true\n",
        "  query:\n",
        "    type: string\n",
    ))
    .expect("args declarations must parse");
    let args = prompt.frontmatter().args();
    assert_eq!(args.len(), 3);
    let limit = args.get("limit").expect("declared");
    assert_eq!(limit.kind(), ArgType::Integer);
    assert!(limit.is_optional());
    assert_eq!(limit.description(), None);
    assert_eq!(args.get("query").expect("declared").kind(), ArgType::String);
}

#[test]
fn an_arg_default_must_match_the_declared_type() {
    for (kind, default) in [("boolean", "'true'"), ("integer", "1.5"), ("string", "42")] {
        let yaml = format!(
            "name: x\ndescription: d\nargs:\n  flag:\n    type: {kind}\n    default: {default}\n"
        );
        let error = parse(&yaml).expect_err("a mismatched default must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{kind}: {error}");
    }
    // A matching default passes.
    let yaml = "name: x\ndescription: d\nargs:\n  limit:\n    type: integer\n    default: 3\n";
    let prompt = parse(yaml).expect("a matching default must parse");
    assert_eq!(
        prompt
            .frontmatter()
            .args()
            .get("limit")
            .and_then(|a| a.default().cloned()),
        Some(serde_yaml_ng::Value::Number(3.into()))
    );
}

#[test]
fn an_arg_name_must_match_the_alias_grammar() {
    let yaml = "name: x\ndescription: d\nargs:\n  '1bad':\n    type: string\n";
    let error = parse(yaml).expect_err("a bad arg name must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn an_absent_args_key_marks_the_default_declaration() {
    // No `args:` key: the implicit default declaration, whose interface
    // prose wraps into `argv.prose`.
    let prompt = parse("name: x\ndescription: d\n").expect("no args key parses");
    assert!(
        prompt.frontmatter().args().is_default(),
        "an absent args key yields the default declaration"
    );

    // An explicit `args:` key is a structured declaration, which never
    // wraps.
    let prompt = parse("name: x\ndescription: d\nargs:\n  query:\n    type: string\n")
        .expect("explicit args parse");
    assert!(!prompt.frontmatter().args().is_default());

    // Even an explicit declaration identical to the default shape is
    // structured: declared is not defaulted.
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "args:\n",
        "  prose:\n",
        "    type: string\n",
        "    optional: true\n",
        "    description: Freeform input for this prompt\n",
    ))
    .expect("a default-shaped explicit declaration parses");
    let args = prompt.frontmatter().args();
    assert!(!args.is_default());
    assert_ne!(
        *args,
        super::ArgsDecl::default(),
        "an explicit declaration never equals the implicit default"
    );
}

#[test]
fn an_unknown_arg_type_is_rejected() {
    let yaml = "name: x\ndescription: d\nargs:\n  flag:\n    type: text\n";
    let error = parse(yaml).expect_err("an unknown arg type must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn model_roles_round_trip_with_defaults() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "models:\n",
        "  analyst:\n",
        "    keywords: [no-thinking, creative, chat]\n",
        "    min_context: 32000\n",
        "    description: deep reasoning\n",
        "  spare: {}\n",
    ))
    .expect("model roles must parse");
    let roles = prompt.frontmatter().models();
    assert_eq!(roles.len(), 2);
    let analyst = roles.get("analyst").expect("declared");
    assert_eq!(
        analyst.keywords(),
        &[
            ModelKeyword::NoThinking,
            ModelKeyword::Creative,
            ModelKeyword::Chat
        ]
    );
    assert_eq!(analyst.min_context(), NonZeroU32::new(32_000));
    let spare = roles.get("spare").expect("declared");
    assert!(spare.keywords().is_empty());
    assert_eq!(spare.min_context(), None);
    assert_eq!(spare.description(), None);
}

#[test]
fn an_unknown_model_keyword_is_a_parse_error() {
    // The keyword vocabulary is closed; `multimodal` is a non-goal and a
    // typo must fail at parse rather than being silently ignored.
    let error = parse("name: x\ndescription: d\nmodels:\n  analyst:\n    keywords: [multimodal]\n")
        .expect_err("an unknown keyword must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn model_role_labels_must_match_the_alias_grammar() {
    let yaml = "name: x\ndescription: d\nmodels:\n  '1analyst':\n    keywords: [fast]\n";
    let error = parse(yaml).expect_err("a bad role label must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_zero_min_context_is_rejected() {
    let error = parse("name: x\ndescription: d\nmodels:\n  analyst:\n    min_context: 0\n")
        .expect_err("a zero context minimum must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn contract_errors_report_their_frontmatter_line_and_column() {
    // Step 6: the retained serde_yaml_ng location surfaces on the parse
    // error, so a rejection inside a contract key points at its own line
    // and column instead of being a bare message.
    let src = concat!(
        "---\n",                     // line 1
        "name: x\n",                 // line 2
        "description: d\n",          // line 3
        "capabilities:\n",           // line 4
        "  - not a capability id\n", // line 5: the offending scalar
        "---\n",
        "\n# T\n\n## S\n\np\n",
    );
    let error = Prompt::parse(src, "test")
        .0
        .expect_err("a capability id with spaces must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert_eq!(
        error.line(),
        Some(5),
        "the offending scalar's line: {error}"
    );
    assert_eq!(
        error.column(),
        Some(5),
        "the offending scalar's column: {error}"
    );
    assert_eq!(
        error.name(),
        None,
        "a frontmatter failure predates the prompt's name"
    );
}
