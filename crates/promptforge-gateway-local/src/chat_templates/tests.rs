//! Catalog metadata, hash precedence, and Jinja2 differential oracles.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io;

use minijinja::value::{Kwargs, Value};
use minijinja::{Environment, Error, ErrorKind};
use serde::{Deserialize, Serialize as _};
use serde_json::ser::{Formatter, PrettyFormatter, Serializer};

use super::*;

#[derive(Debug, Deserialize)]
struct GoldenFixture {
    family: String,
    context_json: String,
    reference_output: String,
    reference_output_utf8_byte_count: usize,
    reference_output_utf8_sha256: String,
    template_utf8_sha256: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleContext {
    add_generation_prompt: bool,
    bos_token: String,
    date_string: String,
    enable_thinking: bool,
    eos_token: String,
    messages: Vec<OracleMessage>,
    preserve_thinking: bool,
    reasoning_effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OracleTool>>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleMessage {
    content: serde_json::Value,
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OracleToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleToolCall {
    function: OracleToolCallFunction,
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleToolCallFunction {
    name: String,
    arguments: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleTool {
    #[serde(rename = "type")]
    kind: String,
    function: OracleToolDefinition,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleToolDefinition {
    name: String,
    description: String,
    parameters: OracleToolParameters,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleToolParameters {
    #[serde(rename = "type")]
    kind: String,
    properties: BTreeMap<String, OracleToolProperty>,
    required: Vec<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct OracleToolProperty {
    #[serde(rename = "type")]
    kind: String,
    description: String,
}

struct PythonCompactFormatter;

impl Formatter for PythonCompactFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(b": ")
    }
}

fn template_error(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, message.into())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "minijinja filters require owned dynamic arguments"
)]
fn python_tojson(value: Value, kwargs: Kwargs) -> Result<String, Error> {
    let indent: Option<usize> = kwargs.get("indent")?;
    kwargs.assert_all_used()?;
    if indent.is_some_and(|width| width > 16) {
        return Err(template_error("tojson indent exceeds 16"));
    }

    let mut bytes = Vec::new();
    if let Some(width) = indent {
        let indentation = vec![b' '; width];
        let formatter = PrettyFormatter::with_indent(&indentation);
        let mut serializer = Serializer::with_formatter(&mut bytes, formatter);
        value
            .serialize(&mut serializer)
            .map_err(|error| template_error(error.to_string()))?;
    } else {
        let mut serializer = Serializer::with_formatter(&mut bytes, PythonCompactFormatter);
        value
            .serialize(&mut serializer)
            .map_err(|error| template_error(error.to_string()))?;
    }
    String::from_utf8(bytes).map_err(|error| template_error(error.to_string()))
}

fn oracle_environment() -> Environment<'static> {
    let mut environment = Environment::new();
    environment.set_trim_blocks(true);
    environment.set_lstrip_blocks(true);
    environment.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    environment.add_filter("tojson", python_tojson);
    environment.add_function(
        "raise_exception",
        |message: String| -> Result<String, Error> { Err(template_error(message)) },
    );
    environment.add_function("strftime_now", |format: String| -> Result<String, Error> {
        if format == "%Y-%m-%d" {
            Ok("2026-08-30".to_owned())
        } else {
            Err(template_error(format!(
                "unsupported pinned strftime format `{format}`"
            )))
        }
    });
    environment
}

fn golden_fixtures() -> Vec<GoldenFixture> {
    serde_json::from_str(include_str!("fixtures/golden.json"))
        .expect("generated golden fixtures must be valid JSON")
}

fn sha256(value: &str) -> String {
    overrides::sha256_hex(value)
}

#[test]
fn every_documented_alias_resolves() {
    for family in Family::ALL {
        for alias in family.aliases() {
            assert_eq!(Family::parse_alias(alias), Some(family), "{alias}");
            assert_eq!(alias.parse::<Family>(), Ok(family), "{alias}");
            assert_eq!(
                Family::parse_alias(&alias.to_ascii_uppercase()),
                Some(family),
                "{alias}"
            );
        }
    }
    assert!(matches!(
        "not-a-family".parse::<Family>(),
        Err(ParseFamilyError::Unknown { .. })
    ));
}

#[test]
fn family_metadata_matches_the_catalog() {
    assert_eq!(Family::ALL.len(), 12);
    let mut snapshot = String::new();
    for family in Family::ALL {
        writeln!(
            &mut snapshot,
            "{}|{}|{}|{}",
            family.canonical_name(),
            family.aliases().join(","),
            family.default_system_message().unwrap_or("<none>"),
            family.stop_tokens().join(",")
        )
        .expect("writing to a String cannot fail");
    }
    assert_eq!(
        sha256(&snapshot),
        "b21d0ee322ce65116d5585264b17f3a9e393acd3564248fe7c4e347a58455d4b"
    );
}

#[test]
fn every_asset_is_exact_and_has_a_generation_prompt() {
    let expected = [
        (
            Family::Chatml,
            "3e3e4ccd01b95ae100e861475e34dd42d044d15858f876bb73fe2934fe1603db",
        ),
        (
            Family::Llama3,
            "99195f2f7dc1344662663119562b46fc067034c0ea660cf1cab333617c7290c2",
        ),
        (
            Family::Llama31,
            "dd2b02f1d93dc00f824fcb75186806a1af3333d9e1069ed6be8c8bdbfde0fd6a",
        ),
        (
            Family::Qwen25,
            "3aa2ec4c84d331ed2c6efca1ed25f6859c9f86ffcda59fe36b14691896aa9c13",
        ),
        (
            Family::Qwen3,
            "f2e03cfdf930fb26e7c88ca83abc3a0c1e7c5b1fe68ee43ba81d0c47c8460ceb",
        ),
        (
            Family::Gemma3,
            "3cb7c5da795557bca16f7dfc2c8784d575870b51b1b00f8405896498fccdea3c",
        ),
        (
            Family::Gemma4,
            "88c645d0098a8f6f0607f0e228cf29114f4739605b8c31fbad3b8eb84e7d7491",
        ),
        (
            Family::Mistral,
            "e5ca69487bf046d5c2ad1dd3889de62c98b8cfd5801fcd561d364c1041d4952e",
        ),
        (
            Family::Phi3,
            "6fc812cb52be39ba92df85609cace4d52790c4d2fc6d14e8c8eeee3fab908138",
        ),
        (
            Family::Phi4,
            "fe4120415a3858b873955c92ce3a103a95bc97c2cb00b72e62659aaf0f891ee5",
        ),
        (
            Family::GptOss,
            "cf69ff19b5e663c88df6583d90b612f7d9b72c3dfd4ef5bbe29eaa6e41decda3",
        ),
        (
            Family::Zephyr,
            "5e2833b50f34f4581d09d801325bf4d7d8285bcad36be595a30a9904b7759f5d",
        ),
    ];
    for (family, expected_hash) in expected {
        let generation_marker = if family == Family::Mistral {
            "[/INST]"
        } else {
            "add_generation_prompt"
        };
        assert!(!family.template().is_empty(), "{}", family.canonical_name());
        assert!(
            family.template().contains(generation_marker),
            "{}",
            family.canonical_name()
        );
        assert_eq!(
            sha256(family.template()),
            expected_hash,
            "{}",
            family.canonical_name()
        );
    }
}

#[test]
fn mapper_is_sorted_exact_and_case_insensitive() {
    assert!(
        mapper_data::MODEL_FAMILIES
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0)
    );
    assert_eq!(mapper_data::MODEL_FAMILIES.len(), 181);
    let mut snapshot = String::new();
    for (model, family) in mapper_data::MODEL_FAMILIES {
        writeln!(&mut snapshot, "{model}={}", family.canonical_name())
            .expect("writing to a String cannot fail");
    }
    assert_eq!(
        sha256(&snapshot),
        "6535d8fa629b0a89e2ec53e3c9d924e3bd226d5844cf389d2c04ae6d18616fb8"
    );
    for (model, family) in mapper_data::MODEL_FAMILIES {
        assert_eq!(family_for_model(model), Some(*family), "{model}");
        assert_eq!(
            family_for_model(&model.to_ascii_uppercase()),
            Some(*family),
            "{model}"
        );
    }
    assert_eq!(family_for_model("Qwen/Qwen3-8B"), Some(Family::Qwen3));
    assert_eq!(family_for_model("Qwen3-8B"), None);
}

#[test]
fn known_overrides_are_hash_first_for_all_current_models() {
    let edge = include_str!("fixtures/broken-gemma-4-edge.jinja");
    let standard = include_str!("fixtures/broken-gemma-4-standard.jinja");
    let current = [
        ("unsloth/gemma-4-12b-it-GGUF", standard),
        ("unsloth/gemma-4-12B-it-qat-GGUF", standard),
        ("unsloth/gemma-4-26B-A4B-it-GGUF", standard),
        ("unsloth/gemma-4-26B-A4B-it-qat-GGUF", standard),
        ("unsloth/gemma-4-31B-it-GGUF", standard),
        ("unsloth/gemma-4-31B-it-qat-GGUF", standard),
        ("unsloth/gemma-4-E2B-it-GGUF", edge),
        ("unsloth/gemma-4-E2B-it-qat-GGUF", edge),
        ("unsloth/gemma-4-E2B-it-qat-mobile-GGUF", edge),
        ("unsloth/gemma-4-E4B-it-GGUF", edge),
        ("unsloth/gemma-4-E4B-it-qat-GGUF", edge),
        ("unsloth/gemma-4-E4B-it-qat-mobile-GGUF", edge),
    ];
    for (model, embedded) in current {
        let resolved = known_override(Some(embedded), Some(model))
            .expect("a revision-pinned broken template must resolve");
        let expected_asset = if std::ptr::eq(embedded, edge) {
            "gemma-4-edge.jinja"
        } else {
            "gemma-4-standard.jinja"
        };
        assert_eq!(resolved.asset_name, expected_asset, "{model}");
    }

    let hash_wins = known_override(Some(edge), Some("unsloth/gemma-4-31B-it-GGUF"))
        .expect("known hash must resolve");
    assert_eq!(hash_wins.asset_name, "gemma-4-edge.jinja");
}

#[test]
fn future_model_ids_are_only_the_secondary_override_fallback() {
    assert_eq!(
        known_override(None, Some("gemma-4-E2B-it-GGUF")).map(|item| item.asset_name),
        Some("gemma-4-edge.jinja")
    );
    for model in [
        "unsloth/gemma-4-E2B-it-qat-GGUF",
        "unsloth/gemma-4-E2B-it-qat-mobile-GGUF",
        "unsloth/gemma-4-E4B-it-qat-GGUF",
        "unsloth/gemma-4-E4B-it-qat-mobile-GGUF",
    ] {
        assert_eq!(
            known_override(None, Some(model)).map(|item| item.asset_name),
            Some("gemma-4-edge.jinja"),
            "{model}"
        );
    }
    assert_eq!(
        known_override(None, Some("unsloth/gemma-4-99B-it-GGUF")).map(|item| item.asset_name),
        Some("gemma-4-standard.jinja")
    );
    assert!(known_override(None, Some("unsloth/gemma-4--GGUF")).is_none());
    assert!(known_override(Some("unknown"), Some("google/gemma-4-e2b-it")).is_none());
}

#[test]
fn override_assets_and_broken_hashes_are_exact() {
    assert_eq!(KNOWN_OVERRIDES.len(), 2);
    assert_eq!(
        sha256(KNOWN_OVERRIDES[0].template),
        "3cbe8a665e97cdc51af3f3b6f7f59078cffdccccbce528464269ae5f348d2295"
    );
    assert_eq!(
        sha256(KNOWN_OVERRIDES[1].template),
        "a9b99f2b1cdc4982b1430ce1dae84f0c88aef8b76e5ea613e64107228fce218d"
    );
    assert_eq!(
        sha256(include_str!("fixtures/broken-gemma-4-edge.jinja")),
        KNOWN_OVERRIDES[0].embedded_template_sha256
    );
    assert_eq!(
        sha256(include_str!("fixtures/broken-gemma-4-standard.jinja")),
        KNOWN_OVERRIDES[1].embedded_template_sha256
    );
}

#[test]
fn validated_llama_cpp_range_is_the_pinned_build() {
    assert_eq!(VALIDATED_LLAMA_CPP_BUILDS.first, "b10082");
    assert_eq!(VALIDATED_LLAMA_CPP_BUILDS.last, "b10082");
}

#[test]
fn all_twelve_golden_renders_are_byte_identical() {
    let fixtures = golden_fixtures();
    assert_eq!(fixtures.len(), Family::ALL.len());
    let mut seen = HashSet::new();

    for fixture in fixtures {
        let family = fixture
            .family
            .parse::<Family>()
            .expect("golden fixture family must be cataloged");
        assert!(seen.insert(family), "duplicate fixture {}", fixture.family);
        assert_eq!(
            sha256(family.template()),
            fixture.template_utf8_sha256,
            "{} template",
            fixture.family
        );

        let mut environment = oracle_environment();
        environment
            .add_template(family.canonical_name(), family.template())
            .expect("catalog template must compile in the oracle");
        let template = environment
            .get_template(family.canonical_name())
            .expect("the just-added template must exist");
        let context: OracleContext =
            serde_json::from_str(&fixture.context_json).expect("golden context must be valid JSON");
        let actual = template
            .render(&context)
            .expect("golden context must render");

        assert_eq!(
            actual.as_bytes(),
            fixture.reference_output.as_bytes(),
            "{} output",
            fixture.family
        );
        assert_eq!(
            actual.len(),
            fixture.reference_output_utf8_byte_count,
            "{} length",
            fixture.family
        );
        assert_eq!(
            sha256(&actual),
            fixture.reference_output_utf8_sha256,
            "{} hash",
            fixture.family
        );
    }
}

#[test]
fn oracle_tojson_refuses_unbounded_indent_without_panicking() {
    let mut environment = oracle_environment();
    environment
        .add_template("bounded", "{{ value|tojson(indent=17) }}")
        .expect("the bound-check fixture compiles");
    let template = environment
        .get_template("bounded")
        .expect("the just-added template must exist");
    let error = template
        .render(minijinja::context!(value => [1, 2, 3]))
        .expect_err("indent above the cap must fail");
    assert!(error.to_string().contains("tojson indent exceeds 16"));
}
