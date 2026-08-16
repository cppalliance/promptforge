//! Fixtures shared by the catalog's unit tests: a prompts directory written to
//! a temporary location and a configuration rooted at it.

use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::error::CatalogError;

/// Writes `contents` to `relative` under `root`, creating parent directories.
pub(super) fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the fixture's parent directory");
    }
    fs::write(&path, contents).expect("write the fixture file");
}

/// A minimal prompt that runs offline: one section whose Lua returns at once.
pub(super) fn prompt_source(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\npromptforge: 1\n---\n\n\
         # Title\n\n## Main\n\n```lua\nreturn args\n```\n"
    )
}

/// A file that declares `promptforge:`, so it is a prompt, but whose
/// frontmatter is short a required field, so nothing in it declares a name.
pub(super) fn unparsable_source() -> &'static str {
    "---\npromptforge: 1\nname: placeholder\n---\n\n## S\n\np\n"
}

/// Writes a valid prompt at `relative`.
pub(super) fn write_prompt(root: &Path, relative: &str, name: &str, description: &str) {
    write(root, relative, &prompt_source(name, description));
}

/// A configuration rooted at `root`, with `extra` appended verbatim.
///
/// The root is rendered through the TOML serializer rather than interpolated
/// raw, so a temporary directory whose path holds a backslash (every Windows
/// path) or an apostrophe cannot produce malformed TOML and fail the test for a
/// reason unrelated to the behavior under test.
pub(super) fn config_at(root: &Path, extra: &str) -> Config {
    let prompts = toml_string(&root.display().to_string());
    let toml = format!(
        "[server]\napi_key = \"shared\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = {prompts}\n\n{extra}",
    );
    Config::from_toml_str(&toml).expect("the fixture configuration parses")
}

/// Renders `value` as a properly quoted and escaped TOML basic string.
pub(super) fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

/// Every fault's `Display`, joined, for a test that only cares that a message
/// names something.
pub(super) fn fault_text(error: &CatalogError) -> String {
    error.to_string()
}
