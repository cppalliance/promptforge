//! The builtin dialect registry and evidence-based resolution.

use super::dispatch::{DetectScore, ToolDialect};
use super::{DialectError, DialectEvidence, Gemma3ToolCodeDialect, OpenAiDialect, ToolDialectId};
use crate::Error;

/// Registry of builtin tool dialects with evidence-based resolution.
///
/// `#[non_exhaustive]` so future fields (or a change away from the builtin-only
/// constructor) are not a breaking change; it is only constructible through
/// [`ToolDialectRegistry::builtin`].
#[non_exhaustive]
pub struct ToolDialectRegistry {
    dialects: Vec<Box<dyn ToolDialect>>,
}

impl std::fmt::Debug for ToolDialectRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<ToolDialectId> = self.dialects.iter().map(|d| d.id()).collect();
        f.debug_struct("ToolDialectRegistry")
            .field("dialects", &ids)
            .finish()
    }
}

impl ToolDialectRegistry {
    /// Construct the registry populated with all builtin dialects.
    ///
    /// ```
    /// use promptforge_core::dialects::{DialectEvidence, ToolDialectId, ToolDialectRegistry};
    ///
    /// let registry = ToolDialectRegistry::builtin();
    /// let evidence = DialectEvidence::new(Some(true), None, None, None);
    /// assert_eq!(registry.resolve(&evidence)?, ToolDialectId::OpenAi);
    /// # Ok::<(), promptforge_core::dialects::DialectError>(())
    /// ```
    #[must_use]
    pub fn builtin() -> ToolDialectRegistry {
        ToolDialectRegistry {
            dialects: vec![Box::new(OpenAiDialect), Box::new(Gemma3ToolCodeDialect)],
        }
    }

    /// Look up a dialect by its [`ToolDialectId`].
    #[must_use]
    pub(crate) fn get(&self, id: ToolDialectId) -> Option<&dyn ToolDialect> {
        self.dialects
            .iter()
            .find(|d| d.id() == id)
            .map(std::convert::AsRef::as_ref)
    }

    /// Resolve evidence into a single dialect, failing on ties or no match.
    ///
    /// Scans the registry once, keeping the highest detection score and every
    /// id tied at it. A unique top score resolves; a shared top score is a
    /// [`DialectErrorKind::Tie`](crate::dialects::DialectErrorKind::Tie); no
    /// score at all is a
    /// [`DialectErrorKind::NoMatch`](crate::dialects::DialectErrorKind::NoMatch).
    ///
    /// ```
    /// use promptforge_core::dialects::{
    ///     DialectEvidence, DialectErrorKind, ToolDialectId, ToolDialectRegistry,
    /// };
    ///
    /// let registry = ToolDialectRegistry::builtin();
    ///
    /// // Authoritative native support -> OpenAI.
    /// let native = DialectEvidence::new(Some(true), None, None, None);
    /// assert_eq!(registry.resolve(&native)?, ToolDialectId::OpenAi);
    ///
    /// // A Gemma template without native tools -> Gemma tool_code.
    /// let gemma = DialectEvidence::new(
    ///     Some(false),
    ///     Some("<start_of_turn>user\n".into()),
    ///     Some("gemma-3-27b-it".into()),
    ///     None,
    /// );
    /// assert_eq!(registry.resolve(&gemma)?, ToolDialectId::Gemma3ToolCode);
    ///
    /// // No evidence -> NoMatch.
    /// let miss = registry.resolve(&DialectEvidence::default()).unwrap_err();
    /// assert_eq!(miss.kind(), DialectErrorKind::NoMatch);
    /// # Ok::<(), promptforge_core::dialects::DialectError>(())
    /// ```
    ///
    /// # Errors
    /// Returns a [`DialectError`] classified `NoMatch` when no dialect scores on
    /// the evidence, and `Tie` when two or more dialects share the top score.
    pub fn resolve(
        &self,
        evidence: &DialectEvidence,
    ) -> std::result::Result<ToolDialectId, DialectError> {
        // Single scan tracking the best score and every id tied at it, in
        // registry order - no intermediate collection or sort.
        let mut best: Option<DetectScore> = None;
        let mut leader: Option<ToolDialectId> = None;
        let mut tied: Vec<ToolDialectId> = Vec::new();
        for dialect in &self.dialects {
            let Some(score) = dialect.detect(evidence) else {
                continue;
            };
            let id = dialect.id();
            match best {
                Some(current) if score < current => {}
                Some(current) if score == current => tied.push(id),
                _ => {
                    best = Some(score);
                    leader = Some(id);
                    tied.clear();
                    tied.push(id);
                }
            }
        }

        let Some(leader) = leader else {
            return Err(DialectError::from(Error::DialectNone));
        };
        if tied.len() > 1 {
            return Err(DialectError::from(Error::DialectTie { candidates: tied }));
        }
        Ok(leader)
    }
}
