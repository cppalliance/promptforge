//! The chat-completions message role and its parse error.

use std::fmt;
use std::str::FromStr;

/// Names who speaks one message in a chat-completions conversation.
///
/// It serializes as its lowercase wire name: `system`, `user`, `assistant`,
/// or `tool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    /// Instructions that frame the conversation.
    System,
    /// A turn from the user.
    User,
    /// A turn from the model.
    Assistant,
    /// The result of a tool call the model requested.
    Tool,
}

impl Role {
    /// Lists every role, in declaration order.
    pub const ALL: &'static [Role] = &[Role::System, Role::User, Role::Assistant, Role::Tool];

    /// Returns the role's wire name: `system`, `user`, `assistant`, or
    /// `tool`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = ParseRoleError;

    /// Parses a role from its exact wire name, never case-folded.
    fn from_str(name: &str) -> Result<Role, ParseRoleError> {
        Role::ALL
            .iter()
            .copied()
            .find(|role| role.as_str() == name)
            .ok_or_else(|| ParseRoleError {
                input: name.to_owned(),
            })
    }
}

/// The error returned when text is not a [`Role`]'s wire name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRoleError {
    /// The rejected text.
    input: String,
}

impl ParseRoleError {
    /// Returns the rejected text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for ParseRoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid role `{}`: required one of ", self.input)?;
        for (position, role) in Role::ALL.iter().enumerate() {
            if position > 0 {
                f.write_str(", ")?;
            }
            write!(f, "`{role}`")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseRoleError {}
