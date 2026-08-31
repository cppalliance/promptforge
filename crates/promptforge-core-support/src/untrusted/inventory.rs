//! The control-markup delimiter inventory and its single-pass matcher.
//!
//! Chat templates address their models through control markup: role headers,
//! turn boundaries, tool-call envelopes, media placeholders, and document
//! boundaries. Untrusted content quoting one verbatim can forge a turn or
//! close a block the template opened, so the envelope's encoding pass spaces
//! the opener of every delimiter listed here. The inventory is ported from
//! the upstream closed name list and stays closed on purpose: `<div>`,
//! `List<String>`, `[1]`, and lowercase `[inst]` are prose and stay as
//! typed. It includes structural tokens that tokenizers do not flag as
//! special, because a string sweep keyed on the special-token flag misses
//! real delimiters.
//!
//! DeepSeek's fullwidth markers (`<｜name｜>`, U+FF5C bars) are the one open
//! name class - the family keeps adding spellings - so they match as a
//! bounded class rather than as table entries.

/// The opener shape shared by every entry in a [`DelimiterGroup`].
#[derive(Clone, Copy, Debug)]
pub(super) enum Shape {
    /// ChatML-style pipe tokens: `<|name|>` and `<|name>`, plus the
    /// `<|/name|>` and `<|/name>` closers some families emit.
    Pipe,
    /// Bare XML-style tags: `<name>` and `</name>`.
    BareTag,
    /// A literal opener matched verbatim: attribute openers, reverse pairs,
    /// and the bracket family.
    Literal,
    /// Llama-2's doubled-angle system block `<<SYS>>`, anchored on the second
    /// bracket so `<SYS>>`, `cout << SYS`, and heredocs stay as typed.
    DoubledAngle,
}

/// One family group of the control-markup delimiter inventory.
#[derive(Debug)]
pub(super) struct DelimiterGroup {
    /// The model family or protocol whose templates emit these delimiters.
    // Read by the table sanity tests; live matching keys on shape and names.
    #[allow(dead_code)]
    pub(super) family: &'static str,
    /// How each entry in `names` spells its opener.
    pub(super) shape: Shape,
    /// Delimiter names or literal openers, interpreted per `shape`.
    pub(super) names: &'static [&'static str],
}

/// The control-markup delimiter inventory, grouped by emitting family.
///
/// Ported from the upstream `_CONTROL_MARKUP` name list. Matching only ever
/// needs the opener: one inserted space after the opening `<` or `[` breaks
/// the delimiter for the template, the think-block extractor, and the
/// stop-sequence matcher while leaving the text readable.
pub(super) static CONTROL_MARKUP: &[DelimiterGroup] = &[
    // Llama-3 role and turn structure.
    DelimiterGroup {
        family: "llama-3",
        shape: Shape::Pipe,
        names: &[
            "start_header_id",
            "end_header_id",
            "start_of_role",
            "end_of_role",
        ],
    },
    // Phi-4 Mini's tool envelope; it closes with `<|/tool|>` rather than a
    // separate closing name, so the closers are part of the shape.
    DelimiterGroup {
        family: "phi-4 tool envelope",
        shape: Shape::Pipe,
        names: &["tool", "tool_call", "tool_response"],
    },
    // Kimi K2 / Moonshot wrap history in a section and each call in a
    // begin/end pair; a paste carrying one fabricates a historical call.
    DelimiterGroup {
        family: "kimi k2 tool sections",
        shape: Shape::Pipe,
        names: &[
            "tool_call_begin",
            "tool_call_end",
            "tool_calls_begin",
            "tool_calls_end",
            "tool_call_section_begin",
            "tool_call_section_end",
            "tool_calls_section_begin",
            "tool_calls_section_end",
            "tool_call_argument_begin",
        ],
    },
    // Turn and text terminators shared across families.
    DelimiterGroup {
        family: "turn terminators",
        shape: Shape::Pipe,
        names: &["end", "end_of_turn", "end_of_text"],
    },
    // Document boundaries: Llama-3.1 / Llama-4's BOS and the GPT-2-lineage
    // EOS that Qwen, Phi, gpt-oss, and GLM-4.5 still carry. A pasted copy
    // lands mid-conversation as a document break the template never opened.
    DelimiterGroup {
        family: "document boundaries",
        shape: Shape::Pipe,
        names: &["begin_of_text", "endoftext"],
    },
    // Llama-4's spelling of Llama-3's role headers.
    DelimiterGroup {
        family: "llama-4 headers",
        shape: Shape::Pipe,
        names: &["header_start", "header_end"],
    },
    // The ChatML three, Phi-4's role separator, and Kimi K2's pair.
    DelimiterGroup {
        family: "chatml and kimi k2 role sentinels",
        shape: Shape::Pipe,
        names: &[
            "im_start",
            "im_end",
            "im_sep",
            "im_system",
            "im_middle",
            "im_user",
            "im_assistant",
        ],
    },
    // DeepSeek-V4 spells role boundaries with ASCII bars and a capital,
    // unlike R1's fullwidth class below; the match is case-sensitive.
    DelimiterGroup {
        family: "deepseek v4 role boundaries",
        shape: Shape::Pipe,
        names: &["User", "Assistant", "System"],
    },
    // gpt-oss Harmony channels and message sentinels.
    DelimiterGroup {
        family: "gpt-oss harmony",
        shape: Shape::Pipe,
        names: &[
            "assistant",
            "constrain",
            "channel",
            "message",
            "eot",
            "eom",
            "eot_id",
            "eom_id",
            "final",
        ],
    },
    // TML Inkling's call envelope; longer than the plain `message` and `end`
    // names, so all three pass through a sweep keyed on those alone.
    DelimiterGroup {
        family: "tml inkling call envelope",
        shape: Shape::Pipe,
        names: &["message_model", "content_invoke_tool_json", "end_message"],
    },
    // Command-R / Aya spell every delimiter in caps.
    DelimiterGroup {
        family: "command-r turn tokens",
        shape: Shape::Pipe,
        names: &[
            "START_OF_TURN_TOKEN",
            "END_OF_TURN_TOKEN",
            "USER_TOKEN",
            "SYSTEM_TOKEN",
            "CHATBOT_TOKEN",
        ],
    },
    // Gemma-4 media placeholders and Llama-3.1's built-in-tool sentinel. A
    // pasted one is counted as media with nothing behind it, a hard error
    // out of the processor.
    DelimiterGroup {
        family: "media placeholders",
        shape: Shape::Pipe,
        names: &["image", "audio", "video", "python_tag"],
    },
    // Qwen 2.5 Coder's fill-in-the-middle prompt tokens.
    DelimiterGroup {
        family: "qwen fill-in-the-middle",
        shape: Shape::Pipe,
        names: &["fim_prefix", "fim_suffix", "fim_middle"],
    },
    // Qwen2-VL / Qwen2.5-VL reserve these for the processor, which expands a
    // pad token per image or video patch.
    DelimiterGroup {
        family: "qwen vision placeholders",
        shape: Shape::Pipe,
        names: &[
            "vision_start",
            "vision_end",
            "vision_pad",
            "image_pad",
            "video_pad",
        ],
    },
    // Structural tokens not every tokenizer flags as special; a sweep keyed
    // on the special-token flag alone would miss them.
    DelimiterGroup {
        family: "non-special structural tokens",
        shape: Shape::Pipe,
        names: &["return", "system", "start", "think", "turn", "user", "call"],
    },
    // Gemma's turn boundaries.
    DelimiterGroup {
        family: "gemma turn boundaries",
        shape: Shape::BareTag,
        names: &["start_of_turn", "end_of_turn"],
    },
    // A `</tools>` in client text closes the real Qwen / Hermes tool catalog
    // and the rest reads as undeclared tools.
    DelimiterGroup {
        family: "qwen and hermes tool blocks",
        shape: Shape::BareTag,
        names: &["tool_call", "tool_response", "tools"],
    },
    // Reasoning blocks: a forged `</think>` ends the reasoning block early.
    DelimiterGroup {
        family: "reasoning blocks",
        shape: Shape::BareTag,
        names: &["think"],
    },
    // Llama-2 / Mistral / Zephyr BOS and EOS. `<s>` collides with a real
    // HTML tag; a live document boundary beats a space in a rare `<s>`.
    DelimiterGroup {
        family: "llama-2 and mistral document boundaries",
        shape: Shape::BareTag,
        names: &["eos", "bos", "s", "sop"],
    },
    // Gemma 3 / 3n media placeholders.
    DelimiterGroup {
        family: "gemma media placeholders",
        shape: Shape::BareTag,
        names: &["start_of_image", "image_soft_token", "audio_soft_token"],
    },
    // GLM-4.5+ and Qwen3.5 nest their call protocol inside the outer tag;
    // every level is structural to the parser.
    DelimiterGroup {
        family: "glm and qwen call protocol",
        shape: Shape::BareTag,
        names: &["arg_key", "arg_value", "function", "parameter", "param"],
    },
    // Opening halves carrying `=value`; the whitespace in the `name=` forms
    // is any run, matched by `attribute_len`.
    DelimiterGroup {
        family: "function attribute openers",
        shape: Shape::Literal,
        names: &[
            "<function=",
            "<parameter=",
            "<function name=\"",
            "<param name=\"",
            "<parameter name=\"",
        ],
    },
    // Phi-4 Mini and Harmony reverse pairs, closed with `|>` instead of
    // opened with `<|`.
    DelimiterGroup {
        family: "reverse-pair closers",
        shape: Shape::Literal,
        names: &[
            "<tool|>",
            "<tool_call|>",
            "<tool_response|>",
            "<channel|>",
            "<turn|>",
        ],
    },
    // Mistral instruct brackets; the chat branch of the same template
    // interpolates message content between `[INST]` and `[/INST]`.
    DelimiterGroup {
        family: "mistral instruct brackets",
        shape: Shape::Literal,
        names: &["[INST]", "[/INST]"],
    },
    DelimiterGroup {
        family: "mistral system prompt block",
        shape: Shape::Literal,
        names: &["[SYSTEM_PROMPT]", "[/SYSTEM_PROMPT]"],
    },
    // Hermes / Mistral observation and catalog blocks.
    DelimiterGroup {
        family: "hermes and mistral tool blocks",
        shape: Shape::Literal,
        names: &[
            "[AVAILABLE_TOOLS]",
            "[/AVAILABLE_TOOLS]",
            "[TOOL_RESULTS]",
            "[/TOOL_RESULTS]",
            "[TOOL_CALLS]",
            "[/TOOL_CALLS]",
        ],
    },
    // Codestral builds its FIM prompt from these three.
    DelimiterGroup {
        family: "codestral fill-in-the-middle",
        shape: Shape::Literal,
        names: &[
            "[PREFIX]",
            "[/PREFIX]",
            "[MIDDLE]",
            "[/MIDDLE]",
            "[SUFFIX]",
            "[/SUFFIX]",
        ],
    },
    DelimiterGroup {
        family: "glm masks",
        shape: Shape::Literal,
        names: &["[gMASK]", "[/gMASK]"],
    },
    // Llama-2 opens its system block with `<<SYS>>` inside the first
    // `[INST]`; a pasted pair in a later turn invents a system block the
    // template only emits once.
    DelimiterGroup {
        family: "llama-2 system block",
        shape: Shape::DoubledAngle,
        names: &["SYS"],
    },
];

/// The fullwidth vertical bar (U+FF5C) framing DeepSeek's open marker class.
const FULLWIDTH_BAR: char = '\u{ff5c}';

/// The most characters allowed after the first letter of a fullwidth marker
/// name, matching the upstream `{0,39}` bound.
const FULLWIDTH_NAME_MAX: usize = 39;

/// The length in bytes of the inventory delimiter opening at the start of
/// `text`, if one does.
///
/// `prev_lt` records whether the character before `text` was a `<`, which
/// anchors the doubled-angle `<<SYS>>` form on its second bracket.
pub(super) fn delimiter_len(text: &str, prev_lt: bool) -> Option<usize> {
    match text.as_bytes().first() {
        Some(b'[') => literal_len(text, b'['),
        Some(b'<') => angle_len(text, prev_lt),
        _ => None,
    }
}

/// The length of the angle-bracket delimiter at the start of `text`.
fn angle_len(text: &str, prev_lt: bool) -> Option<usize> {
    pipe_len(text)
        .or_else(|| bare_tag_len(text))
        .or_else(|| literal_len(text, b'<'))
        .or_else(|| attribute_len(text))
        .or_else(|| fullwidth_len(text))
        .or_else(|| doubled_angle_len(text, prev_lt))
}

/// The length of the `<|name|>`-family delimiter at the start of `text`.
///
/// The closer form `<|/name|>` shares the opener. The name must be followed
/// by `|>` or `>`, so `<|tool_call|>` cannot match as the shorter `tool`.
fn pipe_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("<|")?;
    let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::Pipe) {
            continue;
        }
        for name in group.names {
            let Some(after) = rest.strip_prefix(name) else {
                continue;
            };
            let term = if after.starts_with("|>") {
                2
            } else if after.starts_with('>') {
                1
            } else {
                continue;
            };
            return Some(2 + close + name.len() + term);
        }
    }
    None
}

/// The length of the `<name>`-family bare tag at the start of `text`.
fn bare_tag_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::BareTag) {
            continue;
        }
        for name in group.names {
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with('>')
            {
                return Some(1 + close + name.len() + 1);
            }
        }
    }
    None
}

/// The length of the literal opener at the start of `text` whose first byte
/// is `open`.
fn literal_len(text: &str, open: u8) -> Option<usize> {
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::Literal) {
            continue;
        }
        for lit in group.names {
            if lit.as_bytes().first() == Some(&open) && text.starts_with(lit) {
                return Some(lit.len());
            }
        }
    }
    None
}

/// The length of the `<function=...` / `<name param="...">` attribute opener
/// at the start of `text`, where the `name=` form allows any whitespace run.
fn attribute_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    for name in ["function", "parameter"] {
        if let Some(after) = rest.strip_prefix(name)
            && after.starts_with('=')
        {
            return Some(1 + name.len() + 1);
        }
    }
    for name in ["function", "parameter", "param"] {
        if let Some(after) = rest.strip_prefix(name) {
            let trimmed = after.trim_start_matches(char::is_whitespace);
            let ws = after.len() - trimmed.len();
            if ws > 0 && trimmed.starts_with("name=\"") {
                return Some(1 + name.len() + ws + "name=\"".len());
            }
        }
    }
    None
}

/// The length of the fullwidth `<｜name｜>` marker at the start of `text`.
///
/// The one open name class: an ASCII letter, then at most
/// [`FULLWIDTH_NAME_MAX`] joiner characters, then the closing bar. The
/// charset restriction keeps the class off real CJK content.
fn fullwidth_len(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    let mut rest = rest.strip_prefix(FULLWIDTH_BAR)?;
    let first = rest.chars().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    rest = &rest[first.len_utf8()..];
    let mut total = 1 + FULLWIDTH_BAR.len_utf8() + first.len_utf8();
    let mut extra = 0usize;
    loop {
        let c = rest.chars().next()?;
        if c == FULLWIDTH_BAR {
            let after = &rest[FULLWIDTH_BAR.len_utf8()..];
            return after
                .starts_with('>')
                .then_some(total + FULLWIDTH_BAR.len_utf8() + 1);
        }
        if extra >= FULLWIDTH_NAME_MAX || !is_fullwidth_name_char(c) {
            return None;
        }
        extra += 1;
        total += c.len_utf8();
        rest = &rest[c.len_utf8()..];
    }
}

/// Whether `c` may continue a fullwidth marker name: ASCII letters, the
/// one-quarter-block joiner DeepSeek uses, underscore, space, or backslash
/// (the upstream parser also recognizes backslash-escaped spellings).
fn is_fullwidth_name_char(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{2581}' | '_' | ' ' | '\\')
}

/// The length of the `<<SYS>>` system-block opener at the start of `text`,
/// anchored on the second bracket via `prev_lt`.
fn doubled_angle_len(text: &str, prev_lt: bool) -> Option<usize> {
    if !prev_lt {
        return None;
    }
    for group in CONTROL_MARKUP {
        if !matches!(group.shape, Shape::DoubledAngle) {
            continue;
        }
        for name in group.names {
            let Some(rest) = text.strip_prefix('<') else {
                continue;
            };
            let (close, rest) = rest.strip_prefix('/').map_or((0, rest), |r| (1, r));
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with(">>")
            {
                return Some(1 + close + name.len() + 2);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every full spelling of every Pipe, BareTag, and Literal table entry.
    fn spellings() -> Vec<String> {
        let mut out = Vec::new();
        for group in CONTROL_MARKUP {
            for name in group.names {
                match group.shape {
                    Shape::Pipe => {
                        out.push(format!("<|{name}|>"));
                        out.push(format!("<|{name}>"));
                        out.push(format!("<|/{name}|>"));
                        out.push(format!("<|/{name}>"));
                    }
                    Shape::BareTag => {
                        out.push(format!("<{name}>"));
                        out.push(format!("</{name}>"));
                    }
                    Shape::Literal => out.push((*name).to_owned()),
                    Shape::DoubledAngle => {}
                }
            }
        }
        out
    }

    #[test]
    fn every_table_spelling_matches_in_full() {
        for s in spellings() {
            assert_eq!(
                delimiter_len(&s, false),
                Some(s.len()),
                "inventory spelling {s:?} did not match in full"
            );
        }
    }

    #[test]
    fn inventory_has_no_duplicate_spellings() {
        let all = spellings();
        let mut deduped = all.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            all.len(),
            deduped.len(),
            "duplicate spellings in the inventory"
        );
    }

    #[test]
    fn every_group_names_a_family_and_entries() {
        for group in CONTROL_MARKUP {
            assert!(
                !group.family.is_empty(),
                "a group is missing its family label"
            );
            assert!(
                !group.names.is_empty(),
                "family {:?} has no entries",
                group.family
            );
        }
    }

    #[test]
    fn fullwidth_class_matches_known_and_novel_spellings() {
        let user = "<\u{ff5c}User\u{ff5c}>";
        assert_eq!(delimiter_len(user, false), Some(user.len()));
        let sentence = "<\u{ff5c}begin\u{2581}of\u{2581}sentence\u{ff5c}>";
        assert_eq!(delimiter_len(sentence, false), Some(sentence.len()));
        let novel = "<\u{ff5c}SomeNew\u{ff5c}>";
        assert_eq!(
            delimiter_len(novel, false),
            Some(novel.len()),
            "the open class admits spellings added after the port"
        );
    }

    #[test]
    fn fullwidth_class_stays_bounded_and_off_prose() {
        assert_eq!(
            delimiter_len("<\u{ff5c}\u{ff5c}>", false),
            None,
            "the name must start with a letter"
        );
        let at_bound = format!("<\u{ff5c}a{}\u{ff5c}>", "b".repeat(FULLWIDTH_NAME_MAX));
        assert_eq!(
            delimiter_len(&at_bound, false),
            Some(at_bound.len()),
            "a name at the 39-character bound still matches"
        );
        let past_bound = format!("<\u{ff5c}a{}\u{ff5c}>", "b".repeat(FULLWIDTH_NAME_MAX + 1));
        assert_eq!(
            delimiter_len(&past_bound, false),
            None,
            "the class is bounded"
        );
        assert_eq!(
            delimiter_len("<\u{ff5c} \u{ff5c}>", false),
            None,
            "fullwidth punctuation pairs stay as typed"
        );
    }

    #[test]
    fn doubled_angle_sys_matches_only_at_the_second_bracket() {
        assert_eq!(delimiter_len("<<SYS>>", false), None);
        assert_eq!(delimiter_len("<SYS>>", true), Some("<SYS>>".len()));
        assert_eq!(delimiter_len("</SYS>>", true), Some("</SYS>>".len()));
        assert_eq!(
            delimiter_len("<SYS>>", false),
            None,
            "a single angle stays as typed"
        );
        assert_eq!(
            delimiter_len("< SYS>>", true),
            None,
            "the spaced form is stable"
        );
    }

    #[test]
    fn closed_inventory_leaves_prose_alone() {
        let prose = [
            "<div>",
            "<span class=\"x\">",
            "List<String>",
            "[1]",
            "[inst]",
            "[UNKNOWN]",
            "<s >",
            "<tool>",
            "<|tool|",
            "<|unknown_name|>",
        ];
        for text in prose {
            assert_eq!(
                delimiter_len(text, false),
                None,
                "{text:?} must stay as typed"
            );
        }
    }
}
