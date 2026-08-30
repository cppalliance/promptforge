//! The PromptForge gateway config UI: the embedded SPA assets and the
//! shared loopback middleware.
//!
//! The gateway pulls this crate as an optional dependency behind its
//! `config-ui` feature and nests [`routes`] at `/config`, so the SPA is
//! served on the gateway's own port with no second listener. The asset
//! routes carry no bearer auth - the SPA shell holds no secrets - but
//! they are wrapped in [`require_loopback`], the single shared loopback
//! check the gateway also applies to its admin config endpoints. The
//! check itself lives in the always-on `promptforge-gateway-loopback`
//! crate (re-exported here), so headless gateway builds carry the same
//! wall without compiling this crate's asset machinery.
//!
//! Debug builds read `ui/dist/` from disk at request time, so UI edits
//! need no Rust recompile; release builds embed the packaged, verified
//! artifact into the binary. The crate's build script produces `ui/dist/`
//! in both profiles.

mod assets;
mod routes;

pub use promptforge_gateway_loopback::require_loopback;
pub use routes::routes;

// The release artifact verifier lives outside src/ so build.rs shares it
// through the same `#[path]` mechanism; included here only to run its
// tests under `cargo test`.
#[cfg(test)]
#[path = "../build/manifest.rs"]
mod build_manifest;
