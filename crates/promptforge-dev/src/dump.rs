//! Turn traces for the interactive prompt runner.
//!
//! When raw capture is authorized, [`TraceCapture`] queues each model turn to a
//! worker thread that writes restricted, atomic JSON files under
//! `<store-dir>/.trace/`. Every write goes through [`fs_safe`], which creates
//! owner-only files and directories, refuses to follow a symlink or reparse
//! point at any component, and writes atomically.

mod fs_safe;
mod trace_capture;

pub(crate) use self::trace_capture::{SensitiveCapture, TraceCapture};
