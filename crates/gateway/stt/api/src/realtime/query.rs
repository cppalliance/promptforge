#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum QueryError {
    MissingIntent,
    DuplicateParameter,
    UnknownParameter,
    UnsupportedIntent,
    MalformedParameter,
}

pub(super) fn validate(query: Option<&str>) -> Result<(), QueryError> {
    let query = query
        .filter(|query| !query.is_empty())
        .ok_or(QueryError::MissingIntent)?;
    let mut intent = None;
    for parameter in query.split('&') {
        let mut parts = parameter.split('=');
        let name = parts.next().unwrap_or_default();
        let value = parts.next().ok_or(QueryError::MalformedParameter)?;
        if name.is_empty() || value.is_empty() || parts.next().is_some() {
            return Err(QueryError::MalformedParameter);
        }
        if name != "intent" {
            return Err(QueryError::UnknownParameter);
        }
        if intent.replace(value).is_some() {
            return Err(QueryError::DuplicateParameter);
        }
    }
    match intent {
        None => Err(QueryError::MissingIntent),
        Some("transcription") => Ok(()),
        Some(_) => Err(QueryError::UnsupportedIntent),
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryError, validate};

    #[test]
    fn exact_transcription_intent_is_the_only_accepted_query() {
        assert_eq!(validate(Some("intent=transcription")), Ok(()));
        assert_eq!(validate(None), Err(QueryError::MissingIntent));
        assert_eq!(validate(Some("")), Err(QueryError::MissingIntent));
        assert_eq!(
            validate(Some("intent=transcription&intent=transcription")),
            Err(QueryError::DuplicateParameter)
        );
        assert_eq!(
            validate(Some("intent=transcription&intent=realtime")),
            Err(QueryError::DuplicateParameter)
        );
        assert_eq!(
            validate(Some("intent=transcription&extra=1")),
            Err(QueryError::UnknownParameter)
        );
        assert_eq!(
            validate(Some("intent=realtime")),
            Err(QueryError::UnsupportedIntent)
        );
        assert_eq!(
            validate(Some("intent=transcription=extra")),
            Err(QueryError::MalformedParameter)
        );
        assert_eq!(
            validate(Some("intent=%74ranscription")),
            Err(QueryError::UnsupportedIntent)
        );
    }
}
