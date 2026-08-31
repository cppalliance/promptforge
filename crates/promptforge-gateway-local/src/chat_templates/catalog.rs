//! Family metadata and embedded Jinja assets.

use super::Family;

impl Family {
    /// Every family in stable catalog order.
    pub const ALL: [Self; 12] = [
        Self::Chatml,
        Self::Llama3,
        Self::Llama31,
        Self::Qwen25,
        Self::Qwen3,
        Self::Gemma3,
        Self::Gemma4,
        Self::Mistral,
        Self::Phi3,
        Self::Phi4,
        Self::GptOss,
        Self::Zephyr,
    ];

    /// Returns the stable family name used by `builtin:<family>` values.
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Chatml => "chatml",
            Self::Llama3 => "llama-3",
            Self::Llama31 => "llama-3.1",
            Self::Qwen25 => "qwen-2.5",
            Self::Qwen3 => "qwen-3",
            Self::Gemma3 => "gemma-3",
            Self::Gemma4 => "gemma-4",
            Self::Mistral => "mistral",
            Self::Phi3 => "phi-3",
            Self::Phi4 => "phi-4",
            Self::GptOss => "gpt-oss",
            Self::Zephyr => "zephyr",
        }
    }

    /// Parses a canonical name or documented alias.
    ///
    /// Matching is ASCII-case-insensitive after trimming surrounding
    /// whitespace.
    #[must_use]
    pub fn parse_alias(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatml" => Some(Self::Chatml),
            "llama-3" | "llama3" => Some(Self::Llama3),
            "llama-3.1" | "llama-31" | "llama-3.2" | "llama-3.3" | "llama-32" | "llama-33" => {
                Some(Self::Llama31)
            }
            "qwen-2.5" | "qwen-25" | "qwen25" | "qwen2.5" => Some(Self::Qwen25),
            "qwen-3" | "qwen3" => Some(Self::Qwen3),
            "gemma-3" | "gemma3" => Some(Self::Gemma3),
            "gemma-4" | "gemma4" => Some(Self::Gemma4),
            "mistral" => Some(Self::Mistral),
            "phi-3" | "phi-35" | "phi-3.5" => Some(Self::Phi3),
            "phi-4" => Some(Self::Phi4),
            "gpt-oss" | "gptoss" => Some(Self::GptOss),
            "zephyr" => Some(Self::Zephyr),
            _ => None,
        }
    }

    /// Returns every accepted alias, including the canonical name.
    #[must_use]
    pub const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Chatml => &["chatml"],
            Self::Llama3 => &["llama-3", "llama3"],
            Self::Llama31 => &[
                "llama-3.1",
                "llama-31",
                "llama-3.2",
                "llama-3.3",
                "llama-32",
                "llama-33",
            ],
            Self::Qwen25 => &["qwen-2.5", "qwen-25", "qwen25", "qwen2.5"],
            Self::Qwen3 => &["qwen-3", "qwen3"],
            Self::Gemma3 => &["gemma-3", "gemma3"],
            Self::Gemma4 => &["gemma-4", "gemma4"],
            Self::Mistral => &["mistral"],
            Self::Phi3 => &["phi-3", "phi-35", "phi-3.5"],
            Self::Phi4 => &["phi-4"],
            Self::GptOss => &["gpt-oss", "gptoss"],
            Self::Zephyr => &["zephyr"],
        }
    }

    /// Returns the family default system message when one is defined.
    ///
    /// `Some("")` is distinct from `None`: Llama 3.1 defines an empty default,
    /// while families returning `None` do not define one.
    #[must_use]
    pub const fn default_system_message(self) -> Option<&'static str> {
        match self {
            Self::Llama31 => Some(""),
            Self::Qwen25 => {
                Some("You are Qwen, created by Alibaba Cloud. You are a helpful assistant.")
            }
            Self::Chatml
            | Self::Llama3
            | Self::Qwen3
            | Self::Gemma3
            | Self::Gemma4
            | Self::Mistral
            | Self::Phi3
            | Self::Phi4
            | Self::GptOss
            | Self::Zephyr => None,
        }
    }

    /// Returns the stop-token spellings recorded for this family.
    ///
    /// `eos_token` names the tokenizer-provided value rather than a literal
    /// token string.
    #[must_use]
    pub const fn stop_tokens(self) -> &'static [&'static str] {
        match self {
            Self::Chatml | Self::Qwen3 | Self::Phi4 => &["<|im_end|>"],
            Self::Llama3 | Self::Llama31 | Self::Qwen25 | Self::Mistral | Self::Zephyr => {
                &["eos_token"]
            }
            Self::Gemma3 => &["<end_of_turn>"],
            Self::Gemma4 => &["<turn|>"],
            Self::Phi3 => &["<|end|>"],
            Self::GptOss => &["<|return|>"],
        }
    }

    /// Returns the exact bundled Jinja template.
    #[must_use]
    pub const fn template(self) -> &'static str {
        match self {
            Self::Chatml => include_str!("assets/chatml.jinja"),
            Self::Llama3 => include_str!("assets/llama-3.jinja"),
            Self::Llama31 => include_str!("assets/llama-3.1.jinja"),
            Self::Qwen25 => include_str!("assets/qwen-2.5.jinja"),
            Self::Qwen3 => include_str!("assets/qwen-3.jinja"),
            Self::Gemma3 => include_str!("assets/gemma-3.jinja"),
            Self::Gemma4 => include_str!("assets/gemma-4.jinja"),
            Self::Mistral => include_str!("assets/mistral.jinja"),
            Self::Phi3 => include_str!("assets/phi-3.jinja"),
            Self::Phi4 => include_str!("assets/phi-4.jinja"),
            Self::GptOss => include_str!("assets/gpt-oss.jinja"),
            Self::Zephyr => include_str!("assets/zephyr.jinja"),
        }
    }
}
