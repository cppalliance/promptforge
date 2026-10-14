//! Reading what a prompt declares before it runs: its frontmatter lists
//! every declaration with arguments sorted by name, and a role check by
//! hand agrees with prepare.

use std::num::NonZeroU32;

use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::prompt::ModelKeyword;
use promptforge::{ParseErrorKind, Prompt, RequirementCheck};

use super::support::{Prepared, Scenario, TestResult, prompt};

/// Declares two files, one Plugin, `times` then `name`, and one model
/// role.
const CONTRACT: &str = "
    input: { path: names.md, description: The names to greet }
    output: { path: note.md, description: The note the greeter leaves }
    plugins: [text]
    args: { times: { type: integer, optional: true, default: 1 }, name: { type: string } }
    models: { writer: { keywords: [fast] } }
";

#[test]
fn the_frontmatter_lists_every_declaration_with_arguments_sorted_by_name() -> TestResult {
    let source = prompt(CONTRACT, &[("Greet", "return 'done'")]);
    let parsed = Prompt::parse(&source, "greeter").0?;
    let frontmatter = parsed.frontmatter();
    assert_eq!(frontmatter.name(), "greeter");
    assert_eq!(
        frontmatter.description(),
        "A prompt the facade suite builds."
    );

    let files: Vec<_> = [frontmatter.input(), frontmatter.output()]
        .into_iter()
        .flatten()
        .map(|file| (file.path().to_string(), file.description().to_owned()))
        .collect();
    assert_eq!(
        files,
        [
            ("names.md".to_owned(), "The names to greet".to_owned()),
            (
                "note.md".to_owned(),
                "The note the greeter leaves".to_owned()
            ),
        ]
    );
    let plugins: Vec<_> = frontmatter
        .plugins()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(plugins, ["text"]);
    let roles: Vec<_> = frontmatter
        .models()
        .iter()
        .map(|(label, role)| (label.to_string(), role.min_context()))
        .collect();
    assert_eq!(roles, [("writer".to_owned(), None)]);

    assert!(
        frontmatter
            .args()
            .iter()
            .map(|(name, _)| name)
            .eq(["name", "times"])
    );

    let misspelled = source.replace("plugins:", "plugns:");
    let parsed = Prompt::parse(&misspelled, "greeter").0;
    assert_eq!(
        parsed.err().map(|error| error.kind()),
        Some(ParseErrorKind::Frontmatter)
    );
    Ok(())
}

#[test]
fn a_role_check_by_hand_finds_the_same_shortfall_prepare_reports() -> TestResult {
    let source = prompt(
        "models: { writer: { keywords: [fast] }, checker: { keywords: [no-thinking], min_context: 100000 } }",
        &[("Greet", "return 'done'")],
    );
    let window = NonZeroU32::new(200_000).ok_or("a context window is never zero")?;
    let id = ModelId::gateway("canned")?;
    let model = ModelDescriptor::new(id, "Thinks on every call", window, ThinkingMode::Always);
    let Prepared {
        prompt,
        requirements,
        ..
    } = Scenario::default().model(model.clone()).prepare(&source)?;

    let mut misses = Vec::new();
    for (label, role) in prompt.frontmatter().models().iter() {
        let keyword_miss = role.keywords().iter().any(|keyword| match keyword {
            ModelKeyword::Thinking => model.thinking() == ThinkingMode::Never,
            ModelKeyword::NoThinking => model.thinking() == ThinkingMode::Always,
            _ => false,
        });
        let context_miss = role.min_context().is_some_and(|min| min > model.context());
        if keyword_miss || context_miss {
            misses.push((label, keyword_miss, context_miss));
        }
    }
    assert_eq!(misses, [("checker", true, false)]);

    let [unmet] = requirements.unmet_requirements.as_slice() else {
        return Err("only the checker falls short".into());
    };
    assert_eq!(unmet.role, "checker");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    Ok(())
}
