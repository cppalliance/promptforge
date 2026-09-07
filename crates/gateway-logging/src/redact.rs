//! The privacy pass every queued record goes through.
//!
//! No log record may carry credentials, cookies, authorization headers,
//! environment values, request bodies, audio, transcript text, prompts, or
//! full local model paths. Most of that list is call-site discipline -
//! the gateway never logs payloads - but the well-shaped secrets (bearer
//! tokens, authorization and cookie header values, `api_key` assignments)
//! can leak through an interpolated error or a debug-formatted structure,
//! so the one chokepoint every record crosses masks them on the way in.
//!
//! The patterns are ASCII and matched case-insensitively where a header
//! name is involved; redaction never reorders or truncates the rest of
//! the line.

/// The mask replacing a sensitive value.
const REDACTED: &str = "[redacted]";

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

    fn from_str(text: &str, capacity: usize) -> Self {
        let mut bounded = Self::new(capacity);
        bounded.push_str(text);
        bounded
    }

    fn capacity(&self) -> usize {
        self.storage.len()
    }

    fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.storage[..self.len])
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

/// Masks the sensitive shapes `text` could carry without permitting any
/// intermediate output buffer to exceed `capacity`.
pub(crate) fn redact_line_bounded(text: &str, capacity: usize) -> Option<RedactedLine> {
    let text = RedactedLine::from_str(text, capacity);
    let text = redact_header_values(text, "authorization:")?;
    let text = redact_header_values(text, "cookie:")?;
    let text = redact_header_values(text, "set-cookie:")?;
    let text = redact_bearer_tokens(text)?;
    redact_api_key_assignments(text)
}

/// Masks the sensitive shapes `text` could carry and returns the result.
/// Tests use this convenience path; production supplies its strict record
/// capacity through [`redact_line_bounded`].
#[cfg(test)]
pub(crate) fn redact_line(text: &str) -> String {
    let capacity = text.len().saturating_mul(REDACTED.len());
    redact_line_bounded(text, capacity)
        .and_then(|redacted| redacted.finish("", false))
        .filter(|(_, truncated)| !truncated)
        .map_or_else(String::new, |(text, _)| text.into())
}

/// The first position where `needle` matches `haystack` at or after
/// `from`, comparing ASCII case-insensitively. Byte offsets stay valid
/// because only ASCII needles are ever searched.
fn find_ascii(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack.as_bytes()[from..]
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
        .map(|offset| from + offset)
}

/// Redacts everything after a header name up to the end of the line: an
/// `Authorization:` or `Cookie:` value runs to the line's end in the
/// one-line-per-event format the fmt layer produces.
fn redact_header_values(input: RedactedLine, header: &str) -> Option<RedactedLine> {
    if find_ascii(input.as_str().ok()?, header, 0).is_none() {
        return Some(input);
    }
    let capacity = input.capacity();
    let inherited_truncation = input.truncated;
    let mut out = RedactedLine::new(capacity);
    let mut rest = input.as_str().ok()?;
    while let Some(start) = find_ascii(rest, header, 0) {
        let mut value_start = start + header.len();
        // The conventional space after the colon is kept with the name.
        while rest.as_bytes().get(value_start) == Some(&b' ') {
            value_start += 1;
        }
        let value_end = rest[value_start..]
            .find('\n')
            .map_or(rest.len(), |newline| value_start + newline);
        out.push_str(&rest[..value_start]);
        out.push_str(REDACTED);
        rest = &rest[value_end..];
    }
    out.push_str(rest);
    out.truncated |= inherited_truncation;
    Some(out)
}

/// Redacts the token after `Bearer `, the shape an authorization value
/// takes when it appears without its header name (an interpolated error,
/// a URL query). The token is the run of non-whitespace following the
/// scheme.
fn redact_bearer_tokens(input: RedactedLine) -> Option<RedactedLine> {
    const SCHEME: &str = "bearer ";
    if find_ascii(input.as_str().ok()?, SCHEME, 0).is_none() {
        return Some(input);
    }
    let capacity = input.capacity();
    let inherited_truncation = input.truncated;
    let mut out = RedactedLine::new(capacity);
    let mut rest = input.as_str().ok()?;
    let mut from = 0;
    while let Some(start) = find_ascii(rest, SCHEME, from) {
        let token_start = start + SCHEME.len();
        let token_end = rest[token_start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |space| token_start + space);
        if token_end == token_start {
            from = token_start;
            continue;
        }
        out.push_str(&rest[..token_start]);
        out.push_str(REDACTED);
        rest = &rest[token_end..];
        from = 0;
    }
    out.push_str(rest);
    out.truncated |= inherited_truncation;
    Some(out)
}

/// Redacts the value of an `api_key` assignment in the shapes configs and
/// JSON take: `api_key = "v"`, `api_key="v"`, `"api_key": "v"`, and bare
/// `api_key = v`. The key name is kept so the log still says which field
/// was masked.
fn redact_api_key_assignments(input: RedactedLine) -> Option<RedactedLine> {
    const KEY: &str = "api_key";
    if find_ascii(input.as_str().ok()?, KEY, 0).is_none() {
        return Some(input);
    }
    let capacity = input.capacity();
    let inherited_truncation = input.truncated;
    let mut out = RedactedLine::new(capacity);
    let mut rest = input.as_str().ok()?;
    let mut from = 0;
    while let Some(start) = find_ascii(rest, KEY, from) {
        let after_key = start + KEY.len();
        let bytes = rest.as_bytes();
        let mut cursor = after_key;
        // The JSON shape quotes the key: `"api_key": "v"`.
        if cursor < bytes.len() && bytes[cursor] == b'"' {
            cursor += 1;
        }
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'\n'
        {
            cursor += 1;
        }
        // Only an assignment redacts: a bare mention of the field name is
        // not a leak.
        if cursor >= bytes.len() || (bytes[cursor] != b'=' && bytes[cursor] != b':') {
            from = after_key;
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'\n'
        {
            cursor += 1;
        }
        let quoted = cursor < bytes.len() && bytes[cursor] == b'"';
        if quoted {
            cursor += 1;
        }
        let value_start = cursor;
        let value_end = if quoted {
            rest[value_start..]
                .find('"')
                .map_or(rest.len(), |quote| value_start + quote)
        } else {
            rest[value_start..]
                .find(|c: char| c.is_whitespace() || c == ',')
                .map_or(rest.len(), |end| value_start + end)
        };
        if value_end == value_start {
            from = after_key;
            continue;
        }
        out.push_str(&rest[..value_start]);
        out.push_str(REDACTED);
        rest = &rest[value_end..];
        from = 0;
    }
    out.push_str(rest);
    out.truncated |= inherited_truncation;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_authorization_header_value_is_masked() {
        let redacted = redact_line("sending Authorization: Bearer abc123secret to upstream");
        assert!(
            !redacted.contains("abc123secret"),
            "the bearer token never reaches a record: {redacted}"
        );
        assert!(
            redacted.contains("Authorization: [redacted]"),
            "the header name survives so the log stays legible: {redacted}"
        );
    }

    #[test]
    fn a_lowercase_authorization_header_is_masked() {
        let redacted = redact_line("header authorization: basic dXNlcg== rejected");
        assert!(
            !redacted.contains("dXNlcg=="),
            "HTTP/2's lowercase header names redact the same way: {redacted}"
        );
    }

    #[test]
    fn cookie_and_set_cookie_values_are_masked() {
        let redacted = redact_line("request Cookie: session=xyz789; other=1\nnext line");
        assert!(
            !redacted.contains("xyz789"),
            "the cookie value never reaches a record: {redacted}"
        );
        assert!(
            redacted.contains("next line"),
            "redaction stops at the end of the header's line: {redacted}"
        );
        let redacted = redact_line("response Set-Cookie: token=abc; HttpOnly");
        assert!(
            !redacted.contains("token=abc"),
            "a set-cookie value never reaches a record: {redacted}"
        );
    }

    #[test]
    fn a_bare_bearer_token_is_masked() {
        let redacted = redact_line("upstream rejected Bearer tok_live_51xyz with 401");
        assert!(
            !redacted.contains("tok_live_51xyz"),
            "a bearer token without its header name is still masked: {redacted}"
        );
        assert!(
            redacted.contains("Bearer [redacted]"),
            "the scheme survives: {redacted}"
        );
    }

    #[test]
    fn api_key_assignments_are_masked_in_toml_and_json_shapes() {
        for (line, secret) in [
            ("api_key = \"toml-secret\"", "toml-secret"),
            ("api_key=\"compact-secret\"", "compact-secret"),
            ("{\"api_key\": \"json-secret\"}", "json-secret"),
            ("api_key: bare-secret, done", "bare-secret"),
        ] {
            let redacted = redact_line(line);
            assert!(
                !redacted.contains(secret),
                "the api_key value never reaches a record: {redacted}"
            );
            assert!(
                redacted.contains("api_key"),
                "the field name survives: {redacted}"
            );
        }
    }

    #[test]
    fn an_ordinary_line_passes_through_unchanged() {
        let line = "loaded profile main with 2 models; bind 127.0.0.1:8081";
        assert_eq!(
            redact_line(line),
            line,
            "a line without a sensitive shape is byte-identical"
        );
    }

    #[test]
    fn a_field_name_mention_without_a_value_is_not_a_leak() {
        let line = "the api_key field is required";
        assert_eq!(
            redact_line(line),
            line,
            "naming the field redacts nothing: no assignment follows"
        );
    }
}
