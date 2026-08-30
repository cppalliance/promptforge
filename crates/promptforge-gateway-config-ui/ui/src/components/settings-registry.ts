// The settings registry [Adapted: llama.cpp]: every model field is
// declared as data - key, label, help, section, control type, range,
// options, dependency - and one renderer walks the list. Adding a
// setting later is one object here, not new markup. The field lists
// mirror the plan's `[[model]]` and `[[local_model]]` specifications.

/** The control kinds the renderer knows how to build. */
export type SettingType = "slider" | "toggle" | "dropdown" | "input" | "textarea" | "chips";

/** What a setting definition can read to resolve options and visibility. */
export interface SettingContext {
  /** The current (edited-or-loaded) value of a sibling field. */
  value(key: string): unknown;
  /** The configured `[[dominion]]` entries. */
  dominions(): { id: string; kind: string }[];
  /** The configured `[[endpoint]]` ids. */
  endpointIds(): string[];
}

/** One setting declared as data; the renderer builds its control. */
export interface SettingDef {
  /** Dot path into the entry (`gpu_layers`, `speculative.draft_max`). */
  key: string;
  /** The visible label. */
  label: string;
  /** The muted help sentence under the control. */
  help: string;
  /** The section this setting renders in. */
  section: string;
  /** The control kind. */
  type: SettingType;
  /** The serde default, shown muted / as placeholder when unset. */
  default: number | boolean | string | string[] | null;
  /** Slider/number minimum. */
  min?: number;
  /** Slider/number maximum. */
  max?: number;
  /** Slider/number step. */
  step?: number;
  /** Log-scale slider positions (context-window style ranges). */
  logScale?: boolean;
  /** The value the slider's rightmost detent maps to (`Max`). */
  maxDetent?: number;
  /** Dropdown/chips options, fixed or resolved from the context. */
  options?: string[] | ((ctx: SettingContext) => string[]);
  /** Disabled unless the named sibling holds the given value. */
  dependsOn?: { key: string; value: unknown };
  /** Rendered only when this predicate passes (default: always). */
  visibleWhen?: (ctx: SettingContext) => boolean;
  /** Whether an `input` parses to a number (empty clears to null). */
  numeric?: boolean;
  /** Placeholder text for `input` controls. */
  placeholder?: string;
}

/** One collapsible section of the detail pane. */
export interface SectionDef {
  /** The id `SettingDef.section` references. */
  id: string;
  /** The section heading. */
  label: string;
  /**
   * Rendered only when this key holds a value; absent sections offer an
   * Add button that seeds `addValue` (the companion-table pattern).
   */
  presentKey?: string;
  /** The Add button's label for an absent conditional section. */
  addLabel?: string;
  /** The value the Add button seeds `presentKey` with. */
  addValue?: () => Record<string, unknown>;
}

/** Resolves a def's options against the context. */
export function settingOptions(def: SettingDef, ctx: SettingContext): string[] {
  if (!def.options) {
    return [];
  }
  return typeof def.options === "function" ? def.options(ctx) : def.options;
}

/** The capability block shared by local and remote models. */
function capabilities(): SettingDef[] {
  return [
    {
      key: "max_output",
      label: "Max output",
      help: "Max output tokens per completion. Must not exceed context.",
      section: "capabilities",
      type: "input",
      numeric: true,
      default: null,
      placeholder: "Unlimited",
    },
    {
      key: "default_temperature",
      label: "Default temperature",
      help: "Sampling temperature applied when the caller omits one.",
      section: "capabilities",
      type: "input",
      numeric: true,
      min: 0,
      max: 2,
      step: 0.1,
      default: null,
      placeholder: "Model default",
    },
    {
      key: "images",
      label: "Images",
      help: "Whether the model accepts image inputs.",
      section: "capabilities",
      type: "toggle",
      default: false,
    },
    {
      key: "parallel_tool_calls",
      label: "Parallel tool calls",
      help: "Whether the model can emit parallel tool calls.",
      section: "capabilities",
      type: "toggle",
      default: false,
    },
    {
      key: "effort_levels",
      label: "Effort levels",
      help: "Reasoning effort levels the model accepts.",
      section: "capabilities",
      type: "chips",
      default: [],
    },
    {
      key: "default_effort",
      label: "Default effort",
      help: "The effort level applied when the caller omits one.",
      section: "capabilities",
      type: "dropdown",
      default: null,
      options: (ctx) => {
        const levels = ctx.value("effort_levels");
        return Array.isArray(levels) ? levels.map(String) : [];
      },
      visibleWhen: (ctx) => {
        const levels = ctx.value("effort_levels");
        return Array.isArray(levels) && levels.length > 0;
      },
    },
    {
      key: "adaptive_thinking",
      label: "Adaptive thinking",
      help: "Whether the model adaptively chooses how much to think per request.",
      section: "capabilities",
      type: "toggle",
      default: false,
      visibleWhen: (ctx) => ctx.value("thinking") !== "never" && ctx.value("thinking") != null,
    },
  ];
}

/** The context slider shared by local and remote models. */
function contextSetting(): SettingDef {
  return {
    key: "context",
    label: "Context",
    help: "Context window size in tokens.",
    section: "generation",
    type: "slider",
    logScale: true,
    min: 512,
    max: 262144,
    step: 1,
    default: 4096,
  };
}

/** The thinking dropdown shared by local and remote models. */
function thinkingSetting(): SettingDef {
  return {
    key: "thinking",
    label: "Thinking",
    help: "Whether thinking tokens are never, always, or switchably available.",
    section: "generation",
    type: "dropdown",
    options: ["never", "always", "switchable"],
    default: "never",
  };
}

/** Sections of the local-model detail pane, in render order. */
export const LOCAL_MODEL_SECTIONS: readonly SectionDef[] = [
  { id: "gpu", label: "GPU & Memory" },
  { id: "generation", label: "Context & Generation" },
  { id: "source", label: "Source & Verification" },
  {
    id: "speculative",
    label: "Speculative Decoding",
    presentKey: "speculative",
    addLabel: "Add speculative decoding",
    addValue: () => ({ type: "draft-mtp", source: "", draft_max: 8 }),
  },
  {
    id: "projector",
    label: "Multimodal Projector",
    presentKey: "multimodal_projector",
    addLabel: "Add multimodal projector",
    addValue: () => ({ source: "" }),
  },
  { id: "capabilities", label: "Capabilities" },
];

/** Sections of the remote-model detail pane, in render order. */
export const REMOTE_MODEL_SECTIONS: readonly SectionDef[] = [
  { id: "routing", label: "Routing" },
  { id: "generation", label: "Context & Generation" },
  { id: "capabilities", label: "Capabilities" },
];

/** The `[[local_model]]` field registry. */
export const LOCAL_MODEL_SETTINGS: readonly SettingDef[] = [
  {
    key: "gpu_layers",
    label: "GPU layers",
    help: "GPU layers offloaded. Higher = faster, more VRAM.",
    section: "gpu",
    type: "slider",
    min: 0,
    max: 200,
    step: 1,
    maxDetent: 99999,
    default: 99,
  },
  {
    key: "vram_gb",
    label: "VRAM (GiB)",
    help: "VRAM footprint estimate for co-residency checks.",
    section: "gpu",
    type: "input",
    numeric: true,
    default: null,
    visibleWhen: (ctx) => {
      const bound = ctx.value("dominion");
      return (
        typeof bound === "string" &&
        ctx.dominions().some((dominion) => dominion.id === bound && dominion.kind === "local")
      );
    },
  },
  {
    key: "flash_attention",
    label: "Flash attention",
    help: "Reduces KV memory at long contexts. Required for quantized V cache.",
    section: "gpu",
    type: "toggle",
    default: true,
  },
  {
    key: "cache_type_k",
    label: "Cache type K",
    help: "KV cache quantization for K.",
    section: "gpu",
    type: "dropdown",
    options: ["f16", "q8_0", "q4_0"],
    default: "q8_0",
  },
  {
    key: "cache_type_v",
    label: "Cache type V",
    help: "KV cache quantization for V. Requires flash attention.",
    section: "gpu",
    type: "dropdown",
    options: ["f16", "q8_0", "q4_0"],
    default: "q4_0",
    dependsOn: { key: "flash_attention", value: true },
  },
  contextSetting(),
  {
    key: "n_predict",
    label: "Max prediction",
    help: "Generation ceiling per completion.",
    section: "generation",
    type: "slider",
    min: 256,
    max: 32768,
    step: 1,
    default: 8192,
  },
  {
    key: "parallel",
    label: "Parallel",
    help: "Max concurrent inferences (llama-server --parallel).",
    section: "generation",
    type: "slider",
    min: 1,
    max: 16,
    step: 1,
    default: 1,
  },
  thinkingSetting(),
  {
    key: "chat_template_file",
    label: "Chat template file",
    help: "Override the GGUF's embedded chat template.",
    section: "generation",
    type: "input",
    default: null,
    placeholder: "None (use the GGUF's template)",
  },
  {
    key: "source",
    label: "Source",
    help: "Where the GGUF was downloaded from (URL or local path).",
    section: "source",
    type: "input",
    default: "",
  },
  {
    key: "sha256",
    label: "SHA-256",
    help: "SHA-256 pin verified after download.",
    section: "source",
    type: "input",
    default: null,
    placeholder: "None (no pin)",
  },
  {
    key: "dominion",
    label: "Dominion",
    help: "Local compute pool this model binds to.",
    section: "source",
    type: "dropdown",
    default: null,
    options: (ctx) =>
      ctx
        .dominions()
        .filter((dominion) => dominion.kind === "local")
        .map((dominion) => dominion.id),
  },
  {
    key: "speculative.type",
    label: "Type",
    help: "Speculative decoding strategy.",
    section: "speculative",
    type: "dropdown",
    options: ["draft-mtp"],
    default: "draft-mtp",
  },
  {
    key: "speculative.source",
    label: "Drafter source",
    help: "Drafter GGUF source: URL or local path.",
    section: "speculative",
    type: "input",
    default: "",
  },
  {
    key: "speculative.sha256",
    label: "Drafter SHA-256",
    help: "SHA-256 pin for the drafter; required for remote sources.",
    section: "speculative",
    type: "input",
    default: null,
    placeholder: "None (no pin)",
  },
  {
    key: "speculative.draft_max",
    label: "Draft max",
    help: "Max speculative tokens per step.",
    section: "speculative",
    type: "slider",
    min: 1,
    max: 16,
    step: 1,
    default: 8,
  },
  {
    key: "multimodal_projector.source",
    label: "Projector source",
    help: "Projector GGUF source: URL or local path.",
    section: "projector",
    type: "input",
    default: "",
  },
  {
    key: "multimodal_projector.sha256",
    label: "Projector SHA-256",
    help: "SHA-256 pin for the projector; required for remote sources.",
    section: "projector",
    type: "input",
    default: null,
    placeholder: "None (no pin)",
  },
  ...capabilities(),
];

/** The `[[model]]` (remote) field registry. */
export const REMOTE_MODEL_SETTINGS: readonly SettingDef[] = [
  {
    key: "upstream",
    label: "Upstream",
    help: "The name the backend knows this model by.",
    section: "routing",
    type: "input",
    default: "",
  },
  {
    key: "endpoints",
    label: "Endpoints",
    help: "Which backends serve this model.",
    section: "routing",
    type: "chips",
    default: [],
    options: (ctx) => ctx.endpointIds(),
  },
  contextSetting(),
  thinkingSetting(),
  {
    key: "default_max_tokens",
    label: "Default max tokens",
    help: "Applied when the caller omits max_tokens.",
    section: "generation",
    type: "input",
    numeric: true,
    default: null,
    placeholder: "None (model decides)",
  },
  {
    key: "tool_dialect",
    label: "Tool dialect",
    help: "How tool calls are formatted on the wire.",
    section: "generation",
    type: "dropdown",
    options: ["openai", "gemma3_tool_code"],
    default: "openai",
    visibleWhen: (ctx) => ctx.value("kind") === "chat" || ctx.value("kind") == null,
  },
  ...capabilities(),
];
