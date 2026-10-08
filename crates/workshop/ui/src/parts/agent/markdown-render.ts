// Markdown rendering for model-authored chat content. marked parses to
// HTML with three custom renderers taken from Cursor's markdown
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
// the agent directory's register() to log a failure against. That latch
// is write-once initialization, not shared app state. Before readiness -
// or for a language Shiki does not know - code blocks degrade to
// unhighlighted <pre><code>.
//
// Streaming re-parses and re-sanitizes the whole buffer per delta: at
// chat message sizes that stays well under a frame budget, so there is
// no chunked renderer to keep consistent. A MarkdownStream sits on top
// for a growing reply: it remembers what an earlier render painted and
// fades in only the text that is new (a caret is never drawn).
//
// Each fenced block is wrapped with a hover "Copy code" button that is
// added after sanitizing, so it is our markup, never the model's.

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

import { copyToClipboard, prefersReducedMotion } from "./transcript/motion";

const THEME_NAME = "workshop-dark";

// The theme colors are the skin's --syntax-* token values (@workshop/look/tokens.css).
// Shiki resolves token colors in JS at highlight time, where CSS custom
// properties cannot reach, so the values are duplicated here as a static
// theme; @workshop/look/tokens.css stays the source of truth for what they should be.
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
 * Resolves when the Shiki highlighter is ready. The agent directory's
 * register() logs a failure; until readiness, code blocks render
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
 * Highlights one fenced code block, returning its HTML. Exported so a
 * caller can highlight code outside a markdown document. Falls back to an
 * unhighlighted block before the highlighter is ready, for an unknown
 * language, or when a grammar
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

// A private Marked instance holds the custom renderers, so the global
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

/** How long "Copied" shows on a code block's button before it reads "Copy code" again. */
const COPIED_MS = 2000;

/**
 * Wraps each fenced block in a `ws-code-block` holder with a "Copy code"
 * button at its top-right, shown on hover (CSS). The button reads
 * "Copied" for a moment after a successful copy. Runs after sanitizing:
 * the wrapper and the button are this module's own markup.
 */
function addCopyButtons(root: HTMLElement): void {
  for (const pre of root.querySelectorAll("pre")) {
    const holder = document.createElement("div");
    holder.className = "ws-code-block";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ws-code-block__copy";
    button.textContent = "Copy code";
    button.addEventListener("click", () => {
      void copyToClipboard(pre.textContent ?? "").then((copied) => {
        if (!copied) {
          return;
        }
        button.textContent = "Copied";
        button.classList.add("ws-code-block__copy--copied");
        setTimeout(() => {
          button.textContent = "Copy code";
          button.classList.remove("ws-code-block__copy--copied");
        }, COPIED_MS);
      });
    });
    pre.replaceWith(holder);
    holder.append(pre, button);
  }
}

/**
 * Renders markdown text to a DocumentFragment whose single root element
 * has the `ws-markdown-content` class (the feed's styles target that
 * class). Synchronous; the DOMPurify pass runs before the copy buttons
 * are added, so the returned markup is safe to insert as-is.
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
  root.className = "ws-markdown-content";
  root.append(template.content);
  addCopyButtons(root);
  const fragment = document.createDocumentFragment();
  fragment.append(root);
  return fragment;
}

// --- Streaming fade ---------------------------------------------------------------

/** How long new text takes to fade in, and so how long a run counts as young. Keep in step with markdown-render.css. */
export const FADE_MS = 150;

/** The elements whose text never fades: code, images, and math draw their own way, and the copy button is chrome. */
const FADE_SKIPPED = new Set(["PRE", "SVG", "MATH", "BUTTON"]);

/** Text that arrived at one moment: a range of the rendered text and when it landed. */
interface Run {
  from: number;
  to: number;
  at: number;
}

// DOM node types, spelled out so the walk reads no browser global.
const ELEMENT_NODE = 1;
const TEXT_NODE = 3;

/** The text nodes under `root` that can fade, in document order. */
function fadeableTextNodes(root: Node, found: Text[] = []): Text[] {
  for (const child of root.childNodes) {
    if (child.nodeType === TEXT_NODE) {
      found.push(child as Text);
    } else if (
      child.nodeType === ELEMENT_NODE &&
      !FADE_SKIPPED.has((child as Element).tagName.toUpperCase())
    ) {
      fadeableTextNodes(child, found);
    }
  }
  return found;
}

/** The length of the common prefix of two strings. */
function commonPrefix(a: string, b: string): number {
  const limit = Math.min(a.length, b.length);
  let index = 0;
  while (index < limit && a.charCodeAt(index) === b.charCodeAt(index)) {
    index++;
  }
  return index;
}

/**
 * Replaces one text node's young stretches with spans that fade in. Each
 * word of a young stretch is its own `ws-fade-in` span (split at
 * whitespace), and its negative `animation-delay` is how far into the
 * fade the text already is, so the animation carries on across renders
 * instead of restarting.
 */
function wrapYoung(node: Text, start: number, runs: readonly Run[], now: number): void {
  const data = node.data;
  const end = start + data.length;
  const pieces: Array<string | HTMLSpanElement> = [];
  let cursor = start;
  for (const run of runs) {
    const from = Math.max(start, run.from, cursor);
    const to = Math.min(end, run.to);
    if (to <= from) {
      continue;
    }
    if (from > cursor) {
      pieces.push(data.slice(cursor - start, from - start));
    }
    const delay = `-${Math.max(0, now - run.at)}ms`;
    for (const token of data.slice(from - start, to - start).split(/(\s+)/)) {
      if (token === "") {
        continue;
      }
      if (/^\s+$/.test(token)) {
        pieces.push(token);
        continue;
      }
      const span = document.createElement("span");
      span.className = "ws-fade-in";
      span.style.animationDelay = delay;
      span.textContent = token;
      pieces.push(span);
    }
    cursor = to;
  }
  if (pieces.length === 0) {
    return;
  }
  if (cursor < end) {
    pieces.push(data.slice(cursor - start));
  }
  node.replaceWith(...pieces);
}

/** Options for {@link MarkdownStream.render}. */
export interface MarkdownStreamOptions {
  /** True while the text is still growing. A stream that has streamed keeps fading the tail after it settles. */
  readonly streaming: boolean;
  /** A millisecond clock reading; defaults to `performance.now()`. Tests pass exact times. */
  readonly now?: number;
}

/**
 * Renders one growing reply. Each render is a full re-parse (see the
 * header), but the stream remembers the rendered text of the render
 * before: whatever follows their common prefix is new, and only text
 * that is under {@link FADE_MS} old is wrapped to fade in - so a delta
 * fades its own words, earlier words stay put, and a delta landing mid-fade
 * carries the older words' fade on. Code, images, and math are never
 * wrapped. Under reduced motion nothing is wrapped. A reply that never
 * streamed (settled history) renders plain, with no fade.
 */
export class MarkdownStream {
  private previous = "";
  private runs: Run[] = [];
  private streamed = false;

  render(text: string, options: MarkdownStreamOptions): DocumentFragment {
    const fragment = renderMarkdown(text, { streaming: options.streaming });
    const root = fragment.firstElementChild;
    if (root === null) {
      return fragment;
    }
    const nodes = fadeableTextNodes(root);
    const rendered = nodes.map((node) => node.data).join("");
    const animate = (options.streaming || this.streamed) && !prefersReducedMotion();
    if (options.streaming) {
      this.streamed = true;
    }
    if (!animate) {
      this.previous = rendered;
      this.runs = [];
      return fragment;
    }

    const now = options.now ?? performance.now();
    const shared = commonPrefix(this.previous, rendered);
    this.previous = rendered;
    // Text past the shared prefix is new; runs that reached into the
    // part that changed are cut back to it.
    const runs: Run[] = [];
    for (const run of this.runs) {
      const to = Math.min(run.to, shared);
      if (to > run.from && now - run.at < FADE_MS) {
        runs.push({ from: run.from, to, at: run.at });
      }
    }
    if (rendered.length > shared) {
      runs.push({ from: shared, to: rendered.length, at: now });
    }
    this.runs = runs;

    let offset = 0;
    for (const node of nodes) {
      const length = node.data.length;
      if (runs.some((run) => run.from < offset + length && run.to > offset)) {
        wrapYoung(node, offset, runs, now);
      }
      offset += length;
    }
    return fragment;
  }
}
