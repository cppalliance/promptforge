// Validated HTTP boundary for the prompt-contract API. POST
// /prompts/contract turns prompt markdown into the Run window's
// contract DTO; every response arrives as unknown and is parsed field
// by field into the narrow types below before any consumer touches it,
// no casts. Failures throw typed CatalogError variants
// (services/error-catalog.ts); a 422 parse failure surfaces the
// server's parse_<kind> code and line-numbered message in the thrown
// error's message. The fetch and JSON mechanics are the shared floor
// in json-request.ts.

import { CatalogError, ErrorCatalog } from "./error-catalog";
import { errorCode, errorMessage, isRecord, readJson, request } from "./json-request";

/** A declared input or output file. */
export interface RunContractFile {
  readonly path: string;
  readonly description: string;
}

/** A declared capability: its global id and optionality. */
export interface RunContractCapability {
  readonly id: string;
  readonly optional: boolean;
}

/** One tool slot: its alias and the canonical namespace/pack/name path. */
export interface RunContractTool {
  readonly kind: "exact";
  readonly alias: string;
  readonly path: string;
}

/** The declared arg types the wire format carries. */
export type RunContractArgType = "string" | "boolean" | "integer" | "number";

/** One declared arg. */
export interface RunContractArg {
  readonly name: string;
  readonly type: RunContractArgType;
  readonly optional: boolean;
  /** The declared default; null when absent. */
  readonly default: string | number | boolean | null;
  /** The human-readable description; null when absent. */
  readonly description: string | null;
}

/** The typed args declaration; implicit carries the single prose field. */
export interface RunContractArgs {
  readonly implicit: boolean;
  readonly fields: readonly RunContractArg[];
}

/** One declared model role. */
export interface RunContractModel {
  readonly label: string;
  readonly keywords: readonly string[];
  /** The minimum context window in tokens; null when absent. */
  readonly minContext: number | null;
  readonly description: string | null;
}

/** The Run-window contract: everything the panel renders. */
export interface RunContract {
  readonly name: string;
  readonly description: string;
  /** The declared promptforge engine major; null when absent. */
  readonly promptforge: number | null;
  /** The declared tool-loop cap; null for the runtime default. */
  readonly maxToolIterations: number | null;
  readonly input: RunContractFile | null;
  readonly output: RunContractFile | null;
  readonly capabilities: readonly RunContractCapability[];
  readonly tools: readonly RunContractTool[];
  readonly args: RunContractArgs;
  readonly models: readonly RunContractModel[];
}

function parseFile(value: unknown): RunContractFile | null {
  if (!isRecord(value)) {
    return null;
  }
  const { path, description } = value;
  if (typeof path !== "string" || typeof description !== "string") {
    return null;
  }
  return { path, description };
}

function parseCapability(value: unknown): RunContractCapability | null {
  if (!isRecord(value)) {
    return null;
  }
  const { id, optional } = value;
  if (typeof id !== "string" || typeof optional !== "boolean") {
    return null;
  }
  return { id, optional };
}

function parseTool(value: unknown): RunContractTool | null {
  if (!isRecord(value)) {
    return null;
  }
  const { kind, alias, path } = value;
  if (kind !== "exact" || typeof alias !== "string" || typeof path !== "string") {
    return null;
  }
  return { kind, alias, path };
}

const ARG_TYPES: readonly string[] = ["string", "boolean", "integer", "number"];

function parseArg(value: unknown): RunContractArg | null {
  if (!isRecord(value)) {
    return null;
  }
  const { name, type, optional } = value;
  if (typeof name !== "string" || typeof type !== "string" || !ARG_TYPES.includes(type)) {
    return null;
  }
  if (typeof optional !== "boolean") {
    return null;
  }
  const defaultValue = value.default;
  if (
    defaultValue !== null &&
    typeof defaultValue !== "string" &&
    typeof defaultValue !== "number" &&
    typeof defaultValue !== "boolean"
  ) {
    return null;
  }
  const description = value.description;
  if (description !== null && typeof description !== "string") {
    return null;
  }
  return {
    name,
    type: type as RunContractArgType,
    optional,
    default: defaultValue,
    description: description ?? null,
  };
}

function parseArgs(value: unknown): RunContractArgs | null {
  if (!isRecord(value)) {
    return null;
  }
  const { implicit, fields } = value;
  if (typeof implicit !== "boolean" || !Array.isArray(fields)) {
    return null;
  }
  const parsed: RunContractArg[] = [];
  for (const field of fields) {
    const arg = parseArg(field);
    if (arg === null) {
      return null;
    }
    parsed.push(arg);
  }
  return { implicit, fields: parsed };
}

function parseModel(value: unknown): RunContractModel | null {
  if (!isRecord(value)) {
    return null;
  }
  const { label, keywords, min_context } = value;
  if (typeof label !== "string") {
    return null;
  }
  if (!Array.isArray(keywords) || !keywords.every((keyword) => typeof keyword === "string")) {
    return null;
  }
  if (min_context !== null && typeof min_context !== "number") {
    return null;
  }
  const description = value.description;
  if (description !== null && typeof description !== "string") {
    return null;
  }
  return {
    label,
    keywords,
    minContext: min_context ?? null,
    description: description ?? null,
  };
}

function parseContract(body: unknown): RunContract | null {
  if (!isRecord(body)) {
    return null;
  }
  const { name, description, promptforge, max_tool_iterations, capabilities, tools, models } = body;
  if (typeof name !== "string" || typeof description !== "string") {
    return null;
  }
  if (promptforge !== null && typeof promptforge !== "number") {
    return null;
  }
  if (max_tool_iterations !== null && typeof max_tool_iterations !== "number") {
    return null;
  }
  const input = body.input === null ? null : parseFile(body.input);
  const output = body.output === null ? null : parseFile(body.output);
  if (body.input !== null && input === null) {
    return null;
  }
  if (body.output !== null && output === null) {
    return null;
  }
  if (!Array.isArray(capabilities) || !Array.isArray(tools) || !Array.isArray(models)) {
    return null;
  }
  const parsedCapabilities: RunContractCapability[] = [];
  for (const capability of capabilities) {
    const parsed = parseCapability(capability);
    if (parsed === null) {
      return null;
    }
    parsedCapabilities.push(parsed);
  }
  const parsedTools: RunContractTool[] = [];
  for (const tool of tools) {
    const parsed = parseTool(tool);
    if (parsed === null) {
      return null;
    }
    parsedTools.push(parsed);
  }
  const args = parseArgs(body.args);
  if (args === null) {
    return null;
  }
  const parsedModels: RunContractModel[] = [];
  for (const model of models) {
    const parsed = parseModel(model);
    if (parsed === null) {
      return null;
    }
    parsedModels.push(parsed);
  }
  return {
    name,
    description,
    promptforge: promptforge ?? null,
    maxToolIterations: max_tool_iterations ?? null,
    input,
    output,
    capabilities: parsedCapabilities,
    tools: parsedTools,
    args,
    models: parsedModels,
  };
}

/**
 * Throws the typed failure for one non-OK response. A 422 parse failure
 * carries the server's parse_<kind> code ahead of its line-numbered
 * message, so the panel's error row shows both.
 */
function httpFailure(body: unknown, status: number, route: string): never {
  const message = errorMessage(body, status, route);
  const code = errorCode(body);
  throw new CatalogError(ErrorCatalog.HttpStatus, code === null ? message : `[${code}] ${message}`, {
    status,
  });
}

/**
 * Parses prompt markdown into the Run window's contract. `name` is the
 * prompt's display name (the file's, when it came from one). Throws
 * typed CatalogError variants on transport, HTTP, and shape failures.
 */
export async function fetchPromptContract(name: string, text: string): Promise<RunContract> {
  const route = "/prompts/contract";
  const response = await request(route, `POST ${route}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, text }),
  });
  const body = await readJson(response, `POST ${route}`);
  if (!response.ok) {
    httpFailure(body, response.status, `POST ${route}`);
  }
  const contract = parseContract(body);
  if (contract === null) {
    throw new CatalogError(
      ErrorCatalog.UnexpectedShape,
      `POST ${route} returned an unexpected shape`,
    );
  }
  return contract;
}
