//! Agent discovery: the `.md` agent programs under a configured directory,
//! the built-in `chat` agent embedded at compile time, and the shadowing
//! rule between them.

use std::io;
use std::path::Path;

/// The committed built-in chat agent, embedded at compile time - the same
/// shipped-asset pattern as the SPA `dist/` - so a fresh install has a
/// working chat with no agents directory at all. The built-in is a
/// Markdown prompt on the unified runtime.
pub const BUILTIN_CHAT_SOURCE: &str = include_str!("../agents/chat.md");

/// The built-in default agent's name: discovery always offers it, and a
/// directory file named `chat.md` shadows the embedded source.
const BUILTIN_CHAT_NAME: &str = "chat";

/// One agent's program source: a Markdown prompt document on the
/// unified runtime. Directory agents and the embedded built-in chat are
/// both Markdown; the standalone Lua agent path is retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSource {
    /// A Markdown prompt document (the unified runtime).
    Markdown(String),
}

/// Lists the launchable agent names: the `.md` file stems under `dir`
/// plus the built-in `chat`, sorted. A missing or unreadable directory
/// offers only the built-in, and a directory `chat.md` lists once -
/// it shadows the embedded source instead of duplicating the name.
#[must_use]
pub fn discover_agents(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|extension| extension == "md")
        })
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect();
    if !names.iter().any(|name| name == BUILTIN_CHAT_NAME) {
        names.push(BUILTIN_CHAT_NAME.to_owned());
    }
    names.sort();
    names
}

/// Reads the agent's program source: the directory file when it exists -
/// a directory `chat.md` shadows the built-in - else the embedded
/// built-in for the `chat` name alone. A caller resolves `name` through
/// [`discover_agents`] first, so a missing file for any other name is a
/// real filesystem race, surfaced as the error it is; so is an existing
/// `chat.md` that cannot be read, because silently serving the built-in
/// would mask the operator's own file.
///
/// # Errors
/// Returns the filesystem error for any name other than the built-in,
/// and for the built-in when a directory `chat.md` exists but cannot be
/// read.
pub fn agent_source(dir: &Path, name: &str) -> io::Result<AgentSource> {
    match std::fs::read_to_string(dir.join(format!("{name}.md"))) {
        Ok(source) => Ok(AgentSource::Markdown(source)),
        Err(error) if name == BUILTIN_CHAT_NAME && error.kind() == io::ErrorKind::NotFound => {
            Ok(AgentSource::Markdown(BUILTIN_CHAT_SOURCE.to_owned()))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "discovery-tests.rs"]
mod tests;
