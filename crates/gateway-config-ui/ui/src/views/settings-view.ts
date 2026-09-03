// The Settings view [Unsloth] settings category list: a secondary nav
// column routing #/settings/{section} across System / Gateway / Workshop
// / Dominions / Endpoints / Tools / About. System renders the live
// metric-tile grid and GPU devices from GET /admin/system, polled every
// 5s only while that panel is mounted; the Storage card lives there too,
// beside the disk data it cites. Gateway and Workshop edit the
// global sections and save through PUT /admin/config. Every field
// reuses the established grammar: dirty dot with per-field reset and a
// pending chip against the running value.

import {
  Cpu,
  Eye,
  EyeOff,
  HardDrive,
  MemoryStick,
  Microchip,
  RotateCcw,
  createElement as lucideElement,
} from "lucide";

import { confirmDialog } from "../components/confirm-modal";
import { createChipInput } from "../components/chip-input";
import { createDropdownControl } from "../components/dropdown-control";
import { createSliderControl } from "../components/slider-control";
import { createToggleControl } from "../components/toggle-control";
import type { ToastStack } from "../components/toast";
import { formatBytes } from "../format";
import { readPath, writePath } from "../services/config-store";
import type { ConfigStore, EntryData } from "../services/config-store";
import type { GatewayApi, SystemSnapshot } from "../services/gateway-api";

// The crate version, substituted by the esbuild define in build.mjs; a
// bundle built without the define (build.rs's direct debug bundle) shows
// the "dev" fallback instead of breaking on a free identifier.
declare const __APP_VERSION__: string | undefined;
const APP_VERSION = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";

/** How often the System panel refreshes GET /admin/system while mounted. */
const SYSTEM_POLL_MS = 5000;

/** The Brave Search default base URL, shown as the placeholder. */
const BRAVE_BASE_URL = "https://api.search.brave.com/res/v1";

/** The seven nav sections, in column order. */
const SECTIONS = [
  { id: "system", label: "System" },
  { id: "gateway", label: "Gateway" },
  { id: "workshop", label: "Workshop" },
  { id: "dominions", label: "Dominions" },
  { id: "endpoints", label: "Endpoints" },
  { id: "tools", label: "Tools" },
  { id: "about", label: "About" },
] as const;
type SectionId = (typeof SECTIONS)[number]["id"];

/** GPU vendor chip colors, matched against the reported device name. */
const VENDORS: ReadonlyArray<readonly [pattern: RegExp, label: string, color: string]> = [
  [/nvidia|geforce|quadro/i, "NVIDIA", "#76B900"],
  [/\bamd\b|radeon/i, "AMD", "#ED1C24"],
  [/intel|\barc\b/i, "Intel", "#0068B5"],
];

/** Construction dependencies for the view. */
export interface SettingsViewDeps {
  /** The config store: pending view, payload builders, save paths. */
  store: ConfigStore;
  /** The admin API, for the system poll. */
  api: GatewayApi;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view handle the router calls. */
export interface SettingsView {
  /** Renders the view into `main`, opening `section` (default system). */
  mount(main: HTMLElement, section?: string): () => void;
}

/** One editable card: a section table, a keyed-array entry, or a draft. */
interface Card {
  /** The edit-map key (`server`, `dominion:gpu0`, `tools`). */
  key: string;
  /** The baseline data: the pending view's section, or the draft object. */
  base: EntryData;
  /** True for a browser-created object that has never been saved. */
  draft: boolean;
  /** Field keys whose pending value differs from the running value. */
  pendingFields: ReadonlySet<string>;
  /** The pending-view prefix for running-value tooltips, when applicable. */
  runningPrefix?: string;
}

/** One field's declaration; the renderer builds the control. */
interface FieldSpec {
  /** Dot path within the card's data. */
  path: string;
  /** The visible label. */
  label: string;
  /** The muted help sentence under the control. */
  help: string;
  /** The control kind. */
  type: "input" | "toggle" | "dropdown" | "slider" | "chips" | "secret";
  /** Whether an `input` parses to a number (empty clears to null). */
  numeric?: boolean;
  /** Placeholder text for `input` controls. */
  placeholder?: string;
  /** Dropdown options. */
  options?: string[];
  /** Whether a "None" option maps to null (dropdowns). */
  allowNone?: boolean;
  /** Slider bounds. */
  min?: number;
  max?: number;
  step?: number;
  /** The value shown when the field is unset. */
  fallback?: number | boolean | string | string[] | null;
  /** Renders the control disabled (locked single-option dropdowns). */
  locked?: boolean;
}

/** Bytes rendered as GiB with one decimal. */
function gib(bytes: number): string {
  return (bytes / 1024 ** 3).toFixed(1);
}

/** Structural equality through JSON, matching the store's notion. */
function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

/** The vendor chip data for a GPU name, or null for an unmatched vendor. */
function vendorOf(name: string): { label: string; color: string } | null {
  for (const [pattern, label, color] of VENDORS) {
    if (pattern.test(name)) {
      return { label, color };
    }
  }
  return null;
}

/** The config UI URL derived from the gateway bind (unspecified -> loopback). */
function configUiUrl(bind: string): string {
  const colon = bind.lastIndexOf(":");
  if (colon < 0) {
    return `http://${bind}/config/`;
  }
  let host = bind.slice(0, colon);
  const port = bind.slice(colon + 1);
  if (host === "0.0.0.0") {
    host = "127.0.0.1";
  } else if (host === "[::]") {
    host = "[::1]";
  }
  return `http://${host}:${port}/config/`;
}

/** The `[workshop]` defaults the Enable button seeds. */
function workshopDefaults(): EntryData {
  return { bind: "127.0.0.1:7910", open_browser: false, stt: null };
}

/** The `[workshop.stt]` capture defaults, mirroring the config crate. */
function sttDefaults(): EntryData {
  return {
    window_seconds: 15,
    interval_ms: 500,
    vocabulary: [],
  };
}

/** Digest-pinned catalog entries supplied by the config crate. */
const RECOMMENDED_STT_MODELS: readonly EntryData[] = [
  {
    name: "whisper-base-en",
    role: "interim",
    source: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
    sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
    vram_gb: 1,
  },
  {
    name: "whisper-small-en",
    role: "final",
    source: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
    sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
    vram_gb: 2,
  },
];

/** The `[tools.web_search]` seed the Enable button creates. */
function webSearchDefaults(): EntryData {
  return { provider: "brave", api_key: "" };
}

/** Builds the Settings view (state survives route re-mounts). */
export function createSettingsView(deps: SettingsViewDeps): SettingsView {
  const { store, api, toasts } = deps;

  let main: HTMLElement | null = null;
  /** The last-rendered panel root; a re-render is legal only while it owns `main`. */
  let viewRoot: HTMLElement | null = null;
  let section: SectionId = "system";
  /** Unsaved edits: card key -> field path -> value. */
  const edits = new Map<string, Map<string, unknown>>();
  /** Browser-created section drafts (`workshop`, `tools`). */
  const sectionDrafts = new Map<string, EntryData>();
  /** Browser-created keyed-array drafts, not yet saved. */
  const arrayDrafts: Record<"dominion" | "endpoint", EntryData[]> = {
    dominion: [],
    endpoint: [],
  };
  /** Expanded entry cards, by card key. */
  const expanded = new Set<string>();
  /** Secret fields whose Change button has been clicked, `key:path`. */
  const revealed = new Set<string>();

  let system: SystemSnapshot | null = null;
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollController: AbortController | null = null;
  /** The System panel's live region; polling stops once it disconnects. */
  let liveBox: HTMLElement | null = null;

  store.subscribe(() => {
    // Guard on this view's own root, not just `main`: `main` is shared
    // with every other view, so a store notification arriving while
    // another view owns it must not let this one repaint the pane.
    if (main?.isConnected && viewRoot?.isConnected) {
      render();
    }
  });

  // ----- edit state --------------------------------------------------------

  /** The card's data with every unsaved edit applied. */
  const effective = (card: Card): EntryData => {
    const data = structuredClone(card.base);
    for (const [path, value] of edits.get(card.key) ?? []) {
      writePath(data, path, value);
    }
    return data;
  };

  const value = (card: Card, path: string): unknown => readPath(effective(card), path);

  const isEdited = (card: Card, path: string): boolean =>
    edits.get(card.key)?.has(path) ?? false;

  const hasEdits = (card: Card): boolean =>
    card.draft || (edits.get(card.key)?.size ?? 0) > 0;

  const commit = (card: Card, path: string, next: unknown): void => {
    if (card.draft) {
      writePath(card.base, path, next);
      render();
      return;
    }
    let map = edits.get(card.key);
    if (!map) {
      map = new Map();
      edits.set(card.key, map);
    }
    if (sameJson(next, readPath(card.base, path))) {
      map.delete(path);
    } else {
      map.set(path, next);
    }
    render();
  };

  const resetField = (card: Card, path: string): void => {
    edits.get(card.key)?.delete(path);
    render();
  };

  // ----- rendering scaffold ------------------------------------------------

  const render = (): void => {
    if (!main) {
      return;
    }
    const title = document.createElement("h1");
    title.className = "view-title";
    title.textContent = "Settings";

    const split = document.createElement("div");
    split.className = "settings-split";
    split.append(buildNav(), buildPanel());
    viewRoot = split;
    main.replaceChildren(title, split);
  };

  const buildNav = (): HTMLElement => {
    const nav = document.createElement("nav");
    nav.className = "settings-nav";
    nav.setAttribute("aria-label", "Settings sections");
    const list = document.createElement("ul");
    for (const item of SECTIONS) {
      const row = document.createElement("li");
      const link = document.createElement("a");
      link.className = "settings-nav-link";
      link.href = `#/settings/${item.id}`;
      link.textContent = item.label;
      if (item.id === section) {
        link.setAttribute("aria-current", "true");
      }
      row.append(link);
      list.append(row);
    }
    nav.append(list);
    return nav;
  };

  const buildPanel = (): HTMLElement => {
    const panel = document.createElement("div");
    panel.className = "settings-panel";
    panel.dataset["section"] = section;
    if (section !== "system" && section !== "about" && !store.loaded) {
      const skeleton = document.createElement("div");
      skeleton.className = "skeleton-row";
      skeleton.setAttribute("aria-hidden", "true");
      panel.append(skeleton);
      return panel;
    }
    if (section !== "system" && section !== "about" && store.loadError) {
      panel.append(loadErrorBanner(store.loadError));
      return panel;
    }
    switch (section) {
      case "system":
        renderSystem(panel);
        break;
      case "gateway":
        renderGateway(panel);
        break;
      case "workshop":
        renderWorkshop(panel);
        break;
      case "dominions":
        renderDominions(panel);
        break;
      case "endpoints":
        renderEndpoints(panel);
        break;
      case "tools":
        renderTools(panel);
        break;
      case "about":
        renderAbout(panel);
        break;
    }
    return panel;
  };

  const loadErrorBanner = (message: string): HTMLElement => {
    const banner = document.createElement("div");
    banner.className = "banner banner-danger";
    const text = document.createElement("span");
    text.textContent = `Could not load the configuration: ${message}`;
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "button button-xs button-outline";
    retry.textContent = "Retry";
    retry.addEventListener("click", () => void store.load());
    banner.append(text, retry);
    return banner;
  };

  return {
    mount(target: HTMLElement, sectionId?: string): () => void {
      main = target;
      viewRoot = null;
      section = (SECTIONS.some((item) => item.id === sectionId)
        ? sectionId
        : "system") as SectionId;
      render();
      if (section === "system") {
        ensurePolling();
      } else {
        stopPolling();
      }
      return () => {
        stopPolling();
        main = null;
        viewRoot = null;
        liveBox = null;
      };
    },
  };

  // ----- System ------------------------------------------------------------

  function ensurePolling(): void {
    if (pollTimer !== null) {
      return;
    }
    const tick = async (): Promise<void> => {
      if (!liveBox?.isConnected) {
        stopPolling();
        return;
      }
      let snapshot: SystemSnapshot;
      pollController?.abort();
      const controller = new AbortController();
      pollController = controller;
      try {
        snapshot = await api.getSystem(controller.signal);
      } catch (error) {
        if (
          error !== null &&
          typeof error === "object" &&
          "name" in error &&
          error.name === "AbortError"
        ) {
          return;
        }
        // A failed poll keeps the last snapshot; the next tick retries.
        return;
      } finally {
        if (pollController === controller) {
          pollController = null;
        }
      }
      system = snapshot;
      if (liveBox?.isConnected) {
        renderLive(liveBox);
      }
    };
    void tick();
    pollTimer = setInterval(() => void tick(), SYSTEM_POLL_MS);
    // In tests the interval must not hold the node process open.
    (pollTimer as unknown as { unref?: () => void }).unref?.();
  }

  function stopPolling(): void {
    pollController?.abort();
    pollController = null;
    if (pollTimer !== null) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  function renderSystem(panel: HTMLElement): void {
    // The live region re-renders on each poll tick without touching the
    // Storage card, so typing in cache_dir survives the 5s refresh.
    liveBox = document.createElement("div");
    liveBox.className = "system-live";
    renderLive(liveBox);
    panel.append(liveBox);
    if (store.loaded && !store.loadError) {
      panel.append(storageCard());
    }
  }

  function renderLive(box: HTMLElement): void {
    box.replaceChildren(metricGrid(), ...gpuDevices());
  }

  function metricGrid(): HTMLElement {
    const grid = document.createElement("div");
    grid.className = "metric-grid";
    if (!system) {
      grid.setAttribute("aria-hidden", "true");
      for (let i = 0; i < 4; i += 1) {
        const tile = document.createElement("div");
        tile.className = "metric-tile skeleton-row";
        grid.append(tile);
      }
      return grid;
    }
    grid.append(cpuTile(system), ramTile(system));
    if (system.gpu) {
      grid.append(vramTile(system.gpu));
    }
    grid.append(diskTile(system));
    return grid;
  }

  function metricTile(
    kind: string,
    icon: Parameters<typeof lucideElement>[0],
    label: string,
  ): HTMLElement {
    const tile = document.createElement("div");
    tile.className = `metric-tile metric-${kind}`;
    const head = document.createElement("p");
    head.className = "metric-label";
    head.append(
      lucideElement(icon, { "aria-hidden": "true", width: 14, height: 14 }),
      document.createTextNode(` ${label}`),
    );
    tile.append(head);
    return tile;
  }

  function metricBar(fraction: number, segmented = false): HTMLElement {
    const bar = document.createElement("div");
    bar.className = segmented ? "metric-bar metric-bar-segmented" : "metric-bar";
    const fill = document.createElement("div");
    fill.className = "metric-bar-fill";
    const clamped = Math.min(1, Math.max(0, fraction));
    fill.style.setProperty("--progress", String(clamped));
    if (clamped >= 0.9) {
      fill.classList.add("is-danger");
    } else if (clamped >= 0.7) {
      fill.classList.add("is-warning");
    }
    bar.append(fill);
    if (segmented) {
      const divider = document.createElement("span");
      divider.className = "metric-seg-divider";
      divider.style.setProperty("--progress", String(clamped));
      bar.append(divider);
    }
    return bar;
  }

  function metricValue(text: string): HTMLElement {
    const valueRow = document.createElement("p");
    valueRow.className = "metric-value";
    valueRow.textContent = text;
    return valueRow;
  }

  function metricSub(text: string): HTMLElement {
    const sub = document.createElement("p");
    sub.className = "metric-sub";
    sub.textContent = text;
    return sub;
  }

  function cpuTile(snapshot: SystemSnapshot): HTMLElement {
    const tile = metricTile("cpu", Cpu, "CPU");
    const cpu = snapshot.cpu;
    if (!cpu) {
      tile.append(metricValue("Unavailable"));
      return tile;
    }
    tile.append(metricValue(`${(cpu.frequency_mhz / 1000).toFixed(2)} GHz`));
    const cores =
      cpu.physical_cores === null
        ? `${cpu.logical_cores} logical`
        : `${cpu.logical_cores} logical / ${cpu.physical_cores} physical`;
    tile.append(metricSub(cores), metricBar(cpu.utilization_percent / 100));
    return tile;
  }

  function ramTile(snapshot: SystemSnapshot): HTMLElement {
    const tile = metricTile("ram", MemoryStick, "RAM");
    const ram = snapshot.ram;
    tile.append(
      metricValue(`${gib(ram.used_bytes)} / ${gib(ram.total_bytes)} GiB`),
      metricBar(ram.total_bytes > 0 ? ram.used_bytes / ram.total_bytes : 0),
    );
    return tile;
  }

  function vramTile(gpu: NonNullable<SystemSnapshot["gpu"]>): HTMLElement {
    const tile = metricTile("vram", Microchip, "VRAM");
    tile.append(
      metricValue(`${gib(gpu.vram_used_bytes)} / ${gib(gpu.vram_total_bytes)} GiB`),
      metricBar(gpu.vram_total_bytes > 0 ? gpu.vram_used_bytes / gpu.vram_total_bytes : 0, true),
    );
    const sub = document.createElement("p");
    sub.className = "metric-sub gpu-name";
    sub.textContent = gpu.name;
    const vendor = vendorOf(gpu.name);
    if (vendor) {
      const chip = document.createElement("span");
      chip.className = "pill vendor-chip";
      chip.dataset["vendor"] = vendor.label;
      chip.style.color = vendor.color;
      chip.textContent = vendor.label;
      sub.append(document.createTextNode(" "), chip);
    }
    tile.append(sub);
    return tile;
  }

  function diskTile(snapshot: SystemSnapshot): HTMLElement {
    const tile = metricTile("disk", HardDrive, "Disk");
    const disk = snapshot.disk;
    if (!disk) {
      tile.append(metricValue("Unavailable"));
      return tile;
    }
    tile.append(
      metricValue(`${formatBytes(disk.used_bytes)} / ${formatBytes(disk.total_bytes)}`),
      metricSub(disk.cache_dir),
      metricBar(disk.total_bytes > 0 ? disk.used_bytes / disk.total_bytes : 0),
    );
    return tile;
  }

  function gpuDevices(): HTMLElement[] {
    if (!system?.gpu) {
      return [];
    }
    const gpu = system.gpu;
    const wrap = document.createElement("section");
    wrap.className = "gpu-devices";
    const heading = document.createElement("h2");
    heading.className = "section-heading";
    heading.textContent = "GPU Devices";
    const row = document.createElement("div");
    row.className = "gpu-device-row";
    const name = document.createElement("span");
    name.className = "gpu-name";
    name.textContent = gpu.name;
    const pill = document.createElement("span");
    pill.className = "pill pill-accent gpu-vram-pill";
    pill.textContent = `${gib(gpu.vram_used_bytes)} / ${gib(gpu.vram_total_bytes)} GiB`;
    const readings = document.createElement("span");
    readings.className = "metric-value gpu-vram-readings";
    readings.textContent = `${formatBytes(gpu.vram_used_bytes)} used`;
    row.append(
      name,
      pill,
      metricBar(gpu.vram_total_bytes > 0 ? gpu.vram_used_bytes / gpu.vram_total_bytes : 0, true),
      readings,
    );
    wrap.append(heading, row);
    return [wrap];
  }

  // ----- the field grammar (step-16 dirty/pending/reset) --------------------

  function fieldRow(card: Card, spec: FieldSpec): HTMLElement {
    const row = document.createElement("div");
    row.className = "field-row";
    row.dataset["key"] = spec.path;
    const id = `field-${card.key.replace(/[^a-z0-9]+/gi, "-")}-${spec.path.replace(/\./g, "-")}`;

    const head = document.createElement("div");
    head.className = "field-head";
    const label = document.createElement("label");
    label.id = `${id}-label`;
    label.htmlFor = id;
    label.textContent = spec.label;
    head.append(label);

    if (isEdited(card, spec.path)) {
      const dot = document.createElement("span");
      dot.className = "dirty-dot";
      const hidden = document.createElement("span");
      hidden.className = "visually-hidden";
      hidden.textContent = "edited";
      dot.append(hidden);
      head.append(dot);

      const reset = document.createElement("button");
      reset.type = "button";
      reset.className = "field-reset";
      reset.setAttribute("aria-label", `Reset ${spec.label}`);
      reset.append(lucideElement(RotateCcw, { "aria-hidden": "true", width: 12, height: 12 }));
      reset.addEventListener("click", () => resetField(card, spec.path));
      head.append(reset);
    }

    const rootKey = spec.path.split(".")[0] ?? spec.path;
    if (!card.draft && card.pendingFields.has(rootKey)) {
      const chip = document.createElement("span");
      chip.className = "pill pill-accent pending-chip";
      chip.textContent = "pending";
      const running = card.runningPrefix
        ? store.runningSectionValue(`${card.runningPrefix}.${spec.path}`)
        : undefined;
      chip.title =
        running === undefined
          ? "Saved to the shadow; not yet in the running configuration."
          : `Running value: ${JSON.stringify(running)}`;
      head.append(chip);
    }

    row.append(head);
    const current = value(card, spec.path);

    if (spec.type === "toggle") {
      const toggle = createToggleControl({
        id,
        labelledBy: label.id,
        checked: typeof current === "boolean" ? current : Boolean(spec.fallback),
        onChange: (next) => commit(card, spec.path, next),
      });
      if (spec.locked) {
        toggle.setDisabled(true);
      }
      row.append(toggle.element);
    } else if (spec.type === "dropdown") {
      const options = (spec.options ?? []).map((option) => ({ value: option, label: option }));
      if (spec.allowNone) {
        options.unshift({ value: "", label: "None" });
      }
      const dropdown = createDropdownControl({
        id,
        options,
        value: current == null ? String(spec.fallback ?? "") : String(current),
        onChange: (next) => commit(card, spec.path, next === "" ? null : next),
      });
      if (spec.locked) {
        dropdown.setDisabled(true);
      }
      row.append(dropdown.element);
    } else if (spec.type === "slider") {
      const slider = createSliderControl({
        id,
        min: spec.min ?? 0,
        max: spec.max ?? 100,
        step: spec.step ?? 1,
        logScale: false,
        value: typeof current === "number" ? current : Number(spec.fallback ?? spec.min ?? 0),
        onChange: (next) => commit(card, spec.path, next),
      });
      row.append(slider.element);
    } else if (spec.type === "chips") {
      const chips = createChipInput({
        id,
        values: Array.isArray(current) ? current.map(String) : [],
        onChange: (next) => commit(card, spec.path, next),
      });
      row.append(chips.element);
    } else if (spec.type === "secret") {
      row.append(secretControl(card, spec, id));
    } else {
      const input = document.createElement("input");
      input.type = "text";
      input.id = id;
      input.className = "input";
      if (spec.placeholder) {
        input.placeholder = spec.placeholder;
      }
      input.value = current == null ? "" : String(current);
      input.disabled = spec.locked ?? false;
      input.addEventListener("change", () => {
        const text = input.value.trim();
        if (spec.numeric) {
          if (text === "") {
            commit(card, spec.path, null);
            return;
          }
          const parsed = Number(text);
          if (Number.isFinite(parsed)) {
            commit(card, spec.path, parsed);
          }
          return;
        }
        commit(card, spec.path, text === "" && spec.fallback === null ? null : input.value);
      });
      row.append(input);
    }

    const help = document.createElement("p");
    help.className = "field-help";
    help.id = `${id}-help`;
    help.textContent = spec.help;
    row.append(help);
    return row;
  }

  /**
   * The Change-reveal secret control: an untouched `"***"` renders as a
   * masked readout with a Change button; only after Change (or for a
   * never-saved secret) does the password input render. Leaving the input
   * empty keeps the existing value - the payload still carries `"***"`.
   */
  function secretControl(card: Card, spec: FieldSpec, id: string): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "secret-field";
    const baseline = readPath(card.base, spec.path);
    const masked = baseline === "***";
    const revealKey = `${card.key}:${spec.path}`;
    if (masked && !revealed.has(revealKey) && !isEdited(card, spec.path)) {
      const readout = document.createElement("span");
      readout.className = "secret-mask";
      readout.textContent = "•••";
      const change = document.createElement("button");
      change.type = "button";
      change.id = id;
      change.className = "button button-xs button-outline secret-change";
      change.textContent = "Change";
      change.addEventListener("click", () => {
        revealed.add(revealKey);
        render();
      });
      wrap.append(readout, change);
      return wrap;
    }
    const input = document.createElement("input");
    input.type = "password";
    input.id = id;
    input.className = "input secret-input";
    const current = value(card, spec.path);
    input.value = isEdited(card, spec.path) ? String(current ?? "") : masked ? "" : String(current ?? "");
    input.placeholder = masked ? "Leave empty to keep the current key" : "";
    input.addEventListener("change", () => {
      const text = input.value;
      if (text === "" && masked) {
        resetField(card, spec.path);
        return;
      }
      commit(card, spec.path, text);
    });
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "button button-xs button-outline secret-toggle";
    let shown = false;
    const paintToggle = (): void => {
      toggle.setAttribute("aria-label", shown ? "Hide" : "Show");
      toggle.setAttribute("aria-pressed", String(shown));
      toggle.replaceChildren(
        lucideElement(shown ? EyeOff : Eye, { "aria-hidden": "true", width: 14, height: 14 }),
      );
    };
    toggle.addEventListener("click", () => {
      shown = !shown;
      input.type = shown ? "text" : "password";
      paintToggle();
    });
    paintToggle();
    wrap.append(input, toggle);
    return wrap;
  }

  /** A titled settings card. */
  function settingsCard(title: string): { card: HTMLElement; body: HTMLElement } {
    const card = document.createElement("section");
    card.className = "settings-card";
    const heading = document.createElement("h2");
    heading.className = "section-heading";
    heading.textContent = title;
    const body = document.createElement("div");
    body.className = "section-body";
    card.append(heading, body);
    return { card, body };
  }

  /** The per-card Save button, enabled while the card carries edits. */
  function saveButton(card: Card, onSave: () => Promise<void>): HTMLElement {
    const actions = document.createElement("div");
    actions.className = "detail-actions";
    const save = document.createElement("button");
    save.type = "button";
    save.className = "button button-primary card-save";
    save.textContent = "Save";
    save.disabled = !hasEdits(card);
    save.addEventListener("click", () => {
      save.disabled = true;
      void (async () => {
        try {
          await onSave();
        } catch (error) {
          toasts.show(error instanceof Error ? error.message : "The save failed", "error");
          render();
          return;
        }
        toasts.show("Saved to disk", "success");
      })();
    });
    actions.append(save);
    return actions;
  }

  // ----- global configuration saves -----------------------------------------

  /**
   * Saves one global section through the single configuration shadow.
   */
  async function saveGlobalCard(card: Card, sectionKey: "server" | "workshop"): Promise<void> {
    const payload = store.buildConfigPayload();
    payload[sectionKey] = effective(card);
    await store.savePayload(payload);
    // The store already notified (and re-rendered) while these edits were
    // still held, so clear them and render again: otherwise a just-saved
    // secret stays in a live password input instead of the masked readout.
    edits.delete(card.key);
    if (card.draft) {
      sectionDrafts.delete(card.key);
    }
    revealed.clear();
    render();
  }

  function restartNote(): HTMLElement {
    const note = document.createElement("p");
    note.className = "field-help restart-note";
    note.textContent =
      "Restart required to apply: the gateway cannot hot-reload its boot configuration.";
    return note;
  }

  function renderGateway(panel: HTMLElement): void {
    const server = store.sectionValue("server");
    const card: Card = {
      key: "server",
      base: server !== null && typeof server === "object" ? (server as EntryData) : {},
      draft: false,
      pendingFields: bootPendingFields("server"),
      runningPrefix: "server",
    };
    const { card: box, body } = settingsCard("Gateway ([server])");
    body.append(
      fieldRow(card, {
        path: "bind",
        label: "Bind",
        help: "The socket address the gateway listener binds (host:port).",
        type: "input",
        fallback: "",
      }),
      fieldRow(card, {
        path: "api_key",
        label: "API key",
        help: "The bearer key every API caller must present.",
        type: "secret",
      }),
    );
    if (isEdited(card, "api_key")) {
      const warning = document.createElement("p");
      warning.className = "banner banner-warning new-key-warning";
      warning.textContent = "After restart, you will need to enter the new API key.";
      body.append(warning);
    }
    body.append(restartNote(), saveButton(card, () => saveGlobalCard(card, "server")));
    panel.append(box, configUiCard(card));
  }

  /** Field keys of a boot section whose pending value differs from running. */
  function bootPendingFields(sectionKey: string): ReadonlySet<string> {
    const pending = store.sectionValue(sectionKey);
    const running = store.runningSectionValue(sectionKey);
    const diff = new Set<string>();
    const pendingObj = pending !== null && typeof pending === "object" ? (pending as EntryData) : {};
    const runningObj = running !== null && typeof running === "object" ? (running as EntryData) : {};
    for (const key of new Set([...Object.keys(pendingObj), ...Object.keys(runningObj)])) {
      if (!sameJson(pendingObj[key], runningObj[key])) {
        diff.add(key);
      }
    }
    return diff;
  }

  /** The informational Config UI card: enabled state and the derived URL. */
  function configUiCard(serverCard: Card): HTMLElement {
    const { card, body } = settingsCard("Config UI");
    const status = document.createElement("p");
    status.className = "configui-status";
    const pill = document.createElement("span");
    pill.className = "pill pill-accent";
    pill.textContent = "Enabled";
    status.append(pill);
    const url = document.createElement("p");
    url.className = "metric-value configui-url";
    url.textContent = configUiUrl(String(value(serverCard, "bind") ?? ""));
    const note = document.createElement("p");
    note.className = "field-help";
    note.textContent =
      "Compiled in by the config-ui feature and served on the gateway's own port, loopback only. Nothing to configure.";
    body.append(status, url, note);
    return card;
  }

  function renderWorkshop(panel: HTMLElement): void {
    const pending = store.sectionValue("workshop");
    const draft = sectionDrafts.get("workshop");
    if ((pending === null || pending === undefined) && !draft) {
      const { card, body } = settingsCard("Workshop");
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "Workshop not configured.";
      const enable = document.createElement("button");
      enable.type = "button";
      enable.className = "button button-primary workshop-enable";
      enable.textContent = "Enable Workshop";
      enable.addEventListener("click", () => {
        sectionDrafts.set("workshop", workshopDefaults());
        render();
      });
      body.append(empty, enable);
      panel.append(card);
      return;
    }
    const card: Card = draft
      ? { key: "workshop", base: draft, draft: true, pendingFields: new Set() }
      : {
          key: "workshop",
          base: pending as EntryData,
          draft: false,
          pendingFields: bootPendingFields("workshop"),
          runningPrefix: "workshop",
        };
    const { card: box, body } = settingsCard("Workshop ([workshop])");
    body.append(
      fieldRow(card, {
        path: "bind",
        label: "Bind",
        help: "The socket address the workshop listener binds.",
        type: "input",
        fallback: "127.0.0.1:7910",
      }),
      fieldRow(card, {
        path: "open_browser",
        label: "Open browser",
        help: "Open the system browser at the workshop URL once it is serving.",
        type: "toggle",
        fallback: false,
      }),
      workshopSubsection(card, "stt", "STT capture tuning", sttDefaults, [
        {
          path: "stt.window_seconds",
          label: "Window seconds",
          help: "Seconds of trailing audio each interim pass transcribes.",
          type: "input",
          numeric: true,
          placeholder: "15",
        },
        {
          path: "stt.interval_ms",
          label: "Interval (ms)",
          help: "Milliseconds between interim passes while a take is recording.",
          type: "input",
          numeric: true,
          placeholder: "500",
        },
        {
          path: "stt.vocabulary",
          label: "Vocabulary",
          help: "Domain terms whisper is biased toward.",
          type: "chips",
        },
      ]),
      restoreRecommendedButton(),
      restartNote(),
      saveButton(card, () => saveGlobalCard(card, "workshop")),
    );
    panel.append(box);
  }

  /** A collapsible `[workshop.stt]` subsection. */
  function workshopSubsection(
    card: Card,
    key: string,
    label: string,
    seed: () => EntryData,
    fields: FieldSpec[],
  ): HTMLElement {
    const wrap = document.createElement("section");
    wrap.className = `workshop-sub workshop-${key}`;
    if (value(card, key) == null) {
      const add = document.createElement("button");
      add.type = "button";
      add.className = `button button-outline section-add add-${key}`;
      add.textContent = `Add ${label.toLowerCase()} settings`;
      add.addEventListener("click", () => {
        expanded.add(`workshop:${key}`);
        commit(card, key, seed());
      });
      wrap.append(add);
      return wrap;
    }
    const heading = document.createElement("h3");
    heading.className = "section-heading";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "section-toggle";
    const collapseKey = `workshop:${key}`;
    toggle.setAttribute("aria-expanded", String(expanded.has(collapseKey)));
    toggle.textContent = label;
    heading.append(toggle);
    const body = document.createElement("div");
    body.className = "section-body";
    body.hidden = !expanded.has(collapseKey);
    toggle.addEventListener("click", () => {
      if (expanded.has(collapseKey)) {
        expanded.delete(collapseKey);
      } else {
        expanded.add(collapseKey);
      }
      body.hidden = !expanded.has(collapseKey);
      toggle.setAttribute("aria-expanded", String(!body.hidden));
    });
    for (const spec of fields) {
      body.append(fieldRow(card, spec));
    }
    wrap.append(heading, body);
    return wrap;
  }

  function restoreRecommendedButton(): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "restore-stt";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "button button-outline restore-recommended";
    button.textContent = "Restore recommended models";
    button.addEventListener("click", () => {
      button.disabled = true;
      void store
        .restoreSttModels(RECOMMENDED_STT_MODELS)
        .then(() => toasts.show("Recommended STT models restored", "success"))
        .catch((error: unknown) => {
          button.disabled = false;
          toasts.show(error instanceof Error ? error.message : "The models could not be restored", "error");
        });
    });
    const help = document.createElement("p");
    help.className = "field-help";
    help.textContent =
      "Creates or resets the digest-pinned interim and final speech models in the global catalog.";
    wrap.append(button, help);
    return wrap;
  }

  // ----- Storage ([local]) ---------------------------------------------------

  function storageCard(): HTMLElement {
    const local = store.sectionValue("local");
    const card: Card = {
      key: "local",
      base: local !== null && typeof local === "object" ? (local as EntryData) : {},
      draft: false,
      pendingFields: bootPendingFields("local"),
      runningPrefix: "local",
    };
    const { card: box, body } = settingsCard("Storage ([local])");
    body.append(
      fieldRow(card, {
        path: "cache_dir",
        label: "Cache directory",
        help: "Where downloaded artifacts live.",
        type: "input",
        fallback: null,
        placeholder: "~/.promptforge",
      }),
    );
    if (system?.disk) {
      const usage = document.createElement("p");
      usage.className = "metric-sub storage-usage";
      usage.textContent = `Cache drive: ${formatBytes(system.disk.used_bytes)} used of ${formatBytes(system.disk.total_bytes)}`;
      body.append(usage);
    }
    const warning = document.createElement("p");
    warning.className = "banner banner-warning storage-warning";
    warning.textContent = "Changing cache_dir does not move existing files.";
    body.append(
      warning,
      saveButton(card, async () => {
        const payload = store.buildConfigPayload();
        payload["local"] = effective(card);
        await store.savePayload(payload);
        edits.delete(card.key);
        render();
      }),
    );
    return box;
  }

  // ----- Dominions -----------------------------------------------------------

  /** Human-named dependents of a dominion: endpoints and local models bound to it. */
  function dominionDependents(id: string): string[] {
    const dependents: string[] = [];
    for (const endpoint of store.keyedEntries("endpoint")) {
      if (endpoint.data["dominion"] === id) {
        dependents.push(`endpoint '${endpoint.id}'`);
      }
    }
    for (const row of store.modelEntriesRaw()) {
      if (
        (row.array === "local_model" || row.array === "stt_model") &&
        row.data["dominion"] === id
      ) {
        dependents.push(
          `${row.array === "stt_model" ? "STT" : "local"} model '${String(row.data["name"] ?? "")}'`,
        );
      }
    }
    return dependents;
  }

  /** Human-named dependents of an endpoint: models routing through it. */
  function endpointDependents(id: string): string[] {
    const dependents: string[] = [];
    for (const row of store.modelEntriesRaw()) {
      const endpoints = row.data["endpoints"];
      if (row.array === "model" && Array.isArray(endpoints) && endpoints.includes(id)) {
        dependents.push(`model '${String(row.data["name"] ?? "")}'`);
      }
    }
    return dependents;
  }

  /** An expandable keyed-array entry card with used-by chips and delete. */
  function entryCard(options: {
    card: Card;
    title: string;
    dependents: string[];
    fields: () => HTMLElement[];
    onSave: () => Promise<void>;
    onDelete: () => void;
  }): HTMLElement {
    const { card } = options;
    const box = document.createElement("section");
    box.className = "settings-card entry-card";
    box.dataset["entry"] = card.key;

    const heading = document.createElement("h2");
    heading.className = "section-heading";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "section-toggle entry-toggle";
    toggle.setAttribute("aria-expanded", String(expanded.has(card.key)));
    toggle.textContent = options.title;
    heading.append(toggle);
    if (options.dependents.length > 0) {
      const chip = document.createElement("span");
      chip.className = "pill used-by-chip";
      chip.textContent = `used by ${options.dependents.length}`;
      chip.title = options.dependents.join(", ");
      heading.append(chip);
    }
    if (card.draft) {
      const draftBadge = document.createElement("span");
      draftBadge.className = "pill draft-badge";
      draftBadge.textContent = "unsaved";
      heading.append(draftBadge);
    }
    box.append(heading);
    const body = document.createElement("div");
    body.className = "section-body";
    body.hidden = !expanded.has(card.key);
    toggle.addEventListener("click", () => {
      if (expanded.has(card.key)) {
        expanded.delete(card.key);
      } else {
        expanded.add(card.key);
      }
      body.hidden = !expanded.has(card.key);
      toggle.setAttribute("aria-expanded", String(!body.hidden));
    });
    for (const field of options.fields()) {
      body.append(field);
    }

    const actions = saveButton(card, options.onSave);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-danger entry-delete";
    remove.textContent = card.draft ? "Discard" : "Delete";
    remove.addEventListener("click", options.onDelete);
    actions.append(remove);
    body.append(actions);
    box.append(body);
    return box;
  }

  function renderDominions(panel: HTMLElement): void {
    const entries = store.keyedEntries("dominion");
    for (const entry of entries) {
      const card: Card = {
        key: `dominion:${entry.id}`,
        base: entry.data,
        draft: false,
        pendingFields: entry.pendingFields,
      };
      panel.append(dominionCard(card, entry.id));
    }
    arrayDrafts.dominion.forEach((data, index) => {
      const card: Card = {
        key: `dominion-draft:${index}`,
        base: data,
        draft: true,
        pendingFields: new Set(),
      };
      panel.append(dominionCard(card, String(data["id"] ?? "")));
    });
    panel.append(
      addEntryButton("Add Dominion", () => {
        const draft = {
          id: "",
          kind: "remote",
          max_queue: 100,
          policy: "queue",
          fair_scheduling: true,
        };
        arrayDrafts.dominion.push(draft);
        const key = `dominion-draft:${arrayDrafts.dominion.length - 1}`;
        expanded.add(key);
        render();
        focusIdField(key);
      }),
    );
  }

  function dominionCard(card: Card, id: string): HTMLElement {
    const dependents = card.draft ? [] : dominionDependents(id);
    return entryCard({
      card,
      title: id || "(new dominion)",
      dependents,
      fields: () => {
        const rows = [
          fieldRow(card, {
            path: "id",
            label: "Id",
            help: "The handle endpoints and local models bind to.",
            type: "input",
            fallback: "",
          }),
          fieldRow(card, {
            path: "kind",
            label: "Kind",
            help: "Remote pools govern endpoints; local pools govern GPU co-residency.",
            type: "dropdown",
            options: ["remote", "local"],
            fallback: "remote",
          }),
          fieldRow(card, {
            path: "max_concurrency",
            label: "Max concurrency",
            help: "Max concurrent requests across all endpoints bound to this dominion.",
            type: "input",
            numeric: true,
            placeholder: "Unlimited",
            fallback: null,
          }),
          fieldRow(card, {
            path: "max_queue",
            label: "Max queue",
            help: "How many requests wait when concurrency is full.",
            type: "slider",
            min: 0,
            max: 500,
            step: 10,
            fallback: 100,
          }),
          fieldRow(card, {
            path: "policy",
            label: "Policy",
            help: "Queue waits for a slot; Reject fails immediately when full.",
            type: "dropdown",
            options: ["queue", "reject"],
            fallback: "queue",
          }),
          fieldRow(card, {
            path: "fair_scheduling",
            label: "Fair scheduling",
            help: "Round-robin by client key prevents one caller from monopolizing the pool.",
            type: "toggle",
            fallback: true,
          }),
        ];
        if (value(card, "kind") === "local") {
          rows.push(
            fieldRow(card, {
              path: "vram_gb",
              label: "VRAM (GiB)",
              help: "VRAM budget in GiB for co-residency checks.",
              type: "input",
              numeric: true,
              fallback: null,
            }),
          );
        }
        return rows;
      },
      onSave: () => saveArrayEntry("dominion", card, id),
      onDelete: () => void deleteArrayEntry("dominion", card, id, dependents),
    });
  }

  // ----- Endpoints -----------------------------------------------------------

  function renderEndpoints(panel: HTMLElement): void {
    for (const entry of store.keyedEntries("endpoint")) {
      const card: Card = {
        key: `endpoint:${entry.id}`,
        base: entry.data,
        draft: false,
        pendingFields: entry.pendingFields,
      };
      panel.append(endpointCard(card, entry.id));
    }
    arrayDrafts.endpoint.forEach((data, index) => {
      const card: Card = {
        key: `endpoint-draft:${index}`,
        base: data,
        draft: true,
        pendingFields: new Set(),
      };
      panel.append(endpointCard(card, String(data["id"] ?? "")));
    });
    panel.append(
      addEntryButton("Add Endpoint", () => {
        arrayDrafts.endpoint.push({ id: "", protocol: "openai", base_url: "", api_key: "" });
        const key = `endpoint-draft:${arrayDrafts.endpoint.length - 1}`;
        expanded.add(key);
        render();
        focusIdField(key);
      }),
    );
  }

  function endpointCard(card: Card, id: string): HTMLElement {
    const dependents = card.draft ? [] : endpointDependents(id);
    return entryCard({
      card,
      title: id || "(new endpoint)",
      dependents,
      fields: () => [
        fieldRow(card, {
          path: "id",
          label: "Id",
          help: "The handle [[model]] entries route through.",
          type: "input",
          fallback: "",
        }),
        fieldRow(card, {
          path: "protocol",
          label: "Protocol",
          help: "The wire protocol this endpoint speaks.",
          type: "dropdown",
          options: ["openai"],
          fallback: "openai",
          locked: true,
        }),
        fieldRow(card, {
          path: "base_url",
          label: "Base URL",
          help: "The backend's base URL (e.g. https://api.openai.com/v1).",
          type: "input",
          fallback: "",
        }),
        fieldRow(card, {
          path: "api_key",
          label: "API key",
          help: "The credential sent to this backend.",
          type: "secret",
        }),
        fieldRow(card, {
          path: "dominion",
          label: "Dominion",
          help: "Shared concurrency pool governing this endpoint.",
          type: "dropdown",
          options: store
            .dominions()
            .filter((dominion) => dominion.kind === "remote")
            .map((dominion) => dominion.id),
          allowNone: true,
          fallback: null,
        }),
      ],
      onSave: () => saveArrayEntry("endpoint", card, id),
      onDelete: () => void deleteArrayEntry("endpoint", card, id, dependents),
    });
  }

  // ----- shared keyed-array save/delete --------------------------------------

  function addEntryButton(label: string, onAdd: () => void): HTMLElement {
    const add = document.createElement("button");
    add.type = "button";
    add.className = "button button-outline section-add entry-add";
    add.textContent = label;
    add.addEventListener("click", onAdd);
    return add;
  }

  /** Focuses a fresh draft card's id input ("empty id focused" per the plan). */
  function focusIdField(cardKey: string): void {
    main
      ?.querySelector<HTMLInputElement>(
        `.entry-card[data-entry='${cardKey}'] .field-row[data-key='id'] input`,
      )
      ?.focus();
  }

  async function saveArrayEntry(
    array: "dominion" | "endpoint",
    card: Card,
    id: string,
  ): Promise<void> {
    const payload = store.buildConfigPayload();
    const items = Array.isArray(payload[array]) ? (payload[array] as EntryData[]) : [];
    if (card.draft) {
      items.push(effective(card));
    } else {
      const index = items.findIndex((item) => item["id"] === id);
      if (index >= 0) {
        items[index] = effective(card);
      } else {
        items.push(effective(card));
      }
    }
    payload[array] = items;
    await store.savePayload(payload);
    // Clear-then-render, as in saveBootCard: the notify-render ran while
    // the edits and reveal state were still live.
    edits.delete(card.key);
    if (card.draft) {
      arrayDrafts[array] = arrayDrafts[array].filter((item) => item !== card.base);
    }
    revealed.clear();
    render();
  }

  async function deleteArrayEntry(
    array: "dominion" | "endpoint",
    card: Card,
    id: string,
    dependents: string[],
  ): Promise<void> {
    if (card.draft) {
      arrayDrafts[array] = arrayDrafts[array].filter((item) => item !== card.base);
      render();
      return;
    }
    const noun = array === "dominion" ? "dominion" : "endpoint";
    const warning =
      dependents.length > 0
        ? ` Warning: it is used by ${dependents.join(", ")}.`
        : "";
    const yes = await confirmDialog(document.body, {
      title: `Delete ${noun} '${id}'?`,
      body: `Remove the ${noun} '${id}' from the configuration. The change is staged until you apply it.${warning}`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!yes) {
      return;
    }
    const payload = store.buildConfigPayload();
    const items = Array.isArray(payload[array]) ? (payload[array] as EntryData[]) : [];
    payload[array] = items.filter((item) => item["id"] !== id);
    try {
      await store.savePayload(payload);
    } catch (error) {
      toasts.show(error instanceof Error ? error.message : "The delete failed", "error");
      return;
    }
    edits.delete(card.key);
    toasts.show(`Deleted ${noun} '${id}'`, "success");
  }

  // ----- Tools ---------------------------------------------------------------

  function renderTools(panel: HTMLElement): void {
    const webSearch = store.sectionValue("tools.web_search");
    const draft = sectionDrafts.get("tools");
    if ((webSearch === null || webSearch === undefined) && !draft) {
      const { card, body } = settingsCard("Web Search");
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "Web search not configured.";
      const enable = document.createElement("button");
      enable.type = "button";
      enable.className = "button button-primary tools-enable";
      enable.textContent = "Enable";
      enable.addEventListener("click", () => {
        sectionDrafts.set("tools", webSearchDefaults());
        render();
      });
      body.append(empty, enable);
      panel.append(card);
      return;
    }
    const card: Card = draft
      ? { key: "tools", base: draft, draft: true, pendingFields: new Set() }
      : {
          key: "tools",
          base: webSearch as EntryData,
          draft: false,
          pendingFields: bootPendingFields("tools.web_search"),
          runningPrefix: "tools.web_search",
        };
    const { card: box, body } = settingsCard("Web Search ([tools.web_search])");
    body.append(
      fieldRow(card, {
        path: "provider",
        label: "Provider",
        help: "The search provider backing the tool.",
        type: "dropdown",
        options: ["brave"],
        fallback: "brave",
        locked: true,
      }),
      fieldRow(card, {
        path: "api_key",
        label: "API key",
        help: "The credential sent to the search provider.",
        type: "secret",
      }),
      fieldRow(card, {
        path: "base_url",
        label: "Base URL",
        help: "The search API base URL; override to point at a proxy.",
        type: "input",
        placeholder: BRAVE_BASE_URL,
        fallback: null,
      }),
      fieldRow(card, {
        path: "default_count",
        label: "Default count",
        help: "Used when the request omits count.",
        type: "input",
        numeric: true,
        placeholder: "10",
        fallback: null,
      }),
      fieldRow(card, {
        path: "max_count",
        label: "Max count",
        help: "Clamp and over-fetch ceiling for result counts.",
        type: "input",
        numeric: true,
        placeholder: "20",
        fallback: null,
      }),
      fieldRow(card, {
        path: "max_per_host",
        label: "Max per host",
        help: "Diversity cap per hostname group.",
        type: "input",
        numeric: true,
        placeholder: "2",
        fallback: null,
      }),
      fieldRow(card, {
        path: "default_freshness",
        label: "Default freshness",
        help: "Applied when the request omits freshness and this is non-empty.",
        type: "input",
        fallback: "",
      }),
      fieldRow(card, {
        path: "default_safesearch",
        label: "Default safesearch",
        help: "Applied when the request omits safesearch and this is non-empty.",
        type: "input",
        fallback: "",
      }),
      fieldRow(card, {
        path: "strip_tracking",
        label: "Strip tracking",
        help: "Scrub known tracking query params from result URLs.",
        type: "toggle",
        fallback: true,
      }),
      saveButton(card, async () => {
        const payload = store.buildConfigPayload();
        const tools =
          payload["tools"] !== null && typeof payload["tools"] === "object"
            ? (payload["tools"] as EntryData)
            : {};
        tools["web_search"] = effective(card);
        payload["tools"] = tools;
        await store.savePayload(payload);
        edits.delete(card.key);
        sectionDrafts.delete("tools");
        revealed.clear();
        render();
      }),
    );
    panel.append(box);
  }

  // ----- About ---------------------------------------------------------------

  function renderAbout(panel: HTMLElement): void {
    const { card, body } = settingsCard("About");
    const medallion = document.createElement("img");
    medallion.src = "icons/promptforge-icon-1.png";
    medallion.alt = "";
    medallion.width = 64;
    medallion.height = 64;
    medallion.className = "about-medallion";
    const name = document.createElement("p");
    name.className = "about-name";
    name.textContent = "PromptForge Gateway";
    const version = document.createElement("p");
    version.className = "about-version metric-value";
    version.textContent = `Version ${APP_VERSION}`;
    const license = document.createElement("p");
    const link = document.createElement("a");
    link.href = "https://www.boost.org/LICENSE_1_0.txt";
    link.target = "_blank";
    link.rel = "noopener";
    link.className = "about-license";
    link.textContent = "Boost Software License 1.0 (opens in a new tab)";
    license.append(link);
    body.append(medallion, name, version, license);
    panel.append(card);
  }
}
