// Markdown rendering for model-authored chat content. marked parses to
// HTML with three custom renderers carried over from Cursor's markdown
// pipeline (image dimension suffixes, escaped links that are not
// draggable, inline-only paragraphs); fenced code blocks highlight
// through a Shiki core highlighter whose theme is built from the skin's
// --syntax-* token values; DOMPurify sanitizes the final string before it
// touches the DOM. Sanitizing here, at the render boundary, means no
// caller can bypass it: marked deliberately does not sanitize (it emits
// javascript: hrefs and passes raw <script> through) and its own
// sanitize option was removed for giving false confidence.
//
// Shiki init is async but renderMarkdown is sync, so the highlighter is
// created at module scope and readiness is exported as markdownReady for
// main.ts to await during boot. That latch is write-once initialization,
// not shared app state. Before readiness - or for a language Shiki does
// not know - code blocks degrade to unhighlighted <pre><code>.
//
// Streaming re-parses and re-sanitizes the whole buffer per delta: at
// chat message sizes that stays well under a frame budget, so there is
// no chunked renderer to keep consistent.

import "./markdown-render.css";

import DOMPurify from "dompurify";
import { Marked, type Tokens } from "marked";
import { createHighlighterCore, type HighlighterCore, type ThemeRegistration } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

import langBash from "@shikijs/langs/bash";
import langCss from "@shikijs/langs/css";
import langHtml from "@shikijs/langs/html";
import langJavascript from "@shikijs/langs/javascript";
import langJson from "@shikijs/langs/json";
import langLua from "@shikijs/langs/lua";
import langMarkdown from "@shikijs/langs/markdown";
import langPython from "@shikijs/langs/python";
import langRust from "@shikijs/langs/rust";
import langToml from "@shikijs/langs/toml";
import langTypescript from "@shikijs/langs/typescript";
import langYaml from "@shikijs/langs/yaml";

const THEME_NAME = "workshop-dark";

// The theme colors are the skin's --syntax-* token values (shared-ui/tokens.css).
// Shiki resolves token colors in JS at highlight time, where CSS custom
// properties cannot reach, so the values are duplicated here as a static
// theme; shared-ui/tokens.css stays the source of truth for what they should be.
const workshopTheme: ThemeRegistration = {
  name: THEME_NAME,
  type: "dark",
  fg: "#D6D6DD",
  bg: "#181818",
  settings: [
    { scope: ["keyword", "storage"], settings: { foreground: "#82D2CE" } },
    { scope: ["string"], settings: { foreground: "#E394DC" } },
    {
      scope: ["entity.name.function", "support.function", "meta.function-call"],
      settings: { foreground: "#EFB080" },
    },
    { scope: ["constant.numeric"], settings: { foreground: "#EBC88C" } },
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#E4E4E45E" } },
    {
      scope: ["constant.language", "variable.other.constant", "entity.name.constant"],
      settings: { foreground: "#F8C762" },
    },
    { scope: ["markup.underline.link", "string.other.link"], settings: { foreground: "#87C3FF" } },
  ],
};

let highlighter: HighlighterCore | undefined;

/**
 * Resolves when the Shiki highlighter is ready. main.ts awaits this
 * during boot, before mounting panels; until then, code blocks render
 * unhighlighted.
 */
export const markdownReady: Promise<void> = createHighlighterCore({
  themes: [workshopTheme],
  langs: [
    langBash,
    langCss,
    langHtml,
    langJavascript,
    langJson,
    langLua,
    langMarkdown,
    langPython,
    langRust,
    langToml,
    langTypescript,
    langYaml,
  ],
  engine: createJavaScriptRegexEngine(),
}).then((created) => {
  highlighter = created;
});

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** marked's default fenced-block shape, for the degraded path. */
function plainCodeBlock(code: string, lang: string): string {
  const languageClass = lang === "" ? "" : ` class="language-${escapeHtml(lang)}"`;
  return `<pre><code${languageClass}>${escapeHtml(code)}</code></pre>`;
}

/**
 * Highlights one fenced code block, returning its HTML. Exported for the
 * tool-call card, which highlights tool-call argument JSON outside a
 * markdown document. Falls back to an unhighlighted block before the
 * highlighter is ready, for an unknown language, or when a grammar
 * rejects the input (the JS regex engine refuses some Oniguruma-specific
 * constructs; a block that cannot highlight still renders as plain code).
 */
export function highlightCode(code: string, lang: string): string {
  const active = highlighter;
  if (active !== undefined && lang !== "" && active.getLoadedLanguages().includes(lang)) {
    try {
      return active.codeToHtml(code, { lang, theme: THEME_NAME });
    } catch {
      return plainCodeBlock(code, lang);
    }
  }
  return plainCodeBlock(code, lang);
}

/** Splits a Cursor-style ` =WxH` (or ` =Wx`) dimension suffix off an image href. */
function parseImageSource(href: string): {
  src: string;
  width: string | undefined;
  height: string | undefined;
} {
  const match = /\s+=(\d+)x(\d*)$/.exec(href);
  if (match === null) {
    return { src: href, width: undefined, height: undefined };
  }
  const width = match[1] ?? "";
  const height = match[2] ?? "";
  return {
    src: href.slice(0, match.index),
    width: width === "" ? undefined : width,
    height: height === "" ? undefined : height,
  };
}

// A private Marked instance carries the custom renderers, so the global
// marked instance is never mutated for a hypothetical other consumer.
const markedInstance = new Marked({
  renderer: {
    code({ text, lang }: Tokens.Code): string {
      return highlightCode(text, lang ?? "");
    },
    // Inline tokens only: a paragraph never holds block content.
    paragraph({ tokens }: Tokens.Paragraph): string {
      return `<p>${this.parser.parseInline(tokens)}</p>`;
    },
    // The href is escaped for the attribute and doubles as the title when
    // the source gives none. draggable="false" keeps a link drag out of
    // the window's workspace drop handlers.
    link({ href, title, tokens }: Tokens.Link): string {
      const text = this.parser.parseInline(tokens);
      return `<a href="${escapeHtml(href)}" title="${escapeHtml(title ?? href)}" draggable="false">${text}</a>`;
    },
    image({ href, title, text }: Tokens.Image): string {
      const { src, width, height } = parseImageSource(href);
      const titleAttribute = title === null ? "" : ` title="${escapeHtml(title)}"`;
      const widthAttribute = width === undefined ? "" : ` width="${width}"`;
      const heightAttribute = height === undefined ? "" : ` height="${height}"`;
      return `<img src="${escapeHtml(src)}" alt="${escapeHtml(text)}"${titleAttribute}${widthAttribute}${heightAttribute}>`;
    },
  },
});

/** Options for {@link renderMarkdown}. */
export interface RenderMarkdownOptions {
  /**
   * Hints that the text is a growing stream buffer. Accepted for callers
   * rendering deltas; rendering is a full re-parse and re-sanitize either
   * way, so the hint currently changes nothing.
   */
  readonly streaming?: boolean;
}

/**
 * Renders markdown text to a DocumentFragment whose single root element
 * carries the `markdown-content` class (the feed's caret selector and
 * containers target that class). Synchronous; the DOMPurify pass is the
 * last step, so the returned markup is safe to insert as-is.
 */
export function renderMarkdown(text: string, options?: RenderMarkdownOptions): DocumentFragment {
  void options;
  const dirty = markedInstance.parse(text, { async: false });
  const clean = DOMPurify.sanitize(dirty, {
    USE_PROFILES: { html: true },
    SANITIZE_NAMED_PROPS: true,
  });
  const template = document.createElement("template");
  template.innerHTML = clean;
  const root = document.createElement("div");
  root.className = "markdown-content";
  root.append(template.content);
  const fragment = document.createDocumentFragment();
  fragment.append(root);
  return fragment;
}
