//! Agent discovery: the `.md` agent prompts in Workshop's agents folder,
//! the built-in `chat` agent embedded at compile time, the shadowing rule
//! between them, and the launch's read of one agent's source.

use std::io;
use std::path::Path;

/// The committed built-in chat agent, embedded at compile time, so a
/// fresh install has a working chat with no agents folder at all.
pub const BUILTIN_CHAT_SOURCE: &str = include_str!("../agents/chat.md");

/// The built-in default agent's name: discovery always offers it, and a
/// folder file named `chat.md` shadows the embedded source.
const BUILTIN_CHAT_NAME: &str = "chat";

/// A refused launch.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LaunchError {
    /// The requested name is not one the agent menu lists.
    #[error("unknown agent {name:?}: not in the agents directory")]
    UnknownAgent {
        /// The name that was requested.
        name: String,
    },
    /// The agent's program source could not be read.
    #[error("agent session state unavailable")]
    Unreadable {
        /// The underlying filesystem failure.
        #[source]
        source: io::Error,
    },
}

/// Lists the launchable agent names: the `.md` file stems under `dir`
/// plus the built-in `chat`, sorted. A missing or unreadable folder
/// offers only the built-in, and a folder `chat.md` lists once: it
/// shadows the embedded source instead of duplicating the name.
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

/// Reads the source of the agent `name`, accepting only a name the agent
/// menu lists: [`discover_agents`] over `dir` is the trust boundary, so a
/// client-sent name reaches the filesystem only as the bare stem of a
/// real `.md` file in `dir`, or as the built-in `chat`.
///
/// # Errors
/// Returns [`LaunchError::UnknownAgent`] for a name discovery does not
/// list, paths included, and [`LaunchError::Unreadable`] when a listed
/// agent's source cannot be read.
pub fn load_agent(dir: &Path, name: &str) -> Result<String, LaunchError> {
    if !discover_agents(dir).iter().any(|known| known == name) {
        return Err(LaunchError::UnknownAgent {
            name: name.to_owned(),
        });
    }
    agent_source(dir, name).map_err(|source| LaunchError::Unreadable { source })
}

/// Reads the agent's program source: the folder file when it exists - a
/// folder `chat.md` shadows the built-in - else the embedded built-in for
/// the `chat` name alone. A missing file for any other name is a real
/// filesystem race, surfaced as the error it is; so is an existing
/// `chat.md` that cannot be read, because silently serving the built-in
/// would mask the operator's own file.
fn agent_source(dir: &Path, name: &str) -> io::Result<String> {
    match std::fs::read_to_string(dir.join(format!("{name}.md"))) {
        Ok(source) => Ok(source),
        Err(error) if name == BUILTIN_CHAT_NAME && error.kind() == io::ErrorKind::NotFound => {
            Ok(BUILTIN_CHAT_SOURCE.to_owned())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "discovery-tests.rs"]
mod tests;
