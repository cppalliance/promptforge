//! A run's seed and the values derived from it.
//!
//! The caller draws a run's [`RunSeed`] from a CSPRNG for a live run and
//! passes the recorded one back for a replay. Every value derived from the
//! seed goes through [`derive()`] under its own `promptforge.<purpose>.v1`
//! tag: the guard nonce
//! ([`GuardNonce::from_seed`](crate::untrusted::GuardNonce::from_seed)) and
//! the run id ([`RunUid`]). Each derivation is one-way, so a derived value
//! can be shown without revealing the seed or any other value derived from
//! it.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

/// The domain tag the run id is derived under.
const RUN_UID_DOMAIN: &[u8] = b"promptforge.run-uid.v1";

/// What a run seed's text form must be, for a refusal to name.
const REQUIRED: &str = "a run seed is 64 lowercase hex characters";

/// Holds the 32 bytes the caller supplies to seed a run.
///
/// Every value derived for the run comes from the seed. `Display` and serde
/// use 64 lowercase hex characters. `Debug` prints `RunSeed(..)` with no
/// bytes, so a trace, panic, or test failure never shows the seed, from
/// which anyone could compute the run's nonce.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RunSeed([u8; 32]);

impl RunSeed {
    /// Returns the seed whose bytes are `bytes`.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> RunSeed {
        RunSeed(bytes)
    }

    /// Returns the seed's 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for RunSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}

impl fmt::Debug for RunSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RunSeed(..)")
    }
}

impl Serialize for RunSeed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RunSeed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_hex(&text)
            .map(RunSeed)
            .map_err(serde::de::Error::custom)
    }
}

/// SHA-256 over `domain`, then `seed`'s bytes.
///
/// Every value derived from a run's seed comes from here under its own
/// `promptforge.<purpose>.v1` tag, so no two purposes share a digest. The
/// derivation is part of the run's replay contract and never changes
/// silently.
#[must_use]
pub fn derive(domain: &[u8], seed: &RunSeed) -> [u8; 32] {
    let digest = Sha256::new()
        .chain_update(domain)
        .chain_update(seed.0)
        .finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Identifies a run by 32 bytes derived from its seed.
///
/// The derivation is tagged `promptforge.run-uid.v1`. The id is never the
/// seed itself, so it can appear in logs, records, and views without
/// revealing the seed or the nonce. A replay passes back the recorded seed,
/// so it reproduces the id. `Display` renders 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunUid([u8; 32]);

impl RunUid {
    /// The id of the run whose seed is `seed`.
    #[must_use]
    pub fn from_seed(seed: &RunSeed) -> RunUid {
        RunUid(derive(RUN_UID_DOMAIN, seed))
    }
}

impl fmt::Display for RunUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}

impl fmt::Debug for RunUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RunUid(")?;
        write_hex(f, &self.0)?;
        f.write_str(")")
    }
}

/// Writes `bytes` as lowercase hex.
fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8; 32]) -> fmt::Result {
    bytes.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
}

/// The 32 bytes `text` spells in 64 lowercase hex characters, or a refusal
/// naming the first character outside that form or the actual length.
fn parse_hex(text: &str) -> Result<[u8; 32], String> {
    if let Some((at, found)) = text
        .char_indices()
        .find(|(_, c)| !matches!(c, '0'..='9' | 'a'..='f'))
    {
        return Err(format!("{REQUIRED}, found {found:?} at byte {at}"));
    }
    if text.len() != 64 {
        return Err(format!("{REQUIRED}, found {} characters", text.len()));
    }
    let mut bytes = [0u8; 32];
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = (nibble(*high) << 4) | nibble(*low);
    }
    Ok(bytes)
}

/// The value of one lowercase hex digit `parse_hex` has already checked.
const fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        _ => digit - b'a' + 10,
    }
}

#[cfg(test)]
#[path = "seed-tests.rs"]
mod tests;
