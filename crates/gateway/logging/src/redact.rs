//! The privacy pass every queued record goes through.
//!
//! No log record may contain credentials, cookies, authorization headers,
//! environment values, request bodies, audio, transcript text, prompts, or
//! full local model paths. Classified tracing fields are suppressed before
//! their values are formatted. The bounded text pass remains at the queue
//! chokepoint for authorization values, assignments, and dependency errors
//! embedded in unstructured messages.
//!
//! The patterns are ASCII and matched case-insensitively where a header
//! name is involved; redaction never reorders or truncates the rest of
//! the line.

/// The mask replacing a sensitive value.
pub(crate) const REDACTED: &str = "[redacted]";

const SENSITIVE_FIELDS: &[&str] = &[
    "access_token",
    "api_key",
    "audio",
    "auth",
    "authorization",
    "base_url",
    "body",
    "config_path",
    "cookie",
    "cookies",
    "credential",
    "credentials",
    "endpoint_url",
    "file_path",
    "headers",
    "model_path",
    "password",
    "path",
    "payload",
    "prompt",
    "prompts",
    "proxy_authorization",
    "refresh_token",
    "request_body",
    "request_url",
    "response_body",
    "response_url",
    "secret",
    "set_cookie",
    "system_prompt",
    "token",
    "transcript",
    "uri",
    "url",
    "user_prompt",
];

const SENSITIVE_ALIAS_SUFFIXES: &[&str] = &[
    "data", "field", "header", "headers", "raw", "text", "value", "values",
];

/// Whether a tracing field is classified and must never format its value.
pub(crate) fn is_sensitive_field(name: &str) -> bool {
    let leaf = name
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(name)
        .trim_start_matches("r#");
    SENSITIVE_FIELDS
        .iter()
        .any(|candidate| has_sensitive_component(leaf, candidate))
}

fn has_sensitive_component(name: &str, component: &str) -> bool {
    let mut from = 0;
    while let Some(start) = find_ascii(name, component, from) {
        let end = start + component.len();
        let left_boundary = start == 0
            || name
                .as_bytes()
                .get(start - 1)
                .is_some_and(|byte| matches!(byte, b'_' | b'-'));
        let right_boundary = end == name.len()
            || name
                .as_bytes()
                .get(end)
                .is_some_and(|byte| matches!(byte, b'_' | b'-'));
        if left_boundary && right_boundary {
            let suffix = name[end..].trim_start_matches(['_', '-']);
            if suffix.is_empty()
                || suffix.split(['_', '-']).all(|part| {
                    SENSITIVE_ALIAS_SUFFIXES
                        .iter()
                        .any(|suffix| part.eq_ignore_ascii_case(suffix))
                })
            {
                return true;
            }
        }
        from = end;
    }
    false
}

/// Fixed-capacity valid UTF-8 used while redaction may expand masks.
#[derive(Debug)]
pub(crate) struct RedactedLine {
    storage: Box<[u8]>,
    len: usize,
    truncated: bool,
}

impl RedactedLine {
    fn new(capacity: usize) -> Self {
        #[cfg(test)]
        crate::allocation_tracking::record(capacity);
        Self {
            storage: vec![0; capacity].into_boxed_slice(),
            len: 0,
            truncated: false,
        }
    }

    fn capacity(&self) -> usize {
        self.storage.len()
    }

    fn push_str(&mut self, text: &str) {
        let remaining = self.capacity().saturating_sub(self.len);
        let mut retained = remaining.min(text.len());
        while !text.is_char_boundary(retained) {
            retained -= 1;
        }
        self.storage[self.len..self.len + retained].copy_from_slice(&text.as_bytes()[..retained]);
        self.len += retained;
        self.truncated |= retained < text.len();
    }

    fn truncate(&mut self, mut len: usize) {
        len = len.min(self.len);
        while len < self.len && self.storage[len] & 0b1100_0000 == 0b1000_0000 {
            len -= 1;
        }
        self.len = len;
    }

    /// Adds the truncation marker when either formatting or redaction
    /// omitted bytes and returns exact-size queue storage.
    pub(crate) fn finish(
        mut self,
        marker: &str,
        formatter_truncated: bool,
    ) -> Option<(Box<str>, bool)> {
        let truncated = formatter_truncated || self.truncated;
        if truncated {
            let payload_limit = self.capacity().saturating_sub(marker.len());
            self.truncate(payload_limit);
            self.truncated = false;
            self.push_str(marker);
            debug_assert!(
                !self.truncated,
                "the configured record bound fits the marker"
            );
        }
        let mut bytes = self.storage.into_vec();
        bytes.truncate(self.len);
        #[cfg(test)]
        crate::allocation_tracking::record(self.len);
        let text = String::from_utf8(bytes).ok()?;
        Some((text.into_boxed_str(), truncated))
    }
}

/// Masks the sensitive shapes `text` could contain without permitting any
/// intermediate output buffer to exceed `capacity`.
pub(crate) fn redact_line_bounded(text: &str, capacity: usize) -> RedactedLine {
    let mut out = RedactedLine::new(capacity);
    let mut cursor = 0;
    while let Some(span) = next_sensitive_span(text, cursor) {
        out.push_str(&text[cursor..span.start]);
        out.push_str(REDACTED);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Masks the sensitive shapes `text` could contain and returns the result.
/// Tests use this convenience path; production supplies its strict record
/// capacity through [`redact_line_bounded`].
#[cfg(test)]
pub(crate) fn redact_line(text: &str) -> String {
    let capacity = text.len().saturating_mul(REDACTED.len());
    redact_line_bounded(text, capacity)
        .finish("", false)
        .filter(|(_, truncated)| !truncated)
        .map_or_else(String::new, |(text, _)| text.into())
}

/// The first position where `needle` matches `haystack` at or after
/// `from`, comparing ASCII case-insensitively. Byte offsets stay valid
/// because only ASCII needles are ever searched.
fn find_ascii(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from > haystack.len() || needle.len() > haystack.len().saturating_sub(from) {
        return None;
    }
    haystack.as_bytes()[from..]
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
        .map(|offset| from + offset)
}

#[derive(Debug, Clone, Copy)]
struct SensitiveSpan {
    start: usize,
    end: usize,
}

fn next_sensitive_span(text: &str, from: usize) -> Option<SensitiveSpan> {
    let mut cursor = from;
    while cursor < text.len() {
        for header in ["authorization:", "cookie:", "set-cookie:"] {
            if starts_ascii(text, header, cursor) {
                let mut value_start = cursor + header.len();
                while text
                    .as_bytes()
                    .get(value_start)
                    .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
                {
                    value_start += 1;
                }
                let value_end = text[value_start..]
                    .find('\n')
                    .map_or(text.len(), |newline| value_start + newline);
                if value_end > value_start {
                    return Some(SensitiveSpan {
                        start: value_start,
                        end: value_end,
                    });
                }
            }
        }
        for scheme in ["bearer ", "basic "] {
            if starts_ascii(text, scheme, cursor) {
                let token_start = cursor + scheme.len();
                let token_end = text[token_start..]
                    .find(|character: char| {
                        character.is_whitespace()
                            || matches!(character, ',' | ';' | ')' | ']' | '}')
                    })
                    .map_or(text.len(), |end| token_start + end);
                if token_end > token_start {
                    return Some(SensitiveSpan {
                        start: token_start,
                        end: token_end,
                    });
                }
            }
        }
        for field in SENSITIVE_FIELDS {
            if starts_ascii(text, field, cursor)
                && let Some(span) = assignment_span_at(text, field, cursor)
            {
                return Some(span);
            }
        }
        if let Some(span) = url_span_at(text, cursor) {
            return Some(span);
        }
        if let Some(span) = local_path_span_at(text, cursor) {
            return Some(span);
        }
        cursor += text[cursor..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn starts_ascii(text: &str, needle: &str, at: usize) -> bool {
    text.as_bytes()
        .get(at..at.saturating_add(needle.len()))
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
}

fn assignment_span_at(text: &str, field: &str, start: usize) -> Option<SensitiveSpan> {
    let after_key = start + field.len();
    let bytes = text.as_bytes();
    if start != 0 && bytes[start - 1].is_ascii_alphanumeric()
        || bytes.get(after_key).is_some_and(u8::is_ascii_alphanumeric)
    {
        return None;
    }
    let mut cursor = after_key;
    if cursor < bytes.len() && bytes[cursor] == b'"' {
        cursor += 1;
    }
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'\n' {
        cursor += 1;
    }
    if cursor >= bytes.len() || (bytes[cursor] != b'=' && bytes[cursor] != b':') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'\n' {
        cursor += 1;
    }
    let quote = bytes
        .get(cursor)
        .copied()
        .filter(|byte| matches!(byte, b'"' | b'\''));
    if quote.is_some() {
        cursor += 1;
    }
    let value_start = cursor;
    let value_end = if let Some(quote) = quote {
        quoted_value_end(text.as_bytes(), value_start, quote)
    } else {
        text[value_start..]
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '"' | '\'' | ',' | '&' | ';' | ')' | ']' | '}')
            })
            .map_or(text.len(), |end| value_start + end)
    };
    (value_end > value_start).then_some(SensitiveSpan {
        start: value_start,
        end: value_end,
    })
}

fn quoted_value_end(text: &[u8], start: usize, quote: u8) -> usize {
    let mut escaped = false;
    for (offset, byte) in text[start..].iter().copied().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            return start + offset;
        }
    }
    text.len()
}

fn url_span_at(text: &str, start: usize) -> Option<SensitiveSpan> {
    let bytes = text.as_bytes();
    if !bytes.get(start).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut marker = start + 1;
    while bytes
        .get(marker)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        marker += 1;
    }
    if bytes.get(marker..marker + 3) != Some(b"://") {
        return None;
    }
    Some(SensitiveSpan {
        start,
        end: sensitive_token_end(text, marker + 3),
    })
}

fn local_path_span_at(text: &str, start: usize) -> Option<SensitiveSpan> {
    const MODEL_EXTENSIONS: &[&str] = &[
        ".bin",
        ".ggml",
        ".gguf",
        ".onnx",
        ".pt",
        ".pth",
        ".safetensors",
    ];
    let bytes = text.as_bytes();
    let boundary = start == 0
        || bytes[start - 1].is_ascii_whitespace()
        || matches!(
            bytes[start - 1],
            b'"' | b'\'' | b'(' | b'[' | b'{' | b'=' | b':'
        );
    let windows = start + 2 < bytes.len()
        && bytes[start].is_ascii_alphabetic()
        && bytes[start + 1] == b':'
        && matches!(bytes[start + 2], b'/' | b'\\');
    let unc = start + 1 < bytes.len()
        && matches!(bytes[start], b'/' | b'\\')
        && bytes[start + 1] == bytes[start];
    let unix = bytes[start] == b'/'
        && bytes.get(start + 1).is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b')' | b']' | b'}')
        });
    if boundary && (windows || unc || unix) {
        let end = sensitive_token_end(text, start + usize::from(windows) * 2);
        if MODEL_EXTENSIONS.iter().any(|extension| {
            let path = &text[start..end];
            path.get(path.len().saturating_sub(extension.len())..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(extension))
        }) {
            return Some(SensitiveSpan { start, end });
        }
    }
    None
}

fn sensitive_token_end(text: &str, content_start: usize) -> usize {
    text[content_start..]
        .find(|character: char| {
            character.is_whitespace()
                || matches!(character, '"' | '\'' | ',' | ')' | ']' | '}' | '<' | '>')
        })
        .map_or(text.len(), |end| content_start + end)
}

#[cfg(test)]
#[path = "redact-tests.rs"]
mod tests;
