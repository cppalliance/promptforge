// Test-only reader for the built dist/app.css. The bundle is minified, so
// a test asks for one selector's declarations (and the media condition
// they sit under) instead of matching the stylesheet text. The parse is
// deliberately small: it opens @layer and @media blocks, skips
// @keyframes, and reads plain `property: value` pairs, which is every
// construct the gateway sheets use.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "dist");

let rulesPromise;

/** Removes whitespace and quotes so authored and minified spellings compare equal. */
export function squash(text) {
  return String(text ?? "").replace(/["'\s]/g, "");
}

/** A selector in the one spelling the minifier leaves: `:after` for `::after`. */
function normalizeSelector(selector) {
  return squash(selector).replace(/::(after|before|first-line|first-letter)/g, ":$1");
}

/** Splits on `separator` outside parentheses and brackets. */
function splitTop(text, separator) {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === "(" || ch === "[") {
      depth += 1;
    } else if (ch === ")" || ch === "]") {
      depth -= 1;
    } else if (ch === separator && depth === 0) {
      parts.push(text.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts;
}

/** Reads `a: b; c: d` into a Map, keeping the last value of a repeated property. */
function declarations(body) {
  const map = new Map();
  for (const part of splitTop(body, ";")) {
    const colon = part.indexOf(":");
    if (colon > 0) {
      map.set(part.slice(0, colon).trim(), part.slice(colon + 1).trim());
    }
  }
  return map;
}

/** Walks one block's text, pushing `{ selector, media, declarations }` per style rule. */
function walk(text, media, out) {
  let i = 0;
  while (i < text.length) {
    const open = text.indexOf("{", i);
    const semi = text.indexOf(";", i);
    if (open === -1 || (semi !== -1 && semi < open)) {
      // A statement such as `@layer a, b;` or a trailing declaration.
      i = semi === -1 ? text.length : semi + 1;
      continue;
    }
    let depth = 1;
    let j = open + 1;
    while (j < text.length && depth > 0) {
      if (text[j] === "{") {
        depth += 1;
      } else if (text[j] === "}") {
        depth -= 1;
      }
      j += 1;
    }
    const prelude = text.slice(i, open).trim();
    const body = text.slice(open + 1, j - 1);
    if (prelude.startsWith("@media")) {
      walk(body, prelude.slice("@media".length).trim(), out);
    } else if (prelude.startsWith("@layer") || prelude.startsWith("@supports")) {
      walk(body, media, out);
    } else if (!prelude.startsWith("@")) {
      for (const selector of splitTop(prelude, ",")) {
        out.push({ selector: normalizeSelector(selector), media: media === null ? null : squash(media), declarations: declarations(body) });
      }
    }
    i = j;
  }
}

/** Every style rule in the bundle, in source order. */
async function loadRules() {
  if (!rulesPromise) {
    rulesPromise = readFile(path.join(distDir, "app.css"), "utf8").then((css) => {
      const out = [];
      walk(css.replace(/\/\*[\s\S]*?\*\//g, ""), null, out);
      return out;
    });
  }
  return rulesPromise;
}

/**
 * The declarations the bundle gives `selector` (compared with whitespace and
 * quotes removed), merged in source order, under `media` (null for rules
 * outside any media block). Returns a Map; a missing property reads undefined.
 */
export async function bundledDeclarations(selector, { media = null } = {}) {
  const wantSelector = normalizeSelector(selector);
  const wantMedia = media === null ? null : squash(media);
  const merged = new Map();
  for (const rule of await loadRules()) {
    if (rule.selector === wantSelector && rule.media === wantMedia) {
      for (const [property, value] of rule.declarations) {
        merged.set(property, value);
      }
    }
  }
  return merged;
}

/** How many separate rules in the bundle target exactly `selector` outside media blocks. */
export async function bundledRuleCount(selector) {
  const wantSelector = normalizeSelector(selector);
  return (await loadRules()).filter((rule) => rule.selector === wantSelector && rule.media === null).length;
}
