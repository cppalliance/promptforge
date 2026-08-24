// The vendored markdown-it 14.1.0 build is served as a classic script
// (/markdown-it.min.js) and registers `window.markdownit`.
interface MarkdownIt {
  render(text: string): string;
}

interface Window {
  markdownit(options: { breaks?: boolean; linkify?: boolean }): MarkdownIt;
}
