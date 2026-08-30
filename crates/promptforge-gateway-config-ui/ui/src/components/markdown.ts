// Sanitized markdown for hub READMEs [Unsloth] model card rendering.
// marked does not sanitize, and a README fetched from the hub is
// untrusted input, so the renderer neutralizes every raw-HTML path:
// block and inline HTML tokens are escaped into visible text (a
// <script> in a README renders as its source, never as an element),
// and link/image URLs are dropped unless their scheme is http(s) or
// mailto (or they are relative), killing javascript:/data: vectors.
// What remains is exclusively markup marked itself generated.

import { Marked } from "marked";
import type { Tokens } from "marked";

/** Escapes text for literal inclusion in HTML. */
function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/**
 * Decodes the HTML character references a scheme can hide behind. The
 * browser decodes these in an href/src attribute before it interprets
 * the URL, so a raw-string scheme test that skips them lets
 * `&#106;avascript:` and `javascript&colon;` slip through as live
 * `javascript:` URLs.
 */
function decodeEntities(text: string): string {
  return text
    .replace(/&#x([0-9a-f]+);?/gi, (_, hex: string) => codePoint(parseInt(hex, 16)))
    .replace(/&#(\d+);?/g, (_, dec: string) => codePoint(parseInt(dec, 10)))
    .replace(/&colon;/gi, ":")
    .replace(/&newline;/gi, "\n")
    .replace(/&tab;/gi, "\t")
    .replace(/&sol;/gi, "/");
}

/** A code point as a string, or empty when it is out of range. */
function codePoint(value: number): string {
  return Number.isInteger(value) && value >= 0 && value <= 0x10ffff
    ? String.fromCodePoint(value)
    : "";
}

/** Whether a URL is relative or carries an allowed scheme. */
function isSafeUrl(href: string): boolean {
  // Decode entities and strip ASCII whitespace/control characters first,
  // mirroring what the browser does to an href before it reads the
  // scheme; otherwise a scheme concealed by `&#106;` or an embedded
  // newline reconstitutes into `javascript:` after this check passes.
  const normalized = decodeEntities(href).replace(/[\u0000-\u0020]/g, "");
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(normalized);
  if (!scheme) {
    return true;
  }
  return ["http", "https", "mailto"].includes(scheme[1]?.toLowerCase() ?? "");
}

const parser = new Marked({ gfm: true });

parser.use({
  renderer: {
    html({ text }: Tokens.HTML | Tokens.Tag): string {
      return escapeHtml(text);
    },
    link(token: Tokens.Link): string | false {
      if (isSafeUrl(token.href)) {
        // Falling back to the stock renderer keeps its URL encoding.
        return false;
      }
      return this.parser.parseInline(token.tokens);
    },
    image(token: Tokens.Image): string | false {
      if (isSafeUrl(token.href)) {
        return false;
      }
      return escapeHtml(token.text);
    },
  },
});

/**
 * Renders untrusted markdown to safe HTML: raw HTML escaped, unsafe
 * URLs stripped. A leading YAML frontmatter block (every hub README
 * starts with one) is removed before parsing.
 */
export function renderMarkdown(markdown: string): string {
  const body = markdown.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, "");
  // No async extensions are registered, so parse returns a string.
  return parser.parse(body) as string;
}
