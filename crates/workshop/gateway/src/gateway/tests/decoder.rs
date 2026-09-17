//! SSE decoder tests: chunking invariance, multi-line events, CRLF, EOF
//! flush, and data-less event drops.

use crate::gateway::sse::SseDecoder;
use workshop_support::xorshift;

fn drain(decoder: &mut SseDecoder) -> Vec<String> {
    let mut payloads = Vec::new();
    while let Some(payload) = decoder.pop() {
        payloads.push(payload);
    }
    payloads
}

#[test]
fn decoder_emits_the_same_events_regardless_of_chunk_boundaries() {
    let wire = "data: {\"a\":1}\n\ndata: [DONE]\n\n";
    let mut whole = SseDecoder::default();
    whole.feed(wire.as_bytes());
    whole.finish();

    let mut drip = SseDecoder::default();
    for byte in wire.as_bytes() {
        drip.feed(std::slice::from_ref(byte));
    }
    drip.finish();

    let whole_out = drain(&mut whole);
    assert_eq!(drain(&mut drip), whole_out, "chunking must not matter");
    assert_eq!(
        whole_out,
        ["{\"a\":1}".to_string(), "[DONE]".to_string()],
        "payloads arrive verbatim, including the terminal sentinel"
    );
}

#[test]
fn decoder_joins_multi_line_data_and_ignores_other_fields() {
    let wire = ": comment\nevent: message\nid: 7\ndata: first\ndata: second\n\n";
    let mut decoder = SseDecoder::default();
    decoder.feed(wire.as_bytes());
    decoder.finish();
    assert_eq!(decoder.pop().as_deref(), Some("first\nsecond"));
    assert!(decoder.pop().is_none(), "one event, one payload");
}

#[test]
fn decoder_accepts_crlf_line_endings() {
    let mut decoder = SseDecoder::default();
    decoder.feed(b"data: one\r\n\r\n");
    decoder.finish();
    assert_eq!(decoder.pop().as_deref(), Some("one"));
}

#[test]
fn decoder_flushes_an_unterminated_final_line_at_eof() {
    let mut decoder = SseDecoder::default();
    decoder.feed(b"data: tail");
    decoder.finish();
    assert_eq!(decoder.pop().as_deref(), Some("tail"));
}

#[test]
fn decoder_drops_events_without_data() {
    let mut decoder = SseDecoder::default();
    decoder.feed(b"event: ping\n\ndata: kept\n\n");
    decoder.finish();
    assert_eq!(decoder.pop().as_deref(), Some("kept"));
    assert!(decoder.pop().is_none());
}

/// A realistic delta stream whose payloads carry multi-byte UTF-8
/// (2-, 3-, and 4-byte codepoints), so a byte split can land inside a
/// codepoint; mixed CRLF/LF endings and a multi-line event ride along.
const MULTIBYTE_WIRE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"h\u{e9}llo \u{1f914} w\u{f6}rld\"}}]}\r\n\r\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"\u{65e5}\u{672c}\u{8a9e}\"}}]}\n\n",
    "data: first\ndata: \u{1f40d} second\n\n",
    "data: [DONE]\n\n",
);

/// The payloads [`MULTIBYTE_WIRE`] must always decode to, regardless
/// of how the bytes are chunked.
fn multibyte_payloads() -> Vec<String> {
    vec![
        "{\"choices\":[{\"delta\":{\"reasoning_content\":\"h\u{e9}llo \u{1f914} w\u{f6}rld\"}}]}"
            .to_string(),
        "{\"choices\":[{\"delta\":{\"content\":\"\u{65e5}\u{672c}\u{8a9e}\"}}]}".to_string(),
        "first\n\u{1f40d} second".to_string(),
        "[DONE]".to_string(),
    ]
}

/// Feeds `wire` split at the ascending byte offsets in `cuts`, then
/// returns everything the decoder produced.
fn decode_in_fragments(wire: &[u8], cuts: &[usize]) -> Vec<String> {
    let mut decoder = SseDecoder::default();
    let mut start = 0;
    for &cut in cuts {
        decoder.feed(&wire[start..cut]);
        start = cut;
    }
    decoder.feed(&wire[start..]);
    decoder.finish();
    drain(&mut decoder)
}

#[test]
fn decoder_survives_every_split_point_including_mid_utf8() {
    let wire = MULTIBYTE_WIRE.as_bytes();
    let expected = multibyte_payloads();
    assert_eq!(decode_in_fragments(wire, &[]), expected, "unsplit feed");
    // Every split point, so every mid-codepoint boundary is exercised.
    for cut in 1..wire.len() {
        assert_eq!(
            decode_in_fragments(wire, &[cut]),
            expected,
            "split at byte {cut} changed the decode"
        );
    }
}

#[test]
fn decoder_is_chunking_invariant_under_random_splits() {
    let wire = MULTIBYTE_WIRE.as_bytes();
    let expected = multibyte_payloads();
    for seed in [0x9E37_79B9_7F4A_7C15_u64, 42, 7_777_777] {
        let mut state = seed;
        let cuts: Vec<usize> = (1..wire.len())
            .filter(|_| xorshift(&mut state).is_multiple_of(4))
            .collect();
        assert_eq!(
            decode_in_fragments(wire, &cuts),
            expected,
            "seed {seed}: fragmenting at {cuts:?} changed the decode"
        );
    }
}
