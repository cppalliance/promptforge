// Sanitized markdown for untrusted hub READMEs. Hugging Face model
// cards rely on inline HTML for badges and layout, so markdown-it keeps
// HTML enabled and DOMPurify sanitizes the final rendered string.

import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";

const parser = new MarkdownIt({
  html: true,
  linkify: true,
});

/**
 * DOMPurify call-local policy. Keeping custom-element handling explicit
 * avoids the vulnerable default fallback in older DOMPurify releases.
 */
const SANITIZE_OPTIONS = {
  CUSTOM_ELEMENT_HANDLING: {
    tagNameCheck: null,
    attributeNameCheck: null,
  },
} as const;

/** An element on browsers that implement the native Sanitizer API sink. */
interface SanitizingElement extends HTMLElement {
  setHTML?(html: string): void;
}

/**
 * Removes a leading YAML frontmatter document (the `---` fenced block
 * at the start of HF model cards). Pure string operations with no YAML
 * parsing: the frontmatter content is discarded, not interpreted.
 */
function stripFrontmatter(markdown: string): string {
  const source = markdown.startsWith("\uFEFF") ? markdown.slice(1) : markdown;
  if (!source.startsWith("---") || source.startsWith("----")) {
    return source;
  }
  const afterOpener = source.indexOf("\n");
  if (afterOpener < 0) {
    return source;
  }
  const closerStart = source.indexOf("\n---", afterOpener);
  if (closerStart < 0) {
    return source;
  }
  const afterCloser = source.indexOf("\n", closerStart + 4);
  if (afterCloser < 0) {
    return source.slice(closerStart + 4).trimStart();
  }
  return source.slice(afterCloser + 1);
}

/**
 * Strips a leading "Model card" or "Model Card" H1 that HF's chrome
 * prepends to many model cards, duplicating the detail header.
 */
function stripChromeHeading(markdown: string): string {
  const trimmed = markdown.trimStart();
  if (/^#\s+model\s+card\s*$/im.test(trimmed.split("\n", 1)[0] ?? "")) {
    return trimmed.slice(trimmed.indexOf("\n") + 1);
  }
  return markdown;
}

/**
 * Renders untrusted Markdown to sanitized HTML. Frontmatter is stripped
 * and the redundant "Model card" heading HF prepends is removed.
 */
export function renderMarkdown(markdown: string): string {
  const rendered = parser.render(stripChromeHeading(stripFrontmatter(markdown)));
  return DOMPurify.sanitize(rendered, SANITIZE_OPTIONS);
}

/**
 * Inserts already-sanitized HTML through the native Sanitizer API when
 * available, with `innerHTML` as the compatibility sink.
 */
export function setSanitizedHtml(element: HTMLElement, html: string): void {
  const sanitizingElement = element as SanitizingElement;
  if (typeof sanitizingElement.setHTML === "function") {
    sanitizingElement.setHTML(html);
    return;
  }
  element.innerHTML = html;
}
