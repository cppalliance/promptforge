//! The chat-completions protocol vocabulary: what a model round exchanges,
//! with no transport attached.
//!
//! The wire types ([`Message`], [`ToolSchema`], [`ToolCall`],
//! [`Completion`], [`CompletionResult`]) go out of the Engine in a `Chat`
//! effect and come back in its answer. Their validating constructors run
//! the neutral reply checks, so a completion a wire decoder built and one
//! a Harness built by hand are judged alike.
//!
//! Nothing here opens a connection, reads a clock, or parses a provider's
//! wire format. The OpenAI wire code that builds the request body and
//! reads the streamed reply lives in `harness-gateway-client`. The Engine
//! itself never performs a round: a model round is a `Chat` effect the
//! Harness performs and answers.

mod wire;

pub use wire::{
    Completion, CompletionResult, Message, RawExchange, ToolArguments, ToolCall, ToolSchema,
    ToolSchemaError,
};

#[cfg(test)]
mod tests;
