//! The control-markup delimiter table: every family group and the opener
//! shape its entries share.

/// The opener shape shared by every entry in a [`DelimiterGroup`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
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
pub(crate) struct DelimiterGroup {
    /// The model family or protocol whose templates emit these delimiters.
    // Read by the table sanity tests; live matching keys on shape and names.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by the table sanity tests; live matching keys on shape and names"
        )
    )]
    pub(crate) family: &'static str,
    /// How each entry in `names` spells its opener.
    pub(crate) shape: Shape,
    /// Delimiter names or literal openers, interpreted per `shape`.
    pub(crate) names: &'static [&'static str],
}

/// The control-markup delimiter inventory, grouped by emitting family.
///
/// Ported from the upstream `_CONTROL_MARKUP` name list. Matching only ever
/// needs the opener: one inserted space after the opening `<` or `[` breaks
/// the delimiter for the template, the think-block extractor, and the
/// stop-sequence matcher while leaving the text readable.
pub(crate) static CONTROL_MARKUP: &[DelimiterGroup] = &[
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
    // begin/end pair; a paste holding one fabricates a historical call.
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
    // EOS that Qwen, Phi, gpt-oss, and GLM-4.5 still include. A pasted copy
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
    // Opening halves with `=value`; the whitespace in the `name=` forms
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
