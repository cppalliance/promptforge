//! Wire-adjacent vocabulary a host names in streaming callbacks.

/// One live increment from a streaming completion.
///
/// The client's completion call invokes its delta callback with these as the
/// stream arrives: answer text and the reasoning side channel stay separated
/// so a consumer can render them differently. Tool-call fragments are never
/// surfaced as deltas; they buffer inside the client until the batch is
/// complete and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamDelta {
    /// A fragment of the assistant's answer text.
    Text(String),
    /// A fragment of the reasoning side channel, never part of the answer.
    Reasoning(String),
}
