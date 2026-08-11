//! Error types for the MCP server.
//!
//! One opaque error per failure family, each split into its own submodule and
//! re-exported here so every `crate::error::…` path and the crate's public
//! surface are unchanged. Each error keeps its representation private, so a
//! caller classifies with `kind()` and reads causes through
//! [`std::error::Error::source`] rather than matching a variant.

mod config;
mod fault;
mod prepared;
mod run;
mod serve;
mod watch;

pub use self::config::{ConfigError, ConfigErrorKind};
pub(crate) use self::fault::Fault;
pub use self::fault::{CatalogError, CatalogErrorKind, FaultKind, FaultRef, Faults};
pub use self::prepared::{PreparedToolsError, PreparedToolsErrorKind};
pub use self::run::{RunError, RunErrorKind};
// The transport-start error is internal: the serve functions are crate-private
// and the public boot entry surfaces it only through the opaque `RunError`, so
// its type and classifier stay off the public API. The classifier is read only
// by the transport tests, so it is compiled only under test.
pub(crate) use self::serve::ServeError;
#[cfg(test)]
pub(crate) use self::serve::ServeErrorKind;
pub use self::watch::{WatchError, WatchErrorKind};

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn config_error_classifies_and_renders_each_shape() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let read = ConfigError::read(PathBuf::from("prompts.toml"), io);
        assert_eq!(read.kind(), ConfigErrorKind::Read);
        assert_eq!(read.path(), Some(Path::new("prompts.toml")));
        assert!(read.to_string().contains("prompts.toml"));
        let source = std::error::Error::source(&read).expect("a read carries its io source");
        let io = source
            .downcast_ref::<std::io::Error>()
            .expect("the source is an io::Error");
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);

        let parse = ConfigError::parse("bad value");
        assert_eq!(parse.kind(), ConfigErrorKind::Parse);
        assert!(std::error::Error::source(&parse).is_none());
        assert!(parse.to_string().contains("bad value"));

        assert_eq!(
            ConfigError::empty_token().kind(),
            ConfigErrorKind::EmptyToken
        );

        let unresolved = ConfigError::unresolved_var("TOKEN");
        assert_eq!(unresolved.kind(), ConfigErrorKind::UnresolvedVar);
        assert!(unresolved.to_string().contains("TOKEN"));
        assert!(unresolved.path().is_none());

        assert_eq!(
            ConfigError::interpolation("unclosed").kind(),
            ConfigErrorKind::Interpolation
        );
    }

    #[test]
    fn parse_from_toml_preserves_the_source() {
        let toml_err = toml::from_str::<toml::Table>("= not valid").unwrap_err();
        let err = ConfigError::parse_toml(toml_err);
        assert_eq!(err.kind(), ConfigErrorKind::Parse);
        assert!(
            std::error::Error::source(&err).is_some(),
            "a toml parse failure keeps its source"
        );
    }

    #[test]
    fn read_error_preserves_a_non_unicode_render() {
        // The path is stored as a `PathBuf`, so `path()` returns it verbatim
        // rather than through a lossy display round trip.
        let path = PathBuf::from("weird/../name.toml");
        let err = ConfigError::read(
            path.clone(),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert_eq!(err.path(), Some(path.as_path()));
    }

    #[test]
    fn watch_error_classifies_names_its_path_and_keeps_its_source() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let watch = WatchError::watch(PathBuf::from("prompts"), io);
        assert_eq!(watch.kind(), WatchErrorKind::Watch);
        assert_eq!(watch.path(), Some(Path::new("prompts")));
        assert!(watch.to_string().contains("prompts"));
        let source = std::error::Error::source(&watch).expect("a watch failure keeps its source");
        assert!(
            source.downcast_ref::<std::io::Error>().is_some(),
            "the erased source is the io::Error it was built from"
        );

        let create = WatchError::create(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(create.kind(), WatchErrorKind::Create);
        assert!(create.path().is_none());
        assert!(std::error::Error::source(&create).is_some());

        let runtime = WatchError::runtime();
        assert_eq!(runtime.kind(), WatchErrorKind::Runtime);
        assert!(runtime.path().is_none());
        assert!(std::error::Error::source(&runtime).is_none());
    }

    #[test]
    fn catalog_error_display_is_singular_then_plural() {
        let one = CatalogError::new(vec![Fault::new(
            FaultKind::Unparsable,
            Some("p".into()),
            None,
            "boom",
        )]);
        let text = one.to_string();
        assert!(text.contains("1 fault"), "{text}");
        assert!(!text.contains("faults"), "{text}");
        assert_eq!(one.kind(), CatalogErrorKind::Broken);

        let two = CatalogError::new(vec![
            Fault::new(FaultKind::Empty, None, None, "nothing resolved"),
            Fault::new(FaultKind::Pattern, None, None, "bad include"),
        ]);
        let text = two.to_string();
        assert!(text.contains("2 faults"), "{text}");
        assert_eq!(two.kind(), CatalogErrorKind::Configuration);
    }

    #[test]
    fn fault_ref_reports_locus_kind_and_display() {
        let err = CatalogError::new(vec![
            Fault::new(
                FaultKind::Unparsable,
                Some("p".into()),
                Some(PathBuf::from("a.md")),
                "bad",
            ),
            Fault::new(
                FaultKind::Unreadable,
                None,
                Some(PathBuf::from("b.md")),
                "unreadable",
            ),
            Fault::new(FaultKind::Empty, None, None, "empty catalog"),
        ]);
        let faults: Vec<FaultRef<'_>> = err.faults().collect();
        assert_eq!(faults.len(), 3);

        assert_eq!(faults[0].kind(), FaultKind::Unparsable);
        assert_eq!(faults[0].prompt(), Some("p"));
        assert_eq!(faults[0].path(), Some(Path::new("a.md")));
        assert!(faults[0].to_string().contains("bad"));

        assert_eq!(faults[1].kind(), FaultKind::Unreadable);
        assert_eq!(faults[1].prompt(), None);
        assert_eq!(faults[1].path(), Some(Path::new("b.md")));

        assert_eq!(faults[2].kind(), FaultKind::Empty);
        assert_eq!(faults[2].to_string(), "empty catalog");
    }
}
