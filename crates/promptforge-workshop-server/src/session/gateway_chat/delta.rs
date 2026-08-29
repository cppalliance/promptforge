//! Provider delta decoding for the direct-gateway chat adapter: the
//! content and reasoning fields of one gateway SSE payload, pure and
//! socket-free.

/// The text fields of one streaming delta: answer content and the
/// reasoning side channel, either of which may be absent.
pub(super) struct DeltaFields {
    pub(super) content: Option<String>,
    pub(super) reasoning: Option<String>,
}

/// Extracts the content and reasoning deltas from one gateway SSE payload.
///
/// Role-priming and usage events have no `choices[0].delta.content` and
/// contribute nothing to the assembled response. An empty-string content
/// delta (the common `{"role":"assistant","content":""}` priming chunk)
/// is filtered like an absent one: forwarding it would emit an empty
/// `delta` frame that closes the UI's Thinking block and flips the
/// activity LED before any answer text exists. Reasoning models stream
/// their scratch work under `reasoning_content` (or the `reasoning` /
/// `thinking` synonyms, matching promptforge-core's normalization); the
/// first non-empty synonym wins, so a present-but-empty key falls
/// through to a populated one instead of masking it.
pub(super) fn delta_fields(payload: &str) -> DeltaFields {
    let empty = DeltaFields {
        content: None,
        reasoning: None,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return empty;
    };
    let Some(delta) = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
    else {
        return empty;
    };
    let content = delta
        .get("content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let reasoning = ["reasoning_content", "reasoning", "thinking"]
        .iter()
        .filter_map(|key| delta.get(*key).and_then(serde_json::Value::as_str))
        .find(|text| !text.is_empty())
        .map(str::to_string);
    DeltaFields { content, reasoning }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_reasoning_synonym_falls_through_to_a_populated_one() {
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"","reasoning":"actual scratch work"}}]}"#,
        );
        assert_eq!(fields.reasoning.as_deref(), Some("actual scratch work"));
        assert_eq!(fields.content, None);
    }

    #[test]
    fn all_empty_reasoning_synonyms_yield_no_reasoning() {
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"","reasoning":"","thinking":"","content":"answer"}}]}"#,
        );
        assert!(fields.reasoning.is_none());
        assert_eq!(fields.content.as_deref(), Some("answer"));
    }

    #[test]
    fn an_empty_content_delta_is_filtered_like_an_absent_one() {
        // The common role-priming chunk carries `content: ""`; forwarding
        // it would emit an empty delta frame that closes the UI's Thinking
        // block and flips the activity LED before any answer text exists.
        let fields =
            delta_fields(r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#);
        assert_eq!(fields.content, None);
        assert!(fields.reasoning.is_none());
        // An empty content beside live reasoning must not mask the
        // reasoning nor emit an answer frame mid-think.
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"content":"","reasoning_content":"scratch"}}]}"#,
        );
        assert_eq!(fields.content, None);
        assert_eq!(fields.reasoning.as_deref(), Some("scratch"));
    }
}
