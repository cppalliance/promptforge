// The add-model merge, pure over the loaded config document: appends
// the provider's [[endpoint]] when none matches the provider name
// (reusing the existing one otherwise) and appends the [[model]] entry
// with the sheet's capability fields mirrored under the config's own
// field names. The merged document goes through PUT /admin/config and
// the existing apply flow.

import type { EntryData } from "./config-store";
import type { CloudModelEntry, CloudProviderSlice } from "./gateway-api";

/** The operator-confirmed details the sheet cannot supply. */
export interface MergeDetails {
  /** The catalog name; defaults to the entry's display name. */
  name: string;
  /** Context window; required when the sheet entry lacks one. */
  context?: number;
  /** The description; defaults to the entry's display name. */
  description?: string;
}

/** The config's thinking mode for the sheet's capability triple. */
function thinkingMode(thinking: CloudModelEntry["thinking"]): string {
  if (!thinking.supported) {
    return "never";
  }
  // Manual budget mode is a per-call switch; a bare reasoning flag
  // means the backend reasons without one.
  return thinking.enabled ? "switchable" : "always";
}

/**
 * Merges one sheet entry into `config` (mutated and returned): the
 * provider's endpoint first - `id` the provider name, `protocol`
 * `openai`, `base_url` the slice's OpenAI-compatible base, `api_key`
 * the `${VAR}` indirection of the slice's key-role variable, omitted
 * for keyless providers - then the model entry. Throws when the
 * provider has no OpenAI-compatible endpoint, or when the sheet lacks
 * a context window and the operator supplied none (the
 * `ModelConfig.context` nullability gap).
 */
export function mergeCloudModel(
  config: EntryData,
  provider: string,
  slice: CloudProviderSlice,
  entry: CloudModelEntry,
  details: MergeDetails,
): EntryData {
  if (slice.openai_base_url === null) {
    throw new Error(`${slice.display_name} has no OpenAI-compatible endpoint`);
  }
  const context = details.context ?? entry.context_window;
  if (context === null || context === undefined) {
    throw new Error(
      `context is required: the sheet reports no context window for ${entry.id}`,
    );
  }

  const endpoints = arrayOf(config, "endpoint");
  if (!endpoints.some((item) => item["id"] === provider)) {
    const keyVar = slice.env_vars.find((variable) => variable.role === "key");
    endpoints.push({
      id: provider,
      protocol: "openai",
      base_url: slice.openai_base_url,
      ...(keyVar ? { api_key: `\${${keyVar.name}}` } : {}),
    });
  }

  const model: EntryData = {
    name: details.name,
    kind: entry.kind,
    description: details.description ?? entry.display_name,
    context,
    thinking: thinkingMode(entry.thinking),
    upstream: entry.id,
    endpoints: [provider],
    images: entry.images,
    adaptive_thinking: entry.thinking.adaptive,
    effort_levels: entry.effort_levels,
  };
  if (entry.max_output !== null) {
    model["max_output"] = entry.max_output;
  }
  if (entry.default_effort !== null) {
    model["default_effort"] = entry.default_effort;
  }
  arrayOf(config, "model").push(model);
  return config;
}

/** One keyed array of the config document, created when absent. */
function arrayOf(config: EntryData, key: string): EntryData[] {
  const value = config[key];
  if (Array.isArray(value)) {
    return value as EntryData[];
  }
  const created: EntryData[] = [];
  config[key] = created;
  return created;
}
