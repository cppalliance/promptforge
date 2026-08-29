/**
 * Block-memoized streaming markdown renderer.
 *
 * Incoming message text is split into top-level blocks at blank-line
 * boundaries (code-fence aware). Completed blocks are rendered exactly once
 * and cached; only the still-growing tail block is re-parsed while
 * streaming. Unterminated markdown on the tail (open bold/italic, unclosed
 * code fence, incomplete table or link) is repaired before parsing so
 * partial constructs never flash broken markup. Every block still passes
 * through the renderSafeHTML sanitizer as the final pass.
 */
import { marked } from "marked";
import type { Highlighter } from "./utils/html";
import { renderSafeHTML } from "./utils/html";

/** One top-level markdown block: raw source text and whether it is finalized. */
export interface MarkdownBlock {
	readonly text: string;
	readonly complete: boolean;
}

interface Fence {
	readonly marker: string;
	readonly length: number;
}

const FENCE_RE = /^ {0,3}(`{3,}|~{3,})/;
const BLOCK_BOUNDARY_AT_END_RE = /\n[ \t]*\n[ \t]*$/;

function readFence(line: string): Fence | null {
	const match = FENCE_RE.exec(line);
	const marker = match?.[1];
	if (!marker) return null;
	return { marker: marker.charAt(0), length: marker.length };
}

/**
 * Splits markdown source into top-level blocks on blank-line boundaries.
 * Blank lines inside an open code fence never split. Every block except the
 * last is complete; the last block is the streaming tail unless the text
 * ends on a blank-line boundary.
 */
export function splitMarkdownBlocks(text: string): MarkdownBlock[] {
	if (text.trim() === "") return [];

	const lines = text.split("\n");
	const rawBlocks: string[] = [];
	let current: string[] = [];
	let fence: Fence | null = null;

	const flush = (): void => {
		if (current.length === 0) return;
		rawBlocks.push(current.join("\n"));
		current = [];
	};

	for (const line of lines) {
		const found = readFence(line);
		if (found) {
			if (fence === null) {
				fence = found;
			} else if (found.marker === fence.marker && found.length >= fence.length) {
				fence = null;
			}
		}
		if (fence === null && line.trim() === "") {
			flush();
		} else {
			current.push(line);
		}
	}
	flush();

	const endsAtBoundary = BLOCK_BOUNDARY_AT_END_RE.test(text);
	return rawBlocks.map((blockText, index) => ({
		text: blockText,
		complete: endsAtBoundary || index < rawBlocks.length - 1,
	}));
}

/**
 * Repairs unterminated markdown constructs on a streaming tail block so a
 * partial source never renders as broken markup. The healed text is a
 * rendering aid only; it is never written back to the message.
 */
export function repairStreamingMarkdown(text: string): string {
	const fenceHealed = healCodeFence(text);
	if (fenceHealed !== text) return fenceHealed; // Inside a fence the rest is literal.
	return healEmphasis(healLink(healTable(text)));
}

function healCodeFence(text: string): string {
	let fence: Fence | null = null;
	for (const line of text.split("\n")) {
		const found = readFence(line);
		if (!found) continue;
		if (fence === null) {
			fence = found;
		} else if (found.marker === fence.marker && found.length >= fence.length) {
			fence = null;
		}
	}
	if (fence === null) return text;
	const closing = fence.marker.repeat(fence.length);
	return text.endsWith("\n") ? `${text}${closing}\n` : `${text}\n${closing}\n`;
}

function countOccurrences(haystack: string, needle: string): number {
	let count = 0;
	let index = 0;
	for (;;) {
		index = haystack.indexOf(needle, index);
		if (index === -1) return count;
		count++;
		index += needle.length;
	}
}

function healEmphasis(text: string): string {
	// Escaped markers are literals and never open a span.
	const plain = text.replace(/\\[*_]/g, "");
	let out = text;
	if (countOccurrences(plain, "**") % 2 === 1) out += "**";
	if (countOccurrences(plain.replaceAll("**", ""), "*") % 2 === 1) out += "*";
	if (countOccurrences(plain, "__") % 2 === 1) out += "__";
	if (countOccurrences(plain.replaceAll("__", ""), "_") % 2 === 1) out += "_";
	return out;
}

function healLink(text: string): string {
	const openIndex = text.lastIndexOf("](");
	if (openIndex === -1) return text;
	if (text.slice(openIndex + 2).includes(")")) return text;
	if (text.lastIndexOf("[", openIndex) === -1) return text;
	// Close the destination so the anchor renders instead of raw syntax.
	return `${text})`;
}

const PARTIAL_DELIMITER_RE = /^[|\s:-]+$/;

function isDelimiterRow(line: string): boolean {
	return PARTIAL_DELIMITER_RE.test(line) && line.includes("-") && line.includes("|");
}

function columnCount(headerLine: string): number {
	const stripped = headerLine.trim().replace(/^\|/, "").replace(/\|$/, "");
	return Math.max(1, stripped.split("|").length);
}

function healTable(text: string): string {
	const lines = text.split("\n");
	if (lines.length < 2) return text;
	const lastIndex = lines.length - 1;
	const last = lines[lastIndex];
	const header = lines[lastIndex - 1];
	if (last === undefined || header === undefined) return text;
	// The tail line must look like a partial delimiter row beneath a header.
	if (!isDelimiterRow(last) || !header.includes("|") || isDelimiterRow(header)) return text;
	// A table that already has its delimiter row needs no healing.
	if (lines.slice(0, lastIndex - 1).some(isDelimiterRow)) return text;
	const delimiter = `| ${Array.from({ length: columnCount(header) }, () => "---").join(" | ")} |`;
	lines[lastIndex] = delimiter;
	return lines.join("\n");
}

interface RenderSegment {
	source: string;
	readonly el: HTMLDivElement;
}

/**
 * Renders markdown into a container one top-level block at a time, caching
 * rendered HTML per completed block so streaming updates re-parse only the
 * tail block. `parseCount` instruments the number of marked.parse calls for
 * tests.
 */
export class StreamingMarkdownRenderer {
	public parseCount = 0;
	private segments: RenderSegment[] = [];
	private renderSeq = 0;

	public constructor(
		private readonly container: HTMLElement,
		private readonly highlighter?: Highlighter,
	) {}

	/** Renders the text; pass finalize=true when the message is done streaming. */
	public async render(text: string, finalize: boolean): Promise<void> {
		const seq = ++this.renderSeq;
		const blocks = splitMarkdownBlocks(text);

		while (this.segments.length > blocks.length) {
			this.segments.pop()?.el.remove();
		}

		for (let i = 0; i < blocks.length; i++) {
			const block = blocks[i];
			if (!block) continue;
			const complete = finalize || block.complete;

			let segment = this.segments[i];
			if (!segment) {
				const segmentEl = document.createElement("div");
				segmentEl.className = "mur-md-segment";
				segment = { source: "", el: segmentEl };
				this.segments[i] = segment;
			}
			if (this.container.children[i] !== segment.el) {
				this.container.insertBefore(segment.el, this.container.children[i] ?? null);
			}

			// Memo key is the rendered source (post-repair), so a tail whose
			// healed text equals its raw text is not re-parsed on completion.
			const source = complete ? block.text : repairStreamingMarkdown(block.text);
			if (segment.source === source) continue;

			this.parseCount++;
			const html = await marked.parse(source);
			if (seq !== this.renderSeq) return;
			await renderSafeHTML(segment.el, html, this.highlighter);
			if (seq !== this.renderSeq) return;
			segment.source = source;
		}
	}

	/** Drops in-flight renders; the container is owned and removed by the caller. */
	public destroy(): void {
		this.renderSeq++;
		this.segments = [];
	}
}
