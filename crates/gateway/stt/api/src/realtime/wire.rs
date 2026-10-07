//! Wire module root for the realtime client and server event protocol.

mod client;
mod server;
mod vocabulary;

#[cfg(test)]
mod tests;

pub(in crate::realtime) use client::parse_client_event;
#[expect(
    unused_imports,
    reason = "no code in the realtime module reads these re-exports"
)]
pub(in crate::realtime) use server::{
    ConversationItem, DurationUsage, EffectiveSession, HypothesisRanges, ServerEvent, WireError,
};
pub(in crate::realtime) use vocabulary::{
    ClientError, ClientEvent, HypothesisInclude, IdGenerator,
};
