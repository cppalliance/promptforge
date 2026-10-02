//! The HTTP failure classifier: the one place that reads a status code and
//! response text and turns them into a [`CompletionErrorKind`].
//!
//! [`classify_http_failure`] takes a non-success status and its bounded,
//! control-escaped body. [`classify_stream_error`] applies the same body-text
//! rules to an error envelope that arrives inside a 200 stream, where there
//! is no status. Both keep the body as the error's opt-in
//! [`detail`](CompletionError::detail) and never put it in the message.
//!
//! The rules run in this order, and the first match wins:
//!
//! 1. A 400 or 413 whose body names a context limit is `ContextOverflow`.
//! 2. A 429 whose body names quota or billing is `QuotaExhausted`; every
//!    other 429 is `RateLimited`.
//! 3. A 503 or 529, or any 5xx whose body names `overloaded`, is
//!    `Overloaded`; every other 5xx is `ServerError`.
//! 4. A 401 or 403 is `Unavailable`.
//! 5. A 400 whose body names a content policy term is `Refused`.
//! 6. Every other status is `Rejected`.

use crate::model::{CompletionError, CompletionErrorKind};

/// Body phrases the known backends emit when a request exceeds the model's
/// context window (OpenAI and compatible gateways, Anthropic, llama.cpp,
/// vLLM), matched case-insensitively.
const OVERFLOW_PHRASES: &[&str] = &[
    "context length",
    "context window",
    "context size",
    "context_length_exceeded",
    "maximum context length",
    "prompt is too long",
    "too many tokens",
    "exceeds the available context size",
    "exceed_context_size",
    "input is too long",
    "exceeds the maximum number of tokens",
    "too large for model",
];

/// Body words that mark a 429 as a spent quota rather than a rate limit.
const QUOTA_WORDS: &[&str] = &["quota", "billing", "insufficient_quota", "credit"];

/// Body words that mark a refusal on content policy grounds.
const REFUSAL_WORDS: &[&str] = &["content_filter", "content policy", "safety", "refus"];

/// The message for a 401 or 403, which says more than the generic
/// unavailable phrase.
const CREDENTIALS_PHRASE: &str = "the model backend did not accept the credentials";

/// Classifies a non-success HTTP response into a [`CompletionError`].
///
/// `body` must already be bounded and passed through
/// [`escape_controls`](crate::client::escape_controls); the classifier keeps
/// exactly that text as the error's [`detail`](CompletionError::detail) and
/// never puts it in the message. The message is the kind's fixed phrase
/// with ` (status N)` appended. A body that matches no rule is `Rejected`
/// (or `ServerError` for a 5xx), never a success.
///
/// # Examples
///
/// ```
/// use promptforge::model::CompletionErrorKind;
/// use promptforge::transport::classify_http_failure;
///
/// let error = classify_http_failure(503, "upstream is busy");
/// assert_eq!(error.kind(), CompletionErrorKind::Overloaded);
/// assert!(error.is_retryable());
/// assert_eq!(error.to_string(), "the model backend is overloaded (status 503)");
/// assert_eq!(error.detail(), Some("upstream is busy"));
///
/// let overflow = classify_http_failure(
///     400,
///     "This model's maximum context length is 4096 tokens. However, your \
///      messages resulted in 5120 tokens.",
/// );
/// assert_eq!(overflow.kind(), CompletionErrorKind::ContextOverflow);
/// assert_eq!(overflow.overflow(), (Some(5120), Some(4096)));
/// ```
#[must_use]
pub fn classify_http_failure(status: u16, body: &str) -> CompletionError {
    let lower = body.to_lowercase();
    let http = |kind: CompletionErrorKind| {
        http_error(kind, kind.phrase(), status).with_detail(body.to_owned())
    };
    if matches!(status, 400 | 413) && names_any(&lower, OVERFLOW_PHRASES) {
        let (prompt_tokens, window) = overflow_counts(&lower);
        return CompletionError::context_overflow(
            prompt_tokens,
            window,
            format!(
                "{} (status {status})",
                CompletionErrorKind::ContextOverflow.phrase()
            ),
        )
        .with_detail(body.to_owned());
    }
    match status {
        429 if names_any(&lower, QUOTA_WORDS) => http(CompletionErrorKind::QuotaExhausted),
        429 => http(CompletionErrorKind::RateLimited),
        503 | 529 => http(CompletionErrorKind::Overloaded),
        500..=599 if lower.contains("overloaded") => http(CompletionErrorKind::Overloaded),
        500..=599 => http(CompletionErrorKind::ServerError),
        401 | 403 => http_error(CompletionErrorKind::Unavailable, CREDENTIALS_PHRASE, status)
            .with_detail(body.to_owned()),
        400 if names_any(&lower, REFUSAL_WORDS) => http(CompletionErrorKind::Refused),
        _ => http(CompletionErrorKind::Rejected),
    }
}

/// Classifies an error envelope that arrived inside a 200 response stream.
///
/// The envelope has no status, so only the body-text rules apply: a context
/// limit is `ContextOverflow`, quota words are `QuotaExhausted`, `overloaded`
/// is `Overloaded`, and a content policy term is `Refused`. Text that
/// matches no rule is `Transport`, because the stream died in flight. The
/// message is the kind's fixed phrase with no status, and `body` is kept as
/// the error's [`detail`](CompletionError::detail). `body` must already be
/// bounded and control-escaped.
///
/// # Examples
///
/// ```
/// use promptforge::model::CompletionErrorKind;
/// use promptforge::transport::classify_stream_error;
///
/// let dropped = classify_stream_error("upstream closed the connection");
/// assert_eq!(dropped.kind(), CompletionErrorKind::Transport);
///
/// let busy = classify_stream_error("overloaded_error: try again later");
/// assert_eq!(busy.kind(), CompletionErrorKind::Overloaded);
/// assert_eq!(busy.to_string(), "the model backend is overloaded");
/// ```
#[must_use]
pub fn classify_stream_error(body: &str) -> CompletionError {
    let lower = body.to_lowercase();
    let error = if names_any(&lower, OVERFLOW_PHRASES) {
        let (prompt_tokens, window) = overflow_counts(&lower);
        CompletionError::context_overflow(
            prompt_tokens,
            window,
            CompletionErrorKind::ContextOverflow.phrase(),
        )
    } else {
        let kind = if names_any(&lower, QUOTA_WORDS) {
            CompletionErrorKind::QuotaExhausted
        } else if lower.contains("overloaded") {
            CompletionErrorKind::Overloaded
        } else if names_any(&lower, REFUSAL_WORDS) {
            CompletionErrorKind::Refused
        } else {
            CompletionErrorKind::Transport
        };
        CompletionError::new(kind, kind.phrase())
    };
    error.with_detail(body.to_owned())
}

/// Builds an HTTP failure: `phrase` with the status appended.
fn http_error(kind: CompletionErrorKind, phrase: &str, status: u16) -> CompletionError {
    CompletionError::new(kind, format!("{phrase} (status {status})"))
}

fn names_any(lower: &str, words: &[&str]) -> bool {
    words.iter().any(|word| lower.contains(word))
}

/// The prompt and window token counts a context-limit message states, each
/// `None` when the text does not give it. `lower` is the lowercased body.
///
/// Two forms are read: "maximum context length is N tokens ... M tokens"
/// (window N, prompt M), and "N tokens > M maximum" (prompt N, window M).
fn overflow_counts(lower: &str) -> (Option<u32>, Option<u32>) {
    if let Some(counts) = counts_from_maximum(lower) {
        return counts;
    }
    counts_from_greater_than(lower).unwrap_or((None, None))
}

fn counts_from_maximum(lower: &str) -> Option<(Option<u32>, Option<u32>)> {
    const LEAD: &str = "maximum context length is ";
    let start = lower.find(LEAD)? + LEAD.len();
    let (window, rest) = leading_number(&lower[start..])?;
    Some((first_count_before_tokens(rest), window))
}

fn counts_from_greater_than(lower: &str) -> Option<(Option<u32>, Option<u32>)> {
    const MID: &str = " tokens > ";
    let at = lower.find(MID)?;
    let before = &lower[..at];
    let prompt = &before[before.trim_end_matches(|c: char| c.is_ascii_digit()).len()..];
    if prompt.is_empty() {
        return None;
    }
    let (window, rest) = leading_number(&lower[at + MID.len()..])?;
    rest.starts_with(" maximum")
        .then(|| (prompt.parse().ok(), window))
}

/// Splits a leading run of ASCII digits off `text`. The number is `None`
/// when the run does not fit a `u32`; the whole function is `None` when
/// `text` starts with no digit.
fn leading_number(text: &str) -> Option<(Option<u32>, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some((text[..end].parse().ok(), &text[end..]))
}

/// The first number in `text` that is directly followed by " tokens".
fn first_count_before_tokens(text: &str) -> Option<u32> {
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let (number, after) = leading_number(&rest[start..])?;
        if after.starts_with(" tokens") {
            return number;
        }
        rest = after;
    }
    None
}

#[cfg(test)]
#[path = "classify-tests.rs"]
mod tests;
