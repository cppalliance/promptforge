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

/// Masks the sensitive shapes `text` could carry and returns the result.
/// The input passes through unchanged when nothing matches, which is the
/// common case.
pub(crate) fn redact_line(text: &str) -> String {
    let text = redact_header_values(text, "authorization:");
    let text = redact_header_values(&text, "cookie:");
    let text = redact_header_values(&text, "set-cookie:");
    let text = redact_bearer_tokens(&text);
    redact_api_key_assignments(&text)
}

/// The mask replacing a sensitive value.
const REDACTED: &str = "[redacted]";

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
fn redact_header_values(text: &str, header: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
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
    out
}

/// Redacts the token after `Bearer `, the shape an authorization value
/// takes when it appears without its header name (an interpolated error,
/// a URL query). The token is the run of non-whitespace following the
/// scheme.
fn redact_bearer_tokens(text: &str) -> String {
    const SCHEME: &str = "bearer ";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
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
    out
}

/// Redacts the value of an `api_key` assignment in the shapes configs and
/// JSON take: `api_key = "v"`, `api_key="v"`, `"api_key": "v"`, and bare
/// `api_key = v`. The key name is kept so the log still says which field
/// was masked.
fn redact_api_key_assignments(text: &str) -> String {
    const KEY: &str = "api_key";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
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
    out
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
