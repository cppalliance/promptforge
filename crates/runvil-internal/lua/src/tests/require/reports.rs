//! The library-load reports: a first load's start and end, nothing for a
//! cached call, and a fresh pair on every retry after a failure.

use super::*;

/// The library-load reports alone, each as its kind and library.
fn library_loads(recorder: &Recorder) -> Vec<(String, String)> {
    recorder
        .observations()
        .into_iter()
        .filter_map(|(_, observation)| match observation {
            Observation::Library { kind, library } => Some((kind, library)),
            _ => None,
        })
        .collect()
}

/// The `(kind, library)` record of one library-load report.
fn load(kind: &str, library: &str) -> (String, String) {
    (format!("lua_library_load_{kind}"), library.to_owned())
}

#[test]
fn a_first_load_reports_its_start_and_success_and_a_cached_call_nothing() {
    let recorder = Recorder::default();
    let (vm, _) = require_vm(
        vec![
            ("## a", None, library("a", "## a", &["require '## b'"])),
            ("## b", None, library("b", "## b", &["return 'b'"])),
        ],
        recorder.emitter(),
    );
    returned(&vm, "require '## a'\nrequire '## a'\nrequire '## b'");
    assert_eq!(
        library_loads(&recorder),
        [
            load("started", "## a"),
            load("started", "## b"),
            load("succeeded", "## b"),
            load("succeeded", "## a"),
        ]
    );
}

#[test]
fn a_failing_load_reports_its_start_and_failure_on_every_try() {
    let recorder = Recorder::default();
    let (vm, _) = require_vm(
        vec![("## a", None, library("a", "## a", &["error('fault')"]))],
        recorder.emitter(),
    );
    returned(&vm, "pcall(require, '## a')\npcall(require, '## a')");
    assert_eq!(
        library_loads(&recorder),
        [
            load("started", "## a"),
            load("failed", "## a"),
            load("started", "## a"),
            load("failed", "## a"),
        ]
    );
}
