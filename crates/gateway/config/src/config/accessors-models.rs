//! Read accessors for the remote `[[model]]` and local `[[local_model]]` entries.

use super::super::{
    Capabilities, LocalModelConfig, ModelConfig, ModelKind, ThinkingMode, ToolDialect,
};

impl ModelConfig {
    /// Returns the name callers request and that a slot resolves to.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the workload this model serves: chat (the default),
    /// embedding, classifier, or speech.
    #[must_use]
    pub fn kind(&self) -> ModelKind {
        self.kind
    }

    /// Returns the prose describing the model for catalog consumers and
    /// semantic bind.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the context window size in tokens.
    #[must_use]
    pub fn context(&self) -> u32 {
        self.context
    }

    /// Returns whether thinking tokens are never, always, or switchably
    /// available.
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the string the backend knows this model by.
    #[must_use]
    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// Returns the endpoint ids serving this model (v0 uses the first).
    #[must_use]
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Returns the `max_tokens` default supplied when the caller omits one.
    #[must_use]
    pub fn default_max_tokens(&self) -> Option<u32> {
        self.default_max_tokens
    }

    /// Returns the tool-calling dialect this model speaks: `openai` (the
    /// default) for native wire tool calls, or `gemma3_tool_code` for
    /// emulated content-fence tool calling.
    #[must_use]
    pub fn tool_dialect(&self) -> ToolDialect {
        self.tool_dialect
    }

    /// Returns the capability metadata advertised on the catalog.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}
impl LocalModelConfig {
    /// Returns the caller-facing model name in `/v1/models` and chat
    /// completions.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the workload this model serves: chat (the default),
    /// embedding, classifier, or speech.
    #[must_use]
    pub fn kind(&self) -> ModelKind {
        self.kind
    }

    /// Returns the prose describing the model for catalog consumers and
    /// semantic bind.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the model source: an https URL or a local filesystem path to
    /// a GGUF.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the SHA-256 pin (lowercase hex) verified after download, when
    /// set.
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Returns the local dominion id (`[[dominion]]`) binding this model,
    /// when set.
    #[must_use]
    pub fn dominion(&self) -> Option<&str> {
        self.dominion.as_deref()
    }

    /// Returns the max concurrent inferences: the child's `--parallel` value
    /// and, when no dominion is bound, the model's gateway queue limit.
    /// Defaults to 1.
    #[must_use]
    pub fn parallel(&self) -> u32 {
        self.parallel
    }

    /// Returns the VRAM footprint estimate in gibibytes for the dominion
    /// co-residency check, when set.
    #[must_use]
    pub fn vram_gb(&self) -> Option<f64> {
        self.vram_gb
    }

    /// Returns the context window size in tokens (`--ctx-size`).
    #[must_use]
    pub fn context(&self) -> u32 {
        self.context
    }

    /// Returns whether thinking tokens are never, always, or switchably
    /// available.
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the GPU layers offloaded (`-ngl`).
    #[must_use]
    pub fn gpu_layers(&self) -> u32 {
        self.gpu_layers
    }

    /// Returns whether flash attention is enabled (`--flash-attn on`).
    #[must_use]
    pub fn flash_attention(&self) -> bool {
        self.flash_attention
    }

    /// Returns the KV cache type for K.
    #[must_use]
    pub fn cache_type_k(&self) -> &str {
        &self.cache_type_k
    }

    /// Returns the KV cache type for V.
    #[must_use]
    pub fn cache_type_v(&self) -> &str {
        &self.cache_type_v
    }

    /// Returns the generation ceiling (`--n-predict`).
    #[must_use]
    pub fn n_predict(&self) -> u32 {
        self.n_predict
    }

    /// Returns the path to a Jinja chat template file
    /// (`--chat-template-file`), when set.
    #[must_use]
    pub fn chat_template_file(&self) -> Option<&str> {
        self.chat_template_file.as_deref()
    }

    /// Returns the capability metadata advertised on the catalog.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}
