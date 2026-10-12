//! Library-load reports: each phase folds to its own kind with the library
//! section it names.

use super::*;
use promptforge_types::emitter::LibraryLoad;

#[test]
fn a_library_load_folds_to_its_kind_with_the_library() {
    let recorder = Recorder::default();
    recorder
        .emitter()
        .library_load("Main", "## shared_stuff", LibraryLoad::Started);
    recorder
        .emitter()
        .library_load("Main", "## shared_stuff", LibraryLoad::Succeeded);
    recorder
        .emitter()
        .library_load("Main", "## helpers", LibraryLoad::Failed);
    assert_eq!(
        recorder.observations(),
        [
            (
                "Main".to_owned(),
                Observation::Library {
                    kind: "lua_library_load_started".to_owned(),
                    library: "## shared_stuff".to_owned(),
                },
            ),
            (
                "Main".to_owned(),
                Observation::Library {
                    kind: "lua_library_load_succeeded".to_owned(),
                    library: "## shared_stuff".to_owned(),
                },
            ),
            (
                "Main".to_owned(),
                Observation::Library {
                    kind: "lua_library_load_failed".to_owned(),
                    library: "## helpers".to_owned(),
                },
            ),
        ]
    );
}
