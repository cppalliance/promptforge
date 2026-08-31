// Typed layer over the gateway's Hugging Face proxy for the Discover
// view: maps the view's sort names onto the hub's API parameters, pins
// the GGUF filter, parses pasted hub URLs and user/repo forms, and
// shapes search results and model detail (GGUF siblings grouped into
// quants with exact summed sizes) for the quant picker.

import type { GatewayApi } from "./gateway-api";

/** The Discover sort orders, in dropdown order. */
export type HfSort = "downloads" | "trending" | "newest";

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
  return `https://huggingface.co/${repo}/resolve/main/${encodeURI(filename)}`;
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
  async search(query: string, sort: HfSort): Promise<HfSearchRow[]> {
    const params: Record<string, string> = {
      filter: "gguf",
      sort: HF_SORT_PARAMS[sort],
      direction: "-1",
      limit: "30",
    };
    if (query !== "") {
      params["q"] = query;
    }
    const data = await this.api.hfSearch(params);
    if (!Array.isArray(data)) {
      return [];
    }
    const rows: HfSearchRow[] = [];
    for (const item of data) {
      if (item === null || typeof item !== "object") {
        continue;
      }
      const record = item as Record<string, unknown>;
      const repo = typeof record["id"] === "string" ? record["id"] : "";
      if (repo === "") {
        continue;
      }
      const { owner, name } = splitRepo(repo);
      rows.push({
        repo,
        owner,
        name,
        downloads: typeof record["downloads"] === "number" ? record["downloads"] : 0,
        likes: typeof record["likes"] === "number" ? record["likes"] : 0,
        updatedAt:
          typeof record["lastModified"] === "string" ? record["lastModified"] : null,
        params: paramsFromName(name),
      });
    }
    return rows;
  }

  /** Fetches one repo's detail and shapes its GGUF siblings into quants. */
  async model(repo: string): Promise<HfModelDetail> {
    const data = (await this.api.hfModel(repo)) as Record<string, unknown>;
    const id = typeof data["id"] === "string" ? data["id"] : repo;
    const { owner, name } = splitRepo(id);
    const siblings = Array.isArray(data["siblings"]) ? data["siblings"] : [];
    const groups = new Map<string, HfQuant>();
    for (const sibling of siblings) {
      if (sibling === null || typeof sibling !== "object") {
        continue;
      }
      const record = sibling as Record<string, unknown>;
      const filename = typeof record["rfilename"] === "string" ? record["rfilename"] : "";
      if (!/\.gguf$/i.test(filename)) {
        continue;
      }
      const quant = quantOf(filename);
      if (quant === null) {
        continue;
      }
      const size = typeof record["size"] === "number" ? record["size"] : null;
      const group = groups.get(quant) ?? {
        quant,
        files: [],
        sizeBytes: 0,
        sha256: null,
      };
      const lfs = record["lfs"];
      const rawDigest =
        lfs !== null &&
        typeof lfs === "object" &&
        typeof (lfs as Record<string, unknown>)["sha256"] === "string"
          ? String((lfs as Record<string, unknown>)["sha256"])
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
    const verified =
      authorData !== null &&
      typeof authorData === "object" &&
      (authorData as Record<string, unknown>)["isVerified"] === true;
    return {
      repo: id,
      owner,
      name,
      downloads: typeof data["downloads"] === "number" ? data["downloads"] : 0,
      likes: typeof data["likes"] === "number" ? data["likes"] : 0,
      updatedAt: typeof data["lastModified"] === "string" ? data["lastModified"] : null,
      tags: Array.isArray(data["tags"]) ? data["tags"].map(String) : [],
      verified,
      params: paramsFromName(name),
      quants,
    };
  }
}
