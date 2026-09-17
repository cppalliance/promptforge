mod client;
mod server;
mod shared;

#[cfg(test)]
mod tests;

pub(in crate::realtime) use client::parse_client_event;
#[expect(
    unused_imports,
    reason = "private wire surface is consumed by later realtime steps"
)]
pub(in crate::realtime) use server::{
    ConversationItem, DurationUsage, EffectiveSession, ServerEvent, WireError,
};
pub(in crate::realtime) use shared::{ClientError, ClientEvent, IdGenerator};
