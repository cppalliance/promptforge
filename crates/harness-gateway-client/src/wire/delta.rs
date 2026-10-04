//! The live pieces of a streamed reply, as the read loop hands them out.

/// One piece of a streamed model reply, delivered as it arrives.
///
/// The client passes each piece to the caller's callback as soon as it
/// decodes the piece from the stream. Answer text and reasoning arrive as
/// separate variants, so a caller can show them differently.
///
/// Tool-call fragments never arrive as pieces. The client buffers them
/// until the whole batch of tool calls is complete and validated. They
/// appear only in the finished completion.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamDelta {
    /// A fragment of the assistant's answer text.
    Text(String),
    /// A fragment of the model's reasoning text. It is never part of the
    /// answer.
    Reasoning(String),
}
