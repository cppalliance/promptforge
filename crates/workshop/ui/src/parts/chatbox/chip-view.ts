// The one pill-drawing function. Both the live editor's NodeView
// (mention-chip.ts) and the static draft renderer (chat-box-view.ts)
// build a chip's DOM here, so a pill looks the same wherever it appears:
// an icon slot, a truncated label, and a remove button. The caller
// wires the remove button (the NodeView) or drops it (the read-only
// renderer). The pill carries the chip's kind and tone as data
// attributes for the skin; icons come from the chip's named icon, then
// the label's extension, then a generic glyph.

import {
  File,
  FileCode,
  FileImage,
  FileText,
  Folder,
  Globe,
  Image,
  Link,
  SquareSlash,
  Terminal,
  X,
  createElement,
  type IconNode,
} from "lucide";
import type { ChipRef } from "./types";

const ICON_SIZE_PX = 12;

/** The icons a host may name on a chip. Unknown names fall through to the extension map. */
const NAMED_ICONS: Readonly<Record<string, IconNode>> = {
  file: File,
  "file-code": FileCode,
  "file-image": FileImage,
  "file-text": FileText,
  folder: Folder,
  globe: Globe,
  image: Image,
  link: Link,
  command: SquareSlash,
  terminal: Terminal,
};

/** Label extensions (lower-case, no dot) to the icon drawn when no icon is named. */
const EXTENSION_ICONS: Readonly<Record<string, IconNode>> = {
  md: FileText,
  markdown: FileText,
  txt: FileText,
  rst: FileText,
  ts: FileCode,
  tsx: FileCode,
  js: FileCode,
  mjs: FileCode,
  cjs: FileCode,
  jsx: FileCode,
  rs: FileCode,
  py: FileCode,
  lua: FileCode,
  sh: FileCode,
  ps1: FileCode,
  css: FileCode,
  html: FileCode,
  json: FileCode,
  toml: FileCode,
  yaml: FileCode,
  yml: FileCode,
  png: FileImage,
  jpg: FileImage,
  jpeg: FileImage,
  gif: FileImage,
  webp: FileImage,
  svg: FileImage,
};

/** The named icon, else the label's extension, else the generic file glyph. */
function iconFor(chip: ChipRef): IconNode {
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
  return File;
}

/**
 * Draws a chip as a `ws-mention-chip` pill: icon slot, label, and an
 * unwired remove button. `data-kind` and `data-tone` mirror the chip's
 * fields and are absent when the fields are. The element is
 * non-editable so it behaves as an atom inside a contenteditable.
 */
export function renderChip(chip: ChipRef): HTMLElement {
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
  icon.appendChild(createElement(iconFor(chip), { width: ICON_SIZE_PX, height: ICON_SIZE_PX }));

  const label = document.createElement("span");
  label.className = "ws-mention-chip__label";
  label.textContent = chip.label;

  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "ws-mention-chip__remove";
  remove.setAttribute("aria-label", "Remove");
  remove.appendChild(createElement(X, { width: ICON_SIZE_PX, height: ICON_SIZE_PX }));

  dom.append(icon, label, remove);
  return dom;
}
