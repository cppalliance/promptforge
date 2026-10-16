//! Tests for the library-load variants' serialized form: each carries the
//! library's heading beside the three coordinates, on one line.

use super::*;

/// The coordinates and library of loading `## shared_stuff` into `Main`'s
/// environment, stamped `seq`.
fn coordinates(seq: u32) -> (String, String, Provenance, String) {
    (
        "run-1".to_owned(),
        "Main".to_owned(),
        provenance("0", seq),
        "## shared_stuff".to_owned(),
    )
}

/// One sample of each library-load variant, for `payload_samples`.
pub(super) fn samples() -> Vec<Event> {
    let (execution, section, provenance, library) = coordinates(17);
    let started = Event::LuaLibraryLoadStarted {
        execution,
        section,
        provenance,
        library,
    };
    let (execution, section, provenance, library) = coordinates(18);
    let succeeded = Event::LuaLibraryLoadSucceeded {
        execution,
        section,
        provenance,
        library,
    };
    let (execution, section, provenance, library) = coordinates(19);
    let failed = Event::LuaLibraryLoadFailed {
        execution,
        section,
        provenance,
        library,
    };
    vec![started, succeeded, failed]
}

#[test]
fn a_library_load_serializes_its_library_beside_the_coordinates() {
    let kinds = [
        "lua_library_load_started",
        "lua_library_load_succeeded",
        "lua_library_load_failed",
    ];
    for (event, kind) in samples().iter().zip(kinds) {
        let seq = event.provenance().seq;
        assert_eq!(
            serde_json::to_string(event).expect("an event serializes"),
            format!(
                r###"{{"kind":"{kind}","execution":"run-1","section":"Main","provenance":{{"task":"0","seq":{seq}}},"library":"## shared_stuff"}}"###
            )
        );
        round_trips(event);
    }
}
