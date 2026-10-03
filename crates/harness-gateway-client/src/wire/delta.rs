//! The live pieces of a streamed reply, as the read loop hands them out.

/// One live piece of a streamed reply.
///
/// The read loop hands each one to its caller's callback as the stream
/// arrives: answer text and the reasoning side channel stay separated so
/// a caller can render them differently. Tool-call fragments are never
/// handed out; they buffer inside the reader until the batch is complete
/// and validated, and arrive only in the finished completion.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamDelta {
    /// A fragment of the assistant's answer text.
    Text(String),
    /// A fragment of the reasoning side channel, never part of the answer.
    Reasoning(String),
}
