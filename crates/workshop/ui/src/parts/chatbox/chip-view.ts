// The one pill-drawing function. The live editor's NodeView
// (mention-chip.ts), the live attachments strip (chat-box.ts), and the
// static draft renderer (chat-box-view.ts) build a chip's DOM here, so a
// pill looks the same wherever it appears: an icon slot, a truncated
// label, and optionally a remove button. The NodeView asks for the
// button and wires it; the strip and the read-only renderer pass
// `removable: false` and get a pill without one. The remove button sits
// over the icon slot in the pill's own grid, so the stylesheet can swap
// the icon for the X while the editor is focused and the pill is hovered.
// The pill renders the chip's kind and tone as data attributes for the
// skin; icons are codicons, picked by the chip's named icon, then the
// label's extension, then a generic glyph.

import {
  ICON_CLOSE,
  ICON_FILE,
  ICON_FILE_CODE,
  ICON_FILE_MEDIA,
  ICON_FILE_TEXT,
  ICON_FOLDER,
  ICON_GLOBE,
  ICON_LINK,
  ICON_SYMBOL_KEYWORD,
} from "@workshop/look/icons";
import type { ChipRef } from "./types";

/** The glyph size on a pill. */
const PILL_ICON_SIZE_PX = 12;

/** The icons the owning part may name on a chip. Unknown names fall through to the extension map. */
const NAMED_ICONS: Readonly<Record<string, string>> = {
  file: ICON_FILE,
  "file-code": ICON_FILE_CODE,
  "file-image": ICON_FILE_MEDIA,
  "file-text": ICON_FILE_TEXT,
  folder: ICON_FOLDER,
  globe: ICON_GLOBE,
  image: ICON_FILE_MEDIA,
  link: ICON_LINK,
  command: ICON_SYMBOL_KEYWORD,
  terminal: ICON_SYMBOL_KEYWORD,
};

/** Label extensions (lower-case, no dot) to the icon drawn when no icon is named. */
const EXTENSION_ICONS: Readonly<Record<string, string>> = {
  md: ICON_FILE_TEXT,
  markdown: ICON_FILE_TEXT,
  txt: ICON_FILE_TEXT,
  rst: ICON_FILE_TEXT,
  ts: ICON_FILE_CODE,
  tsx: ICON_FILE_CODE,
  js: ICON_FILE_CODE,
  mjs: ICON_FILE_CODE,
  cjs: ICON_FILE_CODE,
  jsx: ICON_FILE_CODE,
  rs: ICON_FILE_CODE,
  py: ICON_FILE_CODE,
  lua: ICON_FILE_CODE,
  sh: ICON_FILE_CODE,
  ps1: ICON_FILE_CODE,
  css: ICON_FILE_CODE,
  html: ICON_FILE_CODE,
  json: ICON_FILE_CODE,
  toml: ICON_FILE_CODE,
  yaml: ICON_FILE_CODE,
  yml: ICON_FILE_CODE,
  png: ICON_FILE_MEDIA,
  jpg: ICON_FILE_MEDIA,
  jpeg: ICON_FILE_MEDIA,
  gif: ICON_FILE_MEDIA,
  webp: ICON_FILE_MEDIA,
  svg: ICON_FILE_MEDIA,
};

/** The named icon, else the label's extension, else the generic file glyph. */
function iconFor(chip: ChipRef): string {
  if (chip.icon !== undefined) {
    const named = NAMED_ICONS[chip.icon];
    if (named !== undefined) {
      return named;
    }
  }
  const dot = chip.label.lastIndexOf(".");
  if (dot > 0 && dot < chip.label.length - 1) {
    const byExtension = EXTENSION_ICONS[chip.label.slice(dot + 1).toLowerCase()];
    if (byExtension !== undefined) {
      return byExtension;
    }
  }
  return ICON_FILE;
}

/**
 * Parses one codicon string into a decorative `<svg>` at `size` pixels.
 * The markup is a static string from `@workshop/look/icons`, never data.
 */
function glyph(markup: string, size: number): SVGElement {
  const template = document.createElement("template");
  template.innerHTML = markup;
  const svg = template.content.firstElementChild as SVGElement;
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  return svg;
}

/**
 * Draws a chip's icon alone, as a decorative `<svg>`: the same glyph
 * the pill shows, for the typeahead row that offers the chip. The
 * default size is the pill's; the typeahead asks for its own.
 */
export function renderChipIcon(chip: ChipRef, size: number = PILL_ICON_SIZE_PX): SVGElement {
  return glyph(iconFor(chip), size);
}

/** How `renderChip` draws a pill. */
export interface RenderChipOptions {
  /** Draw the unwired remove button; default `true`. `false` omits it entirely. */
  readonly removable?: boolean;
}

/**
 * Draws a chip as a `ws-mention-chip` pill: icon slot, label, and, when
 * `removable` (the default), an unwired remove button. `data-kind` and
 * `data-tone` mirror the chip's fields and are absent when the fields
 * are. The element is non-editable so it behaves as an atom inside a
 * contenteditable.
 */
export function renderChip(chip: ChipRef, options?: RenderChipOptions): HTMLElement {
  const dom = document.createElement("span");
  dom.className = "ws-mention-chip";
  // setAttribute, not the contentEditable property: jsdom does not
  // reflect the property onto the attribute.
  dom.setAttribute("contenteditable", "false");
  if (chip.kind !== undefined) {
    dom.setAttribute("data-kind", chip.kind);
  }
  if (chip.tone !== undefined) {
    dom.setAttribute("data-tone", chip.tone);
  }

  const icon = document.createElement("span");
  icon.className = "ws-mention-chip__icon";
  icon.setAttribute("aria-hidden", "true");
  icon.appendChild(renderChipIcon(chip));

  const label = document.createElement("span");
  label.className = "ws-mention-chip__label";
  label.textContent = chip.label;

  dom.append(icon, label);

  if (options?.removable !== false) {
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "ws-mention-chip__remove";
    remove.setAttribute("aria-label", "Remove");
    remove.appendChild(glyph(ICON_CLOSE, PILL_ICON_SIZE_PX));
    dom.appendChild(remove);
  }
  return dom;
}
