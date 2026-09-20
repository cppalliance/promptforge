//! The shared error-source wrappers: one crate-owned newtype per
//! third-party error a public error surface would otherwise name.
//!
//! Every product family had grown its own copy of the same newtype, so the
//! same wrapper existed under the same name in crates that could not see
//! each other. This crate depends on no workspace crate, which is what lets
//! the harness, workshop, and gateway families all use it without a
//! cross-family edge.
//!
//! `#[error(transparent)]` delegates both `Display` and `source()` to the
//! wrapped error, so the wrapper is invisible in a rendered chain. That
//! same delegation puts the wrapped value out of reach by type: the chain
//! walks straight past it to the third-party error's own source. The
//! accessors are what restore branching on the third-party error.

/// The JSON error behind a product error variant, so the public error
/// surface names no `serde_json` type. Renders and sources exactly as the
/// JSON error does.
#[cfg(feature = "json")]
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct JsonSource(serde_json::Error);

#[cfg(feature = "json")]
impl JsonSource {
    /// The wrapped JSON error.
    #[must_use]
    pub fn as_inner(&self) -> &serde_json::Error {
        &self.0
    }

    /// Takes the wrapped JSON error out of the wrapper.
    #[must_use]
    pub fn into_inner(self) -> serde_json::Error {
        self.0
    }
}

#[cfg(feature = "json")]
impl From<serde_json::Error> for JsonSource {
    fn from(source: serde_json::Error) -> Self {
        JsonSource(source)
    }
}

/// The transport error behind a product error variant, so the public error
/// surface names no HTTP client type. Renders and sources exactly as the
/// transport error does.
#[cfg(feature = "http")]
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct HttpSource(reqwest::Error);

#[cfg(feature = "http")]
impl HttpSource {
    /// The wrapped transport error.
    #[must_use]
    pub fn as_inner(&self) -> &reqwest::Error {
        &self.0
    }

    /// Takes the wrapped transport error out of the wrapper.
    #[must_use]
    pub fn into_inner(self) -> reqwest::Error {
        self.0
    }
}

#[cfg(feature = "http")]
impl From<reqwest::Error> for HttpSource {
    fn from(source: reqwest::Error) -> Self {
        HttpSource(source)
    }
}

/// The database engine's error behind a product error variant, so the
/// public error surface names no engine type. Renders and sources exactly
/// as the engine's error does.
#[cfg(feature = "database")]
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DatabaseSource(turso::Error);

#[cfg(feature = "database")]
impl DatabaseSource {
    /// The wrapped engine error.
    #[must_use]
    pub fn as_inner(&self) -> &turso::Error {
        &self.0
    }

    /// Takes the wrapped engine error out of the wrapper.
    #[must_use]
    pub fn into_inner(self) -> turso::Error {
        self.0
    }
}

#[cfg(feature = "database")]
impl From<turso::Error> for DatabaseSource {
    fn from(source: turso::Error) -> Self {
        DatabaseSource(source)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    #[cfg(feature = "json")]
    #[test]
    fn a_json_source_yields_the_serde_error_and_renders_as_it_in_a_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("the operation did not complete")]
        struct Outer(#[source] crate::JsonSource);

        let Err(error) = serde_json::from_str::<u32>("nope") else {
            panic!("`nope` must not parse as a u32");
        };
        let rendered = error.to_string();
        let source = crate::JsonSource::from(error);
        // Serde's own classification, which `#[error(transparent)]` gives
        // no way back to.
        assert!(source.as_inner().is_syntax());

        let outer = Outer(source);
        assert_eq!(outer.to_string(), "the operation did not complete");
        assert_eq!(
            outer.source().map(ToString::to_string),
            Some(rendered.clone())
        );
        assert_eq!(outer.0.into_inner().to_string(), rendered);
    }

    #[cfg(feature = "http")]
    #[test]
    fn an_http_source_yields_the_reqwest_error_and_renders_as_it_in_a_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("the request did not complete")]
        struct Outer(#[source] crate::HttpSource);

        let Err(error) = reqwest::Proxy::all("http://") else {
            panic!("a proxy URL with an empty host must not build");
        };
        let rendered = error.to_string();
        let source = crate::HttpSource::from(error);
        // Reqwest's own kind predicate, which the chain cannot answer.
        assert!(source.as_inner().is_builder());

        let outer = Outer(source);
        assert_eq!(outer.to_string(), "the request did not complete");
        assert_eq!(
            outer.source().map(ToString::to_string),
            Some(rendered.clone())
        );
        assert_eq!(outer.0.into_inner().to_string(), rendered);
    }

    #[cfg(feature = "database")]
    #[test]
    fn a_database_source_yields_the_turso_error_and_renders_as_it_in_a_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("the run log operation did not complete")]
        struct Outer(#[source] crate::DatabaseSource);

        let error = turso::Error::Corrupt("page 1 is not a b-tree page".to_owned());
        let rendered = error.to_string();
        let source = crate::DatabaseSource::from(error);
        // The engine's own variant, which the chain flattens to text.
        assert!(matches!(source.as_inner(), turso::Error::Corrupt(_)));

        let outer = Outer(source);
        assert_eq!(outer.to_string(), "the run log operation did not complete");
        assert_eq!(
            outer.source().map(ToString::to_string),
            Some(rendered.clone())
        );
        assert_eq!(outer.0.into_inner().to_string(), rendered);
    }
}
