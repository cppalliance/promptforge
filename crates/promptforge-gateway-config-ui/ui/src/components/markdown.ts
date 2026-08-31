// Sanitized markdown for untrusted hub READMEs. Hugging Face model
// cards rely on inline HTML for badges and layout, so markdown-it keeps
// HTML enabled and DOMPurify sanitizes the final rendered string.

import DOMPurify from "dompurify";
import matter from "gray-matter";
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

const FRONTMATTER_OPTIONS = {
  // The model-card metadata is discarded. A no-op parser prevents
  // gray-matter from interpreting attacker-controlled YAML values.
  engines: {
    yaml: (): object => ({}),
  },
};

/** An element on browsers that implement the native Sanitizer API sink. */
interface SanitizingElement extends HTMLElement {
  setHTML?(html: string): void;
}

/**
 * Removes a leading frontmatter document without allowing its language
 * suffix to select gray-matter's JavaScript eval engine.
 */
function stripFrontmatter(markdown: string): string {
  const source = markdown.startsWith("\uFEFF") ? markdown.slice(1) : markdown;
  const lineEnd = source.indexOf("\n");
  if (lineEnd < 0 || !source.startsWith("---") || source.startsWith("----")) {
    return source;
  }
  const canonical = `---\n${source.slice(lineEnd + 1)}`;
  return matter(canonical, FRONTMATTER_OPTIONS).content;
}

/**
 * Renders untrusted Markdown to sanitized HTML. gray-matter removes a
 * leading model-card frontmatter document without a regular expression.
 */
export function renderMarkdown(markdown: string): string {
  const rendered = parser.render(stripFrontmatter(markdown));
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
