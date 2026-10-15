//! Tests for the frontmatter contract keys: `plugins`, `args`, and
//! `models`.

use std::num::NonZeroU32;

use super::{ArgType, ModelKeyword};
use crate::ParseErrorKind;
use crate::test_support::{assert_parse_error, parse_err, parse_ok, prompt};

#[path = "tests-plugins.rs"]
mod plugins;

#[test]
fn the_full_contract_declaration_parses_and_round_trips() {
    let prompt = parse_ok(&prompt(
        "
        plugins: [web, mcp]
        args: { use_mcp: { type: boolean, default: true, \
                description: Search MCP-connected private sources } }
        models: { analyst: { keywords: [frontier, thinking], min_context: 200000, \
                  description: deep reasoning }, \
                  triage: { keywords: [fast, small], description: quick triage of search results } }
        ",
        &[],
    ));

    let fm = prompt.frontmatter();

    let plugins = fm.plugins();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].to_string(), "web");
    assert_eq!(plugins[1].to_string(), "mcp");

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
    let prompt = parse_ok(&prompt("", &[]));
    let fm = prompt.frontmatter();
    assert!(fm.plugins().is_empty());
    assert!(fm.models().is_empty());
}

#[test]
fn an_omitted_args_key_yields_the_default_prose_declaration() {
    // There are no freeform prompts: an absent `args:` key is the default
    // declaration of one optional string field named `prose`.
    let prompt = parse_ok(&prompt("", &[]));
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
    for front in [
        "args: { flag: { tipe: boolean } }",
        "models: { analyst: { keyword: [fast] } }",
    ] {
        let error = parse_err(&prompt(front, &[]));
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    }
}

#[test]
fn every_reserved_name_the_grammar_allows_parses_as_a_model_role_label() {
    for (name, _) in promptforge_lua::RESERVED_NAMES {
        if name.starts_with('_') {
            continue;
        }
        let prompt = parse_ok(&prompt(&format!("models: {{ '{name}': {{}} }}"), &[]));
        assert!(prompt.frontmatter().models().get(name).is_some(), "{name}");
    }
}

#[test]
fn an_arg_name_may_be_a_reserved_name_because_args_are_argv_fields() {
    let front =
        "args: { prose: { type: string }, store: { type: string }, end: { type: boolean } }";
    let prompt = parse_ok(&prompt(front, &[]));
    let args = prompt.frontmatter().args();
    assert_eq!(args.len(), 3);
    assert!(args.get("store").is_some() && args.get("end").is_some());
}

#[test]
fn args_declarations_round_trip() {
    let prompt = parse_ok(&prompt(
        "args: { use_mcp: { type: boolean, default: true, description: Search private sources }, \
         limit: { type: integer, optional: true }, query: { type: string } }",
        &[],
    ));
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
        let front = format!("args: {{ flag: {{ type: {kind}, default: {default} }} }}");
        let error = parse_err(&prompt(&front, &[]));
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{kind}: {error}");
    }
    // A matching default passes.
    let prompt = parse_ok(&prompt(
        "args: { limit: { type: integer, default: 3 } }",
        &[],
    ));
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
    let error = parse_err(&prompt("args: { '1bad': { type: string } }", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn an_absent_args_key_marks_the_default_declaration() {
    // No `args:` key: the implicit default declaration, whose interface
    // prose wraps into `argv.prose`.
    assert!(
        parse_ok(&prompt("", &[])).frontmatter().args().is_default(),
        "an absent args key yields the default declaration"
    );

    // An explicit `args:` key is a structured declaration, which never
    // wraps.
    let explicit = parse_ok(&prompt("args: { query: { type: string } }", &[]));
    assert!(!explicit.frontmatter().args().is_default());

    // Even an explicit declaration identical to the default shape is
    // structured: declared is not defaulted.
    let shaped = parse_ok(&prompt(
        "args: { prose: { type: string, optional: true, \
         description: Freeform input for this prompt } }",
        &[],
    ));
    let args = shaped.frontmatter().args();
    assert!(!args.is_default());
    assert_ne!(
        *args,
        super::ArgsDecl::default(),
        "an explicit declaration never equals the implicit default"
    );
}

#[test]
fn an_unknown_arg_type_is_rejected() {
    let error = parse_err(&prompt("args: { flag: { type: text } }", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn model_roles_round_trip_with_defaults() {
    let prompt = parse_ok(&prompt(
        "models: { analyst: { keywords: [no-thinking, creative, chat], min_context: 32000, \
         description: deep reasoning }, spare: {} }",
        &[],
    ));
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
    let error = parse_err(&prompt(
        "models: { analyst: { keywords: [multimodal] } }",
        &[],
    ));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn model_role_labels_must_match_the_alias_grammar() {
    let error = parse_err(&prompt("models: { '1analyst': { keywords: [fast] } }", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_zero_min_context_is_rejected() {
    let error = parse_err(&prompt("models: { analyst: { min_context: 0 } }", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn contract_errors_report_their_frontmatter_line_and_column() {
    // The retained serde_yaml_ng location surfaces on the parse
    // error, so a rejection inside a contract key points at its own line
    // and column instead of being a bare message.
    let src = concat!(
        "---\n",                 // line 1
        "name: x\n",             // line 2
        "description: d\n",      // line 3
        "plugins:\n",            // line 4
        "  - not a Plugin id\n", // line 5: the offending scalar
        "---\n",
        "\n# T\n\n## S\n\np\n",
    );
    let error = parse_err(src);
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
