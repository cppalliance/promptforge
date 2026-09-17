//! The PromptForge gateway config UI: the embedded SPA assets and the
//! shared loopback middleware.
//!
//! The gateway pulls this crate as an optional dependency behind its
//! `config-ui` feature and nests [`routes`] at `/config`, so the SPA is
//! served on the gateway's own port with no second listener. The asset
//! routes carry no bearer auth - the SPA shell holds no secrets - but
//! they are wrapped in [`require_loopback`], the single shared loopback
//! check the gateway also applies to its admin config endpoints. The
//! check itself lives in the always-on `shared-loopback`
//! crate (re-exported here), so headless gateway builds carry the same
//! wall without compiling this crate's asset machinery.
//!
//! Debug builds read the bundle from `$OUT_DIR/ui-dist/` at request time,
//! so UI edits need no Rust recompile; release builds embed the bundle
//! into the binary. The crate's build script produces the bundle in both
//! profiles.

mod assets;
mod routes;

pub use routes::routes;
pub use shared_loopback::require_loopback;
