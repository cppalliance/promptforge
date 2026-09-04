// The Discover view [Unsloth] Model Hub Discover tab: a master-detail
// split. The list side carries the debounced search bar (keywords,
// user/repo, pasted hub URLs), the locked GGUF filter chip, the sort
// dropdown, and the result rows (publisher avatar, name, params,
// downloads, likes, relative updated time). The detail side renders
// the model card (publisher, verified badge, tags as capability
// pills), the quant picker table with exact sizes and fit badges
// against /admin/system [Adapted: LM Studio], one starred Recommended
// row, and sanitized inline-HTML README rendering. Download actions
// stage model entries; Apply owns artifact provisioning.

import { BadgeCheck, Star, createElement as lucideElement } from "lucide";

import { createDropdownControl } from "../components/dropdown-control";
import { renderMarkdown, setSanitizedHtml } from "../components/markdown";
import type { ToastStack } from "shared-ui/toast";
import { formatBytes } from "../format";
import type { ConfigStore } from "../services/config-store";
import type { GatewayApi, SystemSnapshot } from "../services/gateway-api";
import { HfAuthError, UnauthorizedError } from "../services/gateway-api";
import type {
  DiscoverType,
  HfApi,
  HfModelDetail,
  HfQuant,
  HfSearchRow,
  HfSort,
} from "../services/hf-api";
import {
  DISCOVER_TYPES,
  avatarUrl,
  parseSearchInput,
  resolveUrl,
} from "../services/hf-api";

/** How long the search input waits before querying the hub. */
const SEARCH_DEBOUNCE_MS = 300;

/**
 * Fit badge states [Adapted: LM Studio], from the plan's heuristic
 * with a 1.2 weight margin on the file size:
 * `gpu` size*1.2 < free VRAM, `partial` size*1.2 < total VRAM,
 * `cpu` size*1.2 < free RAM, `none` otherwise.
 */
type Fit = "gpu" | "partial" | "cpu" | "none";

/** Display labels for the fit badges. */
const FIT_LABELS: Readonly<Record<Fit, string>> = {
  gpu: "Fits GPU",
  partial: "Partial offload",
  cpu: "CPU only",
  none: "Too large",
};

/** The sort dropdown's options, in order. */
const SORTS: ReadonlyArray<readonly [HfSort, string]> = [
  ["downloads", "Most downloads"],
  ["trending", "Trending"],
  ["newest", "Newest"],
];

/** Construction dependencies for the view. */
export interface DiscoverViewDeps {
  /** The admin API, for the system snapshot. */
  api: GatewayApi;
  /** The typed HF proxy client. */
  hf: HfApi;
  /** Pending-config store the Download buttons stage entries through. */
  store: ConfigStore;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view handle the router calls. */
export interface DiscoverView {
  /** Renders the view into `main`. */
  mount(main: HTMLElement): () => void;
}

/** Applies the plan's fit heuristic to one quant size. */
function fitOf(sizeBytes: number, system: SystemSnapshot): Fit {
  const need = sizeBytes * 1.2;
  if (system.gpu) {
    if (need < system.gpu.vram_total_bytes - system.gpu.vram_used_bytes) {
      return "gpu";
    }
    if (need < system.gpu.vram_total_bytes) {
      return "partial";
    }
  }
  if (need < system.ram.total_bytes - system.ram.used_bytes) {
    return "cpu";
  }
  return "none";
}

/**
 * The starred Recommended quant: the largest quant whose fit is `gpu`
 * (fully fits free VRAM), else the largest whose fit is `partial`;
 * no star when nothing reaches the GPU at all.
 */
function recommendedQuant(quants: HfQuant[], system: SystemSnapshot): string | null {
  let best: { quant: string; size: number; fit: Fit } | null = null;
  for (const quant of quants) {
    if (quant.sizeBytes === null) {
      continue;
    }
    const fit = fitOf(quant.sizeBytes, system);
    if (fit !== "gpu" && fit !== "partial") {
      continue;
    }
    if (
      best === null ||
      (fit === "gpu" && best.fit !== "gpu") ||
      (fit === best.fit && quant.sizeBytes > best.size)
    ) {
      best = { quant: quant.quant, size: quant.sizeBytes, fit };
    }
  }
  return best?.quant ?? null;
}

/** Builds the Discover view (state survives route re-mounts). */
export function createDiscoverView(deps: DiscoverViewDeps): DiscoverView {
  const { api, hf, store, toasts } = deps;

  let query = "";
  let sort: HfSort = "downloads";
  const activeTypes = new Set<DiscoverType>(["chat"]);
  let rows: HfSearchRow[] = [];
  let searchedOnce = false;
  let searching = false;
  let tokenMissing = false;
  let searchError: string | null = null;
  let selectedRepo: string | null = null;
  let detail: HfModelDetail | null = null;
  let detailLoading = false;
  let detailError: string | null = null;
  /** Sanitized README HTML, null while loading or when none exists. */
  let readmeHtml: string | null = null;
  let system: SystemSnapshot | null = null;
  const staging = new Set<string>();

  let main: HTMLElement | null = null;
  let viewRoot: HTMLElement | null = null;
  let listBox: HTMLElement | null = null;
  let detailBox: HTMLElement | null = null;
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchSeq = 0;
  let detailSeq = 0;
  let searchController: AbortController | null = null;
  let detailController: AbortController | null = null;
  let readmeController: AbortController | null = null;
  let systemController: AbortController | null = null;

  const runSearch = async (): Promise<void> => {
    searchController?.abort();
    const controller = new AbortController();
    searchController = controller;
    const seq = ++searchSeq;
    searching = true;
    renderList();
    const parsed = parseSearchInput(query);
    try {
      if (parsed.kind === "repo") {
        // A user/repo or pasted-URL form goes straight to the model
        // endpoint [Adapted: LM Studio] paste-a-URL.
        const model = await hf.model(parsed.repo, controller.signal);
        if (seq !== searchSeq) {
          return;
        }
        rows = [
          {
            repo: model.repo,
            owner: model.owner,
            name: model.name,
            downloads: model.downloads,
            likes: model.likes,
            updatedAt: model.updatedAt,
            params: model.params,
          },
        ];
        showDetail(model);
      } else {
        const found = await hf.search(parsed.query, sort, activeTypes, controller.signal);
        if (seq !== searchSeq) {
          return;
        }
        rows = found;
      }
      tokenMissing = false;
      searchError = null;
    } catch (error) {
      if (seq !== searchSeq) {
        return;
      }
      if (error instanceof DOMException && error.name === "AbortError") {
        return;
      }
      rows = [];
      if (error instanceof HfAuthError) {
        tokenMissing = true;
        searchError = null;
      } else if (error instanceof UnauthorizedError) {
        // The key prompt already took over the screen.
        return;
      } else {
        searchError = error instanceof Error ? error.message : String(error);
      }
    }
    searching = false;
    searchedOnce = true;
    render();
  };

  const selectRepo = (repo: string): void => {
    if (repo === selectedRepo && detail !== null) {
      return;
    }
    selectedRepo = repo;
    detail = null;
    detailError = null;
    readmeHtml = null;
    detailLoading = true;
    renderList();
    renderDetail();
    detailController?.abort();
    const controller = new AbortController();
    detailController = controller;
    const seq = ++detailSeq;
    void hf
      .model(repo, controller.signal)
      .then((model) => {
        if (seq !== detailSeq) {
          return;
        }
        showDetail(model);
        render();
      })
      .catch((error: unknown) => {
        if (seq !== detailSeq) {
          return;
        }
        if (error instanceof DOMException && error.name === "AbortError") {
          return;
        }
        detailLoading = false;
        if (error instanceof HfAuthError) {
          tokenMissing = true;
        } else if (!(error instanceof UnauthorizedError)) {
          detailError = error instanceof Error ? error.message : String(error);
        }
        render();
      });
  };

  /** Installs a loaded detail and starts its README fetch. */
  const showDetail = (model: HfModelDetail): void => {
    selectedRepo = model.repo;
    detail = model;
    detailLoading = false;
    detailError = null;
    readmeHtml = null;
    const seq = ++detailSeq;
    readmeController?.abort();
    const controller = new AbortController();
    readmeController = controller;
    void (async () => {
      try {
        const text = await hf.readme(model.repo, controller.signal);
        if (text === null || seq !== detailSeq) {
          return;
        }
        readmeHtml = renderMarkdown(text);
        renderDetail();
      } catch {
        // No README (private repo, offline hub): the section says so.
      }
    })();
  };

  const render = (): void => {
    if (!main || (viewRoot !== null && !viewRoot.isConnected)) {
      return;
    }
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Discover";

    const parts: HTMLElement[] = [title];
    if (tokenMissing) {
      parts.push(tokenBanner());
    }

    const split = document.createElement("div");
    split.className = "split discover-split";
    listBox = document.createElement("div");
    listBox.className = "split-list";
    detailBox = document.createElement("div");
    detailBox.className = "split-detail";
    split.append(listBox, detailBox);
    parts.push(split);

    renderList();
    renderDetail();
    viewRoot = split;
    main.replaceChildren(...parts);
  };

  // No-HF_TOKEN banner [Adapted: Open WebUI] missing-connection notice.
  const tokenBanner = (): HTMLElement => {
    const banner = document.createElement("div");
    banner.className = "banner banner-token";
    const text = document.createElement("span");
    text.textContent = "Set HF_TOKEN in Secrets to enable Hugging Face search.";
    const link = document.createElement("a");
    link.className = "button button-xs button-outline";
    link.href = "#/secrets";
    link.textContent = "Open Secrets";
    banner.append(text, link);
    return banner;
  };

  // ----- the list side ---------------------------------------------------

  const renderList = (): void => {
    if (!listBox) {
      return;
    }
    const parts: HTMLElement[] = [buildToolbar()];
    if (searching || !searchedOnce) {
      parts.push(skeletonList());
    } else if (searchError !== null) {
      parts.push(errorBanner(searchError));
    } else if (rows.length === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = tokenMissing
        ? "Hugging Face search is unavailable without a token."
        : "No models match the search.";
      parts.push(empty);
    } else {
      parts.push(resultList());
    }
    listBox.replaceChildren(...parts);
  };

  const buildToolbar = (): HTMLElement => {
    const toolbar = document.createElement("div");
    toolbar.className = "models-toolbar discover-toolbar";

    const searchLabel = document.createElement("label");
    searchLabel.className = "visually-hidden";
    searchLabel.htmlFor = "discover-search";
    searchLabel.textContent = "Search Hugging Face models";
    const searchInput = document.createElement("input");
    searchInput.type = "search";
    searchInput.id = "discover-search";
    searchInput.className = "input";
    searchInput.placeholder = "Search models, user/repo, or paste a URL";
    searchInput.value = query;
    searchInput.addEventListener("input", () => {
      if (searchTimer !== null) {
        clearTimeout(searchTimer);
      }
      searchTimer = setTimeout(() => {
        searchTimer = null;
        query = searchInput.value;
        void runSearch().then(() => {
          // The toolbar re-renders with the list; put focus back where
          // the user is typing.
          listBox?.querySelector<HTMLInputElement>("#discover-search")?.focus();
        });
      }, SEARCH_DEBOUNCE_MS);
      (searchTimer as unknown as { unref?: () => void }).unref?.();
    });

    // The GGUF chip is locked on: the gateway serves GGUF inference,
    // so the hub filter is pinned, shown pressed and disabled.
    const chips = document.createElement("div");
    chips.className = "filter-chips discover-types";
    chips.setAttribute("role", "group");
    chips.setAttribute("aria-label", "Filter models");
    const gguf = document.createElement("button");
    gguf.type = "button";
    gguf.className = "pill filter-chip gguf-chip";
    gguf.textContent = "GGUF";
    gguf.disabled = true;
    gguf.setAttribute("aria-pressed", "true");
    gguf.title = "Only GGUF repositories run on the gateway.";
    chips.append(gguf);
    const labels: Readonly<Record<DiscoverType, string>> = {
      chat: "Chat",
      embedding: "Embedding",
      reranker: "Reranker",
      stt: "STT",
      image: "Image",
      tts: "TTS",
    };
    for (const type of DISCOVER_TYPES) {
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "pill filter-chip discover-type";
      toggle.dataset["type"] = type;
      toggle.textContent = labels[type];
      toggle.setAttribute("aria-pressed", String(activeTypes.has(type)));
      toggle.addEventListener("click", () => {
        if (activeTypes.has(type)) {
          activeTypes.delete(type);
        } else {
          activeTypes.add(type);
        }
        void runSearch();
      });
      chips.append(toggle);
    }

    const sortLabel = document.createElement("label");
    sortLabel.className = "visually-hidden";
    sortLabel.htmlFor = "discover-sort";
    sortLabel.textContent = "Sort results";
    const sortSelect = createDropdownControl({
      id: "discover-sort",
      options: SORTS.map(([value, label]) => ({ value, label })),
      value: sort,
      onChange: (value) => {
        const selected = SORTS.find(([candidate]) => candidate === value)?.[0];
        if (selected !== undefined) {
          sort = selected;
          void runSearch();
        }
      },
    });
    sortSelect.trigger.classList.add("select-sm");

    toolbar.append(searchLabel, searchInput, chips, sortLabel, sortSelect.element);
    return toolbar;
  };

  const resultList = (): HTMLElement => {
    const list = document.createElement("ul");
    list.className = "result-list";
    for (const row of rows) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "result-row";
      button.setAttribute("aria-pressed", String(row.repo === selectedRepo));
      button.addEventListener("click", () => selectRepo(row.repo));

      const avatar = document.createElement("img");
      avatar.className = "result-avatar";
      avatar.src = avatarUrl(row.owner);
      avatar.alt = "";
      avatar.width = 32;
      avatar.height = 32;
      avatar.loading = "lazy";
      avatar.decoding = "async";

      const body = document.createElement("span");
      body.className = "result-main";
      const name = document.createElement("span");
      name.className = "model-name";
      name.textContent = row.repo;
      const stats = document.createElement("span");
      stats.className = "result-stats";
      if (row.params !== null) {
        const params = document.createElement("span");
        params.className = "pill result-params";
        params.textContent = row.params;
        stats.append(params);
      }
      stats.append(
        statSpan(`${compactCount(row.downloads)}`, "downloads"),
        statSpan(`${compactCount(row.likes)}`, "likes"),
        statSpan(relativeTime(row.updatedAt), "updated"),
      );
      body.append(name, stats);

      button.append(avatar, body);
      item.append(button);
      list.append(item);
    }
    return list;
  };

  const statSpan = (text: string, label: string): HTMLElement => {
    const stat = document.createElement("span");
    stat.className = "result-stat";
    stat.textContent = text;
    const hidden = document.createElement("span");
    hidden.className = "visually-hidden";
    hidden.textContent = ` ${label}`;
    stat.append(hidden);
    return stat;
  };

  const skeletonList = (): HTMLElement => {
    const list = document.createElement("ul");
    list.className = "result-list";
    list.setAttribute("aria-hidden", "true");
    for (let i = 0; i < 4; i += 1) {
      const row = document.createElement("li");
      row.className = "skeleton-row";
      list.append(row);
    }
    return list;
  };

  const errorBanner = (message: string): HTMLElement => {
    const banner = document.createElement("div");
    banner.className = "banner banner-danger";
    const text = document.createElement("span");
    text.textContent = `The search failed: ${message}`;
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "button button-xs button-outline";
    retry.textContent = "Retry";
    retry.addEventListener("click", () => void runSearch());
    banner.append(text, retry);
    return banner;
  };

  // ----- the detail side ---------------------------------------------------

  const renderDetail = (): void => {
    if (!detailBox) {
      return;
    }
    if (detailLoading) {
      detailBox.replaceChildren(skeletonList());
      return;
    }
    if (detailError !== null) {
      const failed = document.createElement("p");
      failed.className = "view-empty";
      failed.textContent = `Could not load the model: ${detailError}`;
      detailBox.replaceChildren(failed);
      return;
    }
    if (detail === null) {
      const hint = document.createElement("p");
      hint.className = "view-empty";
      hint.textContent = "Select a model to see its details.";
      detailBox.replaceChildren(hint);
      return;
    }
    detailBox.replaceChildren(detailCard(detail), readmeSection());
  };

  const detailCard = (model: HfModelDetail): HTMLElement => {
    const card = document.createElement("article");
    card.className = "hub-detail";

    const header = document.createElement("header");
    header.className = "hub-detail-header";
    const title = document.createElement("h2");
    title.className = "hub-detail-title";
    title.textContent = model.name;
    const publisher = document.createElement("p");
    publisher.className = "hub-publisher";
    const owner = document.createElement("span");
    owner.textContent = model.owner;
    publisher.append(owner);
    if (model.verified) {
      const badge = document.createElement("span");
      badge.className = "verified-badge";
      badge.append(
        lucideElement(BadgeCheck, { "aria-hidden": "true", width: 14, height: 14 }),
      );
      const hidden = document.createElement("span");
      hidden.className = "visually-hidden";
      hidden.textContent = "verified publisher";
      badge.append(hidden);
      publisher.append(badge);
    }

    const stats = document.createElement("p");
    stats.className = "result-stats hub-stats";
    if (model.params !== null) {
      const params = document.createElement("span");
      params.className = "pill result-params";
      params.textContent = model.params;
      stats.append(params);
    }
    stats.append(
      statSpan(compactCount(model.downloads), "downloads"),
      statSpan(compactCount(model.likes), "likes"),
      statSpan(relativeTime(model.updatedAt), "updated"),
    );

    header.append(title, publisher, stats);
    card.append(header);

    if (model.tags.length > 0) {
      const tags = document.createElement("div");
      tags.className = "pill-row";
      for (const tag of model.tags) {
        const pill = document.createElement("span");
        pill.className = "pill capability-pill";
        pill.textContent = tag;
        tags.append(pill);
      }
      card.append(tags);
    }

    card.append(quantTable(model));
    return card;
  };

  const quantTable = (model: HfModelDetail): HTMLElement => {
    const wrap = document.createElement("div");
    wrap.className = "quant-picker";
    if (model.quants.length === 0) {
      const none = document.createElement("p");
      none.className = "view-empty";
      none.textContent = "This repository has no GGUF files.";
      wrap.append(none);
      return wrap;
    }
    const recommended = system !== null ? recommendedQuant(model.quants, system) : null;

    const table = document.createElement("table");
    table.className = "quant-table";
    const caption = document.createElement("caption");
    caption.className = "visually-hidden";
    caption.textContent = "Available GGUF quantizations";
    table.append(caption);

    const thead = document.createElement("thead");
    const headRow = document.createElement("tr");
    for (const label of ["Quant", "Size", "Fit"]) {
      const th = document.createElement("th");
      th.scope = "col";
      th.textContent = label;
      headRow.append(th);
    }
    const actionsHead = document.createElement("th");
    actionsHead.scope = "col";
    const actionsLabel = document.createElement("span");
    actionsLabel.className = "visually-hidden";
    actionsLabel.textContent = "Actions";
    actionsHead.append(actionsLabel);
    headRow.append(actionsHead);
    thead.append(headRow);
    table.append(thead);

    const tbody = document.createElement("tbody");
    for (const quant of model.quants) {
      const row = document.createElement("tr");
      row.dataset["quant"] = quant.quant;

      const name = document.createElement("td");
      name.className = "quant-name";
      const nameText = document.createElement("span");
      nameText.className = "model-name";
      nameText.textContent = quant.quant;
      name.append(nameText);
      if (quant.quant === recommended) {
        row.classList.add("is-recommended");
        const star = document.createElement("span");
        star.className = "pill pill-accent recommended-pill";
        star.append(
          lucideElement(Star, { "aria-hidden": "true", width: 12, height: 12 }),
        );
        const text = document.createElement("span");
        text.textContent = "Recommended";
        star.append(text);
        name.append(star);
      }

      const size = document.createElement("td");
      size.className = "quant-size";
      if (quant.sizeBytes !== null) {
        size.textContent = formatBytes(quant.sizeBytes);
        size.title = `${quant.sizeBytes.toLocaleString()} bytes`;
      } else {
        size.textContent = "-";
      }

      const fitCell = document.createElement("td");
      if (quant.sizeBytes !== null && system !== null) {
        const fit = fitOf(quant.sizeBytes, system);
        const badge = document.createElement("span");
        badge.className = "pill fit-badge";
        badge.dataset["fit"] = fit;
        badge.textContent = FIT_LABELS[fit];
        fitCell.append(badge);
      } else {
        fitCell.textContent = "-";
      }

      const actions = document.createElement("td");
      actions.className = "quant-actions";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "button button-xs button-primary quant-download";
      const source =
        quant.files.length === 1 ? resolveUrl(model.repo, quant.files[0] ?? "") : null;
      const isAdded =
        source !== null &&
        store
          .models()
          .some((entry) => entry.kind !== "remote" && entry.data["source"] === source);
      const isStaging = source !== null && staging.has(source);
      button.textContent = isAdded ? "Added" : isStaging ? "Adding..." : "Download";
      button.disabled = source === null || isAdded || isStaging;
      if (source === null) {
        button.title = "Multi-part GGUF entries cannot be provisioned as one model.";
      }
      button.addEventListener("click", () => void stageDownload(model, quant));
      actions.append(button);

      row.append(name, size, fitCell, actions);
      tbody.append(row);
    }
    table.append(tbody);
    wrap.append(table);
    return wrap;
  };

  const stageDownload = async (model: HfModelDetail, quant: HfQuant): Promise<void> => {
    const file = quant.files.length === 1 ? quant.files[0] : undefined;
    if (file === undefined) {
      return;
    }
    const source = resolveUrl(model.repo, file);
    staging.add(source);
    renderDetail();
    const isStt =
      model.pipelineTag === "automatic-speech-recognition";
    const data: Record<string, unknown> = isStt
      ? {
          name: `${model.name.replace(/-gguf$/i, "")}-${quant.quant}`,
          role: "interim",
          source,
          vram_gb: 1,
          dominion: null,
        }
      : {
          name: `${model.name.replace(/-gguf$/i, "")}-${quant.quant}`,
          kind: "chat",
          description: `Hugging Face model ${model.repo}`,
          source,
          context: 4096,
        };
    if (quant.sha256 !== null) {
      data["sha256"] = quant.sha256;
    }
    if (quant.sizeBytes !== null) {
      data["vram_gb"] = Number((quant.sizeBytes / 1024 ** 3).toFixed(2));
    }
    if (!isStt) {
      const family = store.mappedChatTemplateFamily(model.repo);
      if (family !== null) {
        data["chat_template_file"] = `builtin:${family}`;
      }
    }
    try {
      const name = await store.stageDiscoveredModel(isStt ? "stt" : "local", data);
      toasts.show(`${name} added - Apply to download`, "success");
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The model could not be added", "error");
    } finally {
      staging.delete(source);
      renderDetail();
    }
  };

  const readmeSection = (): HTMLElement => {
    const section = document.createElement("section");
    section.className = "readme";
    section.setAttribute("aria-label", "README");
    const heading = document.createElement("h3");
    heading.className = "readme-heading";
    heading.textContent = "README";
    section.append(heading);
    if (readmeHtml === null) {
      const none = document.createElement("p");
      none.className = "view-empty";
      none.textContent = "No README available.";
      section.append(none);
      return section;
    }
    const body = document.createElement("div");
    body.className = "markdown";
    setSanitizedHtml(body, readmeHtml);
    section.append(body);
    return section;
  };

  return {
    mount(target: HTMLElement): () => void {
      main = target;
      viewRoot = null;
      render();
      if (system === null) {
        systemController?.abort();
        const controller = new AbortController();
        systemController = controller;
        void api
          .getSystem(controller.signal)
          .then((snapshot) => {
            system = snapshot;
            renderDetail();
          })
          .catch(() => {
            // No snapshot: fit badges render as "-" until one arrives.
          });
      }
      if (!searchedOnce && !searching) {
        // First visit browses the hub by the default sort, so the view
        // never opens empty (and a missing token surfaces immediately).
        void runSearch();
      }
      return () => {
        searchController?.abort();
        detailController?.abort();
        readmeController?.abort();
        systemController?.abort();
        searchController = null;
        detailController = null;
        readmeController = null;
        systemController = null;
        if (searchTimer !== null) {
          clearTimeout(searchTimer);
          searchTimer = null;
        }
        searchSeq += 1;
        detailSeq += 1;
        searching = false;
        detailLoading = false;
        main = null;
        viewRoot = null;
        listBox = null;
        detailBox = null;
      };
    },
  };
}

/** Compact count for stats: 1234567 -> "1.2M". */
function compactCount(count: number): string {
  if (count >= 1_000_000) {
    return `${trimmed(count / 1_000_000)}M`;
  }
  if (count >= 1_000) {
    return `${trimmed(count / 1_000)}K`;
  }
  return String(count);
}

/** One decimal under 10, none above. */
function trimmed(value: number): string {
  return value >= 10 ? String(Math.round(value)) : value.toFixed(1);
}

/** Relative time for the updated stat: "3d ago", "2mo ago". */
function relativeTime(iso: string | null): string {
  if (iso === null) {
    return "";
  }
  const then = Date.parse(iso);
  if (Number.isNaN(then)) {
    return "";
  }
  const days = Math.floor((Date.now() - then) / 86_400_000);
  if (days <= 0) {
    return "today";
  }
  if (days < 30) {
    return `${days}d ago`;
  }
  if (days < 365) {
    return `${Math.floor(days / 30)}mo ago`;
  }
  return `${Math.floor(days / 365)}y ago`;
}

