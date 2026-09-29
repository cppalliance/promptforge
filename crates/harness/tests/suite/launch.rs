//! The launch surface a client builds through the public API: the
//! engine's filesystem handle, named through `harness::vfs`, and the
//! output error a session reports.

use harness::vfs::{VfsError, VfsRef};
use harness::{LaunchOptions, OutputError};

#[test]
fn default_launch_options_hand_over_no_filesystem() {
    assert!(LaunchOptions::default().vfs.is_none());
}

#[test]
fn a_client_hands_a_session_the_engines_filesystem_handle() {
    let options = LaunchOptions {
        vfs: Some(VfsRef::default()),
    };
    assert!(options.vfs.is_some());
}

#[test]
fn an_output_store_failure_renders_its_own_message_and_sources_the_engines_error() {
    let error = OutputError::Store {
        path: "report.md".to_owned(),
        source: VfsError::NotFound {
            path: "report.md".to_owned(),
        },
    };
    assert_eq!(
        error.to_string(),
        "the output file `report.md` could not be read"
    );
    assert!(std::error::Error::source(&error).is_some());
}
