// Prism-compatible tokenizer core. See THIRD_PARTY_NOTICES.md for attribution.

export type TokenStream = Array<string | Token>;

export interface Grammar {
	[token: string]: GrammarValue | Grammar | undefined;
	rest?: Grammar;
}

export type GrammarValue = RegExp | GrammarToken | Array<RegExp | GrammarToken>;

export interface GrammarToken {
	pattern: RegExp;
	lookbehind?: boolean;
	greedy?: boolean;
	alias?: string | string[];
	inside?: Grammar | null;
}

export interface LanguageDefinition {
	id: string;
	grammar: Grammar;
	aliases?: string[];
}

export type LanguageCollection = Array<LanguageDefinition> | Record<string, unknown>;

export interface CreateHighlighterOptions {
	languages?: LanguageCollection;
}

export interface Highlighter {
	readonly languages: LanguagesRegistry;
	registerLanguage: (language: string | LanguageDefinition, grammar?: Grammar) => void;
	highlight: (code: string, language: string) => string;
	highlightWithGrammar: (code: string, grammar: Grammar, language?: string) => string;
	tokenize: (text: string, grammar: Grammar) => TokenStream;
}

interface LinkedListNode<T> {
	value: T;
	prev: LinkedListNode<T> | null;
	next: LinkedListNode<T> | null;
}

interface LinkedList<T> {
	head: LinkedListNode<T>;
	tail: LinkedListNode<T>;
	length: number;
}

interface RescanState {
	skipPattern: string;
	maxReach: number;
}

export class Token {
	readonly type: string;
	readonly content: string | TokenStream;
	readonly alias?: string | string[];
	readonly length: number;

	constructor(type: string, content: string | TokenStream, alias?: string | string[], matchedText = "") {
		this.type = type;
		this.content = content;
		this.alias = alias;
		this.length = matchedText.length | 0;
	}
}

export interface LanguagesRegistry {
	[language: string]: Grammar | LanguagesRegistry[keyof LanguageHelpers] | undefined;
	extend: (id: string, redef: Grammar) => Grammar;
	insertBefore: (inside: string, before: string, insert: Grammar, root?: Record<string, unknown>) => Grammar;
}

interface LanguageHelpers {
	extend: LanguagesRegistry["extend"];
	insertBefore: LanguagesRegistry["insertBefore"];
}

const plainTextGrammar: Grammar = {};
const globalPatternCache = new WeakMap<RegExp, RegExp>();
const htmlEscapePattern = /[&<\u00a0]/g;

export function createLanguagesRegistry(): LanguagesRegistry {
	const registry: LanguagesRegistry = {
		plain: plainTextGrammar,
		plaintext: plainTextGrammar,
		text: plainTextGrammar,
		txt: plainTextGrammar,

		extend(id: string, redef: Grammar): Grammar {
			const base = registry[id];

			if (!isGrammar(base)) {
				throw new Error(`Cannot extend missing language "${id}".`);
			}

			const grammar = cloneGrammar(base);

			for (const key of Object.keys(redef)) {
				grammar[key] = redef[key];
			}

			return grammar;
		},

		insertBefore(inside: string, before: string, insert: Grammar, root: Record<string, unknown> = registry): Grammar {
			const grammar = root[inside];

			if (!isGrammar(grammar)) {
				throw new Error(`Cannot insert into missing grammar "${inside}".`);
			}

			const replacement: Grammar = {};

			for (const token of Object.keys(grammar)) {
				if (token === before) {
					for (const newToken of Object.keys(insert)) {
						replacement[newToken] = insert[newToken];
					}
				}

				// biome-ignore lint/suspicious/noPrototypeBuiltins: Object.hasOwn is ES2022, but core targets ES2018.
				if (!Object.prototype.hasOwnProperty.call(insert, token)) {
					replacement[token] = grammar[token];
				}
			}

			const oldGrammar = grammar;
			root[inside] = replacement;
			replaceGrammarReferences(registry, oldGrammar, replacement);

			return replacement;
		},
	};

	return registry;
}

export const languages: LanguagesRegistry = createLanguagesRegistry();

export function registerLanguage(language: string, grammar: Grammar): void {
	registerLanguageInRegistry(languages, language, grammar);
}

export function highlight(code: string, language: string): string {
	return highlightFromRegistry(languages, code, language);
}

export function createHighlighter(options: CreateHighlighterOptions = {}): Highlighter {
	const registry = createLanguagesRegistry();
	const highlighter: Highlighter = {
		languages: registry,
		registerLanguage(language: string | LanguageDefinition, grammar?: Grammar): void {
			if (typeof language === "string") {
				if (!grammar) {
					throw new Error(`Missing grammar for language "${language}".`);
				}

				registerLanguageInRegistry(registry, language, grammar);
				return;
			}

			registerLanguageDefinition(registry, language);
		},
		highlight(code: string, language: string): string {
			return highlightFromRegistry(registry, code, language);
		},
		highlightWithGrammar,
		tokenize,
	};

	registerLanguageCollection(registry, options.languages);

	return highlighter;
}

function highlightFromRegistry(registry: LanguagesRegistry, code: string, language: string): string {
	const grammar = registry[language] ?? registry[language.toLowerCase()];

	if (!isGrammar(grammar) || grammar === plainTextGrammar) {
		return escapeHtml(code);
	}

	return highlightWithGrammar(code, grammar, language);
}

function registerLanguageCollection(
	registry: LanguagesRegistry,
	collection: CreateHighlighterOptions["languages"],
): void {
	if (!collection) {
		return;
	}

	if (Array.isArray(collection)) {
		for (const definition of collection) {
			registerLanguageDefinition(registry, definition);
		}

		return;
	}

	for (const [language, grammar] of Object.entries(collection)) {
		if (isGrammar(grammar)) {
			registerLanguageInRegistry(registry, language, grammar);
		}
	}
}

function registerLanguageDefinition(registry: LanguagesRegistry, definition: LanguageDefinition): void {
	const grammar = cloneGrammar(definition.grammar);
	registerLanguageInRegistry(registry, definition.id, grammar);

	for (const alias of definition.aliases ?? []) {
		registerLanguageInRegistry(registry, alias, grammar);
	}
}

function registerLanguageInRegistry(registry: LanguagesRegistry, language: string, grammar: Grammar): void {
	registry[language] = grammar;
}

export function highlightWithGrammar(code: string, grammar: Grammar, language = ""): string {
	return renderHtml(tokenize(code, grammar), language);
}

export function tokenize(text: string, grammar: Grammar): TokenStream {
	const rest = grammar.rest;

	if (rest) {
		for (const token of Object.keys(rest)) {
			grammar[token] = rest[token];
		}

		delete grammar.rest;
	}

	const tokenList = createLinkedList<string | Token>();
	insertAfter(tokenList, tokenList.head, text);
	tokenizeInto(text, tokenList, grammar, tokenList.head, 0);

	return listValues(tokenList);
}

function renderHtml(value: string | Token | TokenStream, language: string): string {
	if (typeof value === "string") {
		return escapeHtml(value);
	}

	if (Array.isArray(value)) {
		let html = "";

		for (const item of value) {
			html += renderHtml(item, language);
		}

		return html;
	}

	const classes = ["token", value.type];
	const aliases = value.alias;

	if (Array.isArray(aliases)) {
		classes.push(...aliases);
	} else if (aliases) {
		classes.push(aliases);
	}

	const content = renderHtml(value.content, language);
	const title = value.type === "entity" ? ` title="${content.replace(/&amp;/, "&")}"` : "";

	return `<span class="${classes.join(" ")}"${title}>${content}</span>`;
}

function execPatternAt(pattern: RegExp, position: number, text: string, lookbehind: boolean): RegExpExecArray | null {
	pattern.lastIndex = position;
	const match = pattern.exec(text);

	if (match && lookbehind && match[1]) {
		const lookbehindLength = match[1].length;
		match.index += lookbehindLength;
		match[0] = match[0].slice(lookbehindLength);
	}

	return match;
}

function tokenizeInto(
	text: string,
	tokenList: LinkedList<string | Token>,
	grammar: Grammar,
	startNode: LinkedListNode<string | Token>,
	startPosition: number,
	rescan?: RescanState,
): void {
	for (const tokenType of Object.keys(grammar)) {
		if (tokenType === "rest") {
			continue;
		}

		const grammarValue = grammar[tokenType];

		if (!isPatternEntry(grammarValue)) {
			continue;
		}

		const tokenPatterns = Array.isArray(grammarValue) ? grammarValue : [grammarValue];

		for (let patternIndex = 0; patternIndex < tokenPatterns.length; patternIndex += 1) {
			if (rescan && rescan.skipPattern === `${tokenType},${patternIndex}`) {
				return;
			}

			const tokenPattern = toGrammarToken(tokenPatterns[patternIndex]);
			const nestedGrammar = tokenPattern.inside ?? null;
			const lookbehind = !!tokenPattern.lookbehind;
			const greedy = !!tokenPattern.greedy;
			const alias = tokenPattern.alias;
			const pattern = greedy ? asGlobalPattern(tokenPattern.pattern) : tokenPattern.pattern;

			for (
				let node = startNode.next, segmentStart = startPosition;
				node && node !== tokenList.tail;
				segmentStart += sourceLength(node.value), node = node.next
			) {
				if (rescan && segmentStart >= rescan.maxReach) {
					break;
				}

				let segment = node.value;

				if (tokenList.length > text.length) {
					return;
				}

				if (segment instanceof Token) {
					continue;
				}

				let replaceCount = 1;
				let match: RegExpExecArray | null;

				if (greedy) {
					match = execPatternAt(pattern, segmentStart, text, lookbehind);

					if (!match || match.index >= text.length) {
						break;
					}

					const matchStart = match.index;
					const matchEnd = match.index + match[0].length;
					let scanEnd = segmentStart + segment.length;

					while (matchStart >= scanEnd) {
						node = node.next;

						if (!node) {
							break;
						}

						segment = node.value;
						scanEnd += sourceLength(segment);
					}

					if (!node) {
						break;
					}

					scanEnd -= sourceLength(segment);
					segmentStart = scanEnd;

					if (segment instanceof Token) {
						continue;
					}

					for (let scanNode = node; scanNode !== tokenList.tail; ) {
						if (scanEnd >= matchEnd && typeof scanNode.value !== "string") {
							break;
						}

						replaceCount += 1;
						scanEnd += sourceLength(scanNode.value);
						scanNode = scanNode.next ?? tokenList.tail;
					}

					replaceCount -= 1;
					segment = text.slice(segmentStart, scanEnd);
					match.index -= segmentStart;
				} else {
					match = execPatternAt(pattern, 0, segment, lookbehind);

					if (!match) {
						continue;
					}
				}

				const matchStart = match.index;
				const matchedText = match[0];
				const prefix = segment.slice(0, matchStart);
				const suffix = segment.slice(matchStart + matchedText.length);
				const rescanReach = segmentStart + segment.length;

				if (rescan && rescanReach > rescan.maxReach) {
					rescan.maxReach = rescanReach;
				}

				let beforeMatchNode = node.prev;

				if (!beforeMatchNode) {
					continue;
				}

				if (prefix) {
					beforeMatchNode = insertAfter(tokenList, beforeMatchNode, prefix);
					segmentStart += prefix.length;
				}

				removeAfter(tokenList, beforeMatchNode, replaceCount);

				const wrapped = new Token(
					tokenType,
					nestedGrammar ? tokenize(matchedText, nestedGrammar) : matchedText,
					alias,
					matchedText,
				);
				node = insertAfter(tokenList, beforeMatchNode, wrapped);

				if (suffix) {
					insertAfter(tokenList, node, suffix);
				}

				if (replaceCount > 1) {
					const overlapRescan = {
						skipPattern: `${tokenType},${patternIndex}`,
						maxReach: rescanReach,
					};
					tokenizeInto(text, tokenList, grammar, node.prev ?? tokenList.head, segmentStart, overlapRescan);

					if (rescan && overlapRescan.maxReach > rescan.maxReach) {
						rescan.maxReach = overlapRescan.maxReach;
					}
				}
			}
		}
	}
}

function toGrammarToken(pattern: RegExp | GrammarToken): GrammarToken {
	if (pattern instanceof RegExp) {
		return { pattern };
	}

	return pattern;
}

function isPatternEntry(value: GrammarValue | Grammar | undefined): value is GrammarValue {
	if (!value) {
		return false;
	}

	if (value instanceof RegExp || Array.isArray(value)) {
		return true;
	}

	return value.pattern instanceof RegExp;
}

function asGlobalPattern(pattern: RegExp): RegExp {
	if (pattern.global) {
		return pattern;
	}

	let globalPattern = globalPatternCache.get(pattern);

	if (!globalPattern) {
		globalPattern = new RegExp(pattern.source, `${pattern.flags}g`);
		globalPatternCache.set(pattern, globalPattern);
	}

	return globalPattern;
}

function createLinkedList<T>(): LinkedList<T> {
	const head: LinkedListNode<T> = { value: null as T, prev: null, next: null };
	const tail: LinkedListNode<T> = { value: null as T, prev: head, next: null };
	head.next = tail;

	return { head, tail, length: 0 };
}

function insertAfter<T>(list: LinkedList<T>, node: LinkedListNode<T>, value: T): LinkedListNode<T> {
	const next = node.next;

	if (!next) {
		throw new Error("Cannot insert after a detached linked-list node.");
	}

	const newNode = { value, prev: node, next };
	node.next = newNode;
	next.prev = newNode;
	list.length += 1;

	return newNode;
}

function removeAfter<T>(list: LinkedList<T>, node: LinkedListNode<T>, count: number): void {
	let next = node.next;
	let removed = 0;

	for (; removed < count && next !== list.tail; removed += 1) {
		next = next?.next ?? null;
	}

	if (!next) {
		throw new Error("Cannot remove past the end of a linked list.");
	}

	node.next = next;
	next.prev = node;
	list.length -= removed;
}

function listValues<T>(list: LinkedList<T>): T[] {
	const array: T[] = [];
	let node = list.head.next;

	while (node && node !== list.tail) {
		array.push(node.value);
		node = node.next;
	}

	return array;
}

function sourceLength(value: string | Token): number {
	return value.length;
}

function escapeHtml(value: string): string {
	return value.replace(htmlEscapePattern, replaceHtmlCharacter);
}

function replaceHtmlCharacter(value: string): string {
	return value === "&" ? "&amp;" : value === "<" ? "&lt;" : " ";
}

function isGrammar(value: unknown): value is Grammar {
	return !!value && typeof value === "object" && !Array.isArray(value) && !(value instanceof RegExp);
}

function cloneGrammar<T>(value: T, visited = new Map<object, unknown>()): T {
	if (!value || typeof value !== "object") {
		return value;
	}

	if (value instanceof RegExp) {
		return value as T;
	}

	if (visited.has(value)) {
		return visited.get(value) as T;
	}

	if (Array.isArray(value)) {
		const array: unknown[] = [];
		visited.set(value, array);

		for (const item of value) {
			array.push(cloneGrammar(item, visited));
		}

		return array as T;
	}

	const object: Record<string, unknown> = {};
	visited.set(value, object);

	for (const key of Object.keys(value)) {
		object[key] = cloneGrammar((value as Record<string, unknown>)[key], visited);
	}

	return object as T;
}

function replaceGrammarReferences(
	value: unknown,
	oldGrammar: Grammar,
	replacement: Grammar,
	visited = new Set<object>(),
): void {
	if (!value || typeof value !== "object" || value instanceof RegExp || visited.has(value)) {
		return;
	}

	visited.add(value);

	const object = value as Record<string, unknown>;

	for (const key of Object.keys(object)) {
		if (object[key] === oldGrammar) {
			object[key] = replacement;
		} else {
			replaceGrammarReferences(object[key], oldGrammar, replacement, visited);
		}
	}
}
