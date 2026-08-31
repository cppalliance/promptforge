// Typed layer over the gateway's Hugging Face proxy for the Discover
// view: maps the view's sort names onto the hub's API parameters, pins
// the GGUF filter, parses pasted hub URLs and user/repo forms, and
// shapes search results and model detail (GGUF siblings grouped into
// quants with exact summed sizes) for the quant picker.

import type { GatewayApi } from "./gateway-api";

/** The Discover sort orders, in dropdown order. */
export type HfSort = "downloads" | "trending" | "newest";

/** Discover workload toggles, in display order. */
export const DISCOVER_TYPES = [
  "chat",
  "embedding",
  "reranker",
  "stt",
  "image",
  "tts",
] as const;
export type DiscoverType = (typeof DISCOVER_TYPES)[number];

/** Hugging Face pipeline tags contributed by each workload toggle. */
export const PIPELINE_TAGS: Readonly<Record<DiscoverType, readonly string[]>> = {
  chat: ["text-generation"],
  embedding: ["feature-extraction", "sentence-similarity"],
  reranker: ["text-classification"],
  stt: ["automatic-speech-recognition"],
  image: ["text-to-image"],
  tts: ["text-to-speech"],
};

/** The hub `sort` parameter each Discover sort maps to. */
export const HF_SORT_PARAMS: Readonly<Record<HfSort, string>> = {
  downloads: "downloads",
  trending: "trendingScore",
  newest: "lastModified",
};

/** One search result row. */
export interface HfSearchRow {
  /** The `owner/name` repo id. */
  repo: string;
  /** The publisher (the repo's owner segment). */
  owner: string;
  /** The model name (the repo's name segment). */
  name: string;
  /** All-time download count. */
  downloads: number;
  /** Like count. */
  likes: number;
  /** ISO timestamp of the last modification, when the hub sent one. */
  updatedAt: string | null;
  /** Parameter-count tag parsed from the name ("8B"), when present. */
  params: string | null;
}

/** One GGUF quantization: its files (multi-part sums) and exact size. */
export interface HfQuant {
  /** The quant tag parsed from the filename (Q4_K_M, F16, ...). */
  quant: string;
  /** The sibling rfilenames making up this quant, in part order. */
  files: string[];
  /** Exact summed byte size, or null when the hub omitted a size. */
  sizeBytes: number | null;
  /** LFS SHA-256 for a single-file quant, when the hub supplied it. */
  sha256: string | null;
}

/** The model detail feeding the Discover detail card. */
export interface HfModelDetail {
  /** The `owner/name` repo id. */
  repo: string;
  /** The publisher. */
  owner: string;
  /** The model name. */
  name: string;
  /** All-time download count. */
  downloads: number;
  /** Like count. */
  likes: number;
  /** ISO timestamp of the last modification, when the hub sent one. */
  updatedAt: string | null;
  /** The hub's tag list. */
  tags: string[];
  /** The hub's primary workload classification. */
  pipelineTag: string | null;
  /** Whether the hub marked the publisher verified (when it says). */
  verified: boolean;
  /** Parameter-count tag parsed from the name, when present. */
  params: string | null;
  /** GGUF quantizations, smallest first. */
  quants: HfQuant[];
}

/** A parsed search-bar input: free text, or a direct repo target. */
export type SearchInput =
  | { readonly kind: "query"; readonly query: string }
  | { readonly kind: "repo"; readonly repo: string };

/** Hub path heads that are site sections, never repo owners. */
const HUB_SECTIONS = new Set([
  "models",
  "datasets",
  "spaces",
  "collections",
  "papers",
  "blog",
  "docs",
  "posts",
]);

/**
 * Classifies the search bar's text: a pasted hub URL or a bare
 * `user/repo` resolves to the repo it names; everything else is a
 * free-text query.
 */
export function parseSearchInput(raw: string): SearchInput {
  const trimmed = raw.trim();
  const url = /^https?:\/\/(?:www\.)?(?:huggingface\.co|hf\.co)\/([^?#]+)/i.exec(trimmed);
  if (url) {
    const segments = (url[1] ?? "").split("/").filter((segment) => segment !== "");
    const head = segments[0]?.toLowerCase() ?? "";
    if (segments.length >= 2 && !HUB_SECTIONS.has(head)) {
      return { kind: "repo", repo: `${segments[0]}/${segments[1]}` };
    }
    return { kind: "query", query: segments.join(" ") };
  }
  if (/^[A-Za-z0-9][A-Za-z0-9_.-]*\/[A-Za-z0-9_.-]+$/.test(trimmed)) {
    return { kind: "repo", repo: trimmed };
  }
  return { kind: "query", query: trimmed };
}

/** The hub's redirecting avatar endpoint for a user or organization. */
export function avatarUrl(owner: string): string {
  return `https://huggingface.co/api/avatars/${encodeURIComponent(owner)}`;
}

/** The hub download URL for one file of a repo. */
export function resolveUrl(repo: string, filename: string): string {
  const parsed = splitRepo(repo);
  if (!isRepoSegment(parsed.owner) || !isRepoSegment(parsed.name)) {
    throw new TypeError("the hub returned an invalid repository id");
  }
  const fileSegments = filename.split("/");
  if (
    fileSegments.some(
      (segment) => segment === "" || segment === "." || segment === "..",
    )
  ) {
    throw new TypeError("the hub returned an invalid model filename");
  }
  const encodedRepo = `${encodeURIComponent(parsed.owner)}/${encodeURIComponent(parsed.name)}`;
  const encodedFile = fileSegments.map(encodeURIComponent).join("/");
  return `https://huggingface.co/${encodedRepo}/resolve/main/${encodedFile}`;
}

/** The parameter-count tag in a model name ("8B", "0.6B", "8x7B"). */
export function paramsFromName(name: string): string | null {
  const match = /(?:^|[-_.])(\d+x\d+(?:\.\d+)?|\d+(?:\.\d+)?)([bm])(?=[-_.]|$)/i.exec(name);
  return match ? `${match[1]}${match[2]?.toUpperCase()}` : null;
}

/**
 * The quant tag of a GGUF filename, tolerating multi-part suffixes
 * (`-00001-of-00002`); null when the name carries no recognizable tag.
 */
function quantOf(filename: string): string | null {
  const base = filename
    .replace(/\.gguf$/i, "")
    .replace(/-\d{5}-of-\d{5}$/i, "");
  const match = /(?:^|[-._])(i?q\d+(?:_[a-z0-9]+)*|f16|f32|bf16)$/i.exec(base);
  return match?.[1]?.toUpperCase() ?? null;
}

/** Splits `owner/name`, tolerating a bare name. */
function splitRepo(repo: string): { owner: string; name: string } {
  const slash = repo.indexOf("/");
  if (slash < 0) {
    return { owner: "", name: repo };
  }
  return { owner: repo.slice(0, slash), name: repo.slice(slash + 1) };
}

/** Typed client for the gateway's HF proxy. */
export class HfApi {
  private readonly api: GatewayApi;

  constructor(api: GatewayApi) {
    this.api = api;
  }

  /**
   * Searches the hub for GGUF models. An empty `query` browses by the
   * sort order alone (the view's initial state).
   */
  async search(
    query: string,
    sort: HfSort,
    types: ReadonlySet<DiscoverType>,
    signal?: AbortSignal,
  ): Promise<HfSearchRow[]> {
    const common: Array<readonly [string, string]> = [
      ["filter", "gguf"],
      ["sort", HF_SORT_PARAMS[sort]],
      ["direction", "-1"],
      ["limit", "30"],
    ];
    const tags: string[] = [];
    for (const type of DISCOVER_TYPES) {
      if (types.has(type)) {
        for (const tag of PIPELINE_TAGS[type]) {
          tags.push(tag);
        }
      }
    }
    if (query !== "") {
      common.push(["q", query]);
    }
    // Hugging Face intersects repeated pipeline_tag parameters. One
    // request per tag plus a keyed merge provides the OR behavior the
    // workload toggles promise.
    const requests =
      tags.length === 0
        ? [this.api.hfSearch(common, signal)]
        : tags.map((tag) => this.api.hfSearch([...common, ["pipeline_tag", tag]], signal));
    const payloads = await Promise.all(requests);
    const byRepo = new Map<string, SearchCandidate>();
    for (const payload of payloads) {
      for (const candidate of searchCandidates(payload)) {
        const previous = byRepo.get(candidate.row.repo);
        if (previous === undefined || candidate.trendingScore > previous.trendingScore) {
          byRepo.set(candidate.row.repo, candidate);
        }
      }
    }
    return [...byRepo.values()]
      .sort((left, right) => compareCandidates(left, right, sort))
      .slice(0, 30)
      .map((candidate) => candidate.row);
  }

  /** Fetches one repo's detail and shapes its GGUF siblings into quants. */
  async model(repo: string, signal?: AbortSignal): Promise<HfModelDetail> {
    const raw = await this.api.hfModel(repo, signal);
    if (!isRecord(raw)) {
      throw new TypeError("the hub returned invalid model JSON");
    }
    const data = raw;
    const candidateId = typeof data["id"] === "string" ? data["id"] : repo;
    const id = isRepoId(candidateId) ? candidateId : repo;
    if (!isRepoId(id)) {
      throw new TypeError("the hub returned an invalid repository id");
    }
    const { owner, name } = splitRepo(id);
    const siblings = Array.isArray(data["siblings"]) ? data["siblings"] : [];
    const groups = new Map<string, HfQuant>();
    for (const sibling of siblings) {
      if (!isRecord(sibling)) {
        continue;
      }
      const filename = typeof sibling["rfilename"] === "string" ? sibling["rfilename"] : "";
      if (!/\.gguf$/i.test(filename) || !isSafeFilePath(filename)) {
        continue;
      }
      const quant = quantOf(filename);
      if (quant === null) {
        continue;
      }
      const size = typeof sibling["size"] === "number" ? sibling["size"] : null;
      const group = groups.get(quant) ?? {
        quant,
        files: [],
        sizeBytes: 0,
        sha256: null,
      };
      const lfs = sibling["lfs"];
      const rawDigest =
        isRecord(lfs) && typeof lfs["sha256"] === "string"
          ? lfs["sha256"]
          : null;
      const digest =
        rawDigest !== null && /^[0-9a-f]{64}$/i.test(rawDigest)
          ? rawDigest.toLowerCase()
          : null;
      group.sha256 = group.files.length === 0 ? digest : null;
      group.files.push(filename);
      // One missing part size makes the whole quant's size unknown.
      group.sizeBytes = group.sizeBytes === null || size === null ? null : group.sizeBytes + size;
      groups.set(quant, group);
    }
    const quants = [...groups.values()].sort(
      (a, b) => (a.sizeBytes ?? Number.MAX_SAFE_INTEGER) - (b.sizeBytes ?? Number.MAX_SAFE_INTEGER),
    );
    const authorData = data["authorData"];
    const verified = isRecord(authorData) && authorData["isVerified"] === true;
    return {
      repo: id,
      owner,
      name,
      downloads: typeof data["downloads"] === "number" ? data["downloads"] : 0,
      likes: typeof data["likes"] === "number" ? data["likes"] : 0,
      updatedAt: typeof data["lastModified"] === "string" ? data["lastModified"] : null,
      tags: stringArray(data["tags"]),
      pipelineTag: typeof data["pipeline_tag"] === "string" ? data["pipeline_tag"] : null,
      verified,
      params: paramsFromName(name),
      quants,
    };
  }
}

interface SearchCandidate {
  row: HfSearchRow;
  trendingScore: number;
}

function searchCandidates(data: unknown): SearchCandidate[] {
  if (!Array.isArray(data)) {
    throw new TypeError("the hub returned invalid search JSON");
  }
  const candidates: SearchCandidate[] = [];
  for (const item of data) {
    if (!isRecord(item)) {
      continue;
    }
    const repo = typeof item["id"] === "string" ? item["id"] : "";
    if (!isRepoId(repo)) {
      continue;
    }
    const { owner, name } = splitRepo(repo);
    candidates.push({
      row: {
        repo,
        owner,
        name,
        downloads: finiteNumber(item["downloads"]),
        likes: finiteNumber(item["likes"]),
        updatedAt:
          typeof item["lastModified"] === "string" ? item["lastModified"] : null,
        params: paramsFromName(name),
      },
      trendingScore: finiteNumber(item["trendingScore"]),
    });
  }
  return candidates;
}

function compareCandidates(left: SearchCandidate, right: SearchCandidate, sort: HfSort): number {
  let order = 0;
  if (sort === "downloads") {
    order = right.row.downloads - left.row.downloads;
  } else if (sort === "trending") {
    order = right.trendingScore - left.trendingScore;
  } else {
    order = timestamp(right.row.updatedAt) - timestamp(left.row.updatedAt);
  }
  return order || left.row.repo.localeCompare(right.row.repo);
}

function timestamp(value: string | null): number {
  if (value === null) {
    return 0;
  }
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

function finiteNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string")
    : [];
}

function isRepoId(repo: string): boolean {
  const { owner, name } = splitRepo(repo);
  return isRepoSegment(owner) && isRepoSegment(name) && repo === `${owner}/${name}`;
}

function isRepoSegment(segment: string): boolean {
  return (
    segment !== "" &&
    !/^\.+$/.test(segment) &&
    /^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(segment)
  );
}

function isSafeFilePath(path: string): boolean {
  return path
    .split("/")
    .every((segment) => segment !== "" && segment !== "." && segment !== "..");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
