import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerDiffLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.diff;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const diff: Grammar = {
		coord: [/^(?:\*{3}|-{3}|\+{3}).*$/m, /^@@.*@@$/m, /^\d.*$/m],
		"deleted-sign": createDiffLineToken("-", ["deleted"], "deleted"),
		"deleted-arrow": createDiffLineToken("<", ["deleted"], "deleted"),
		"inserted-sign": createDiffLineToken("+", ["inserted"], "inserted"),
		"inserted-arrow": createDiffLineToken(">", ["inserted"], "inserted"),
		unchanged: createDiffLineToken(" ", [], "unchanged"),
		diff: createDiffLineToken("!", ["bold"], "diff"),
	};

	registry.diff = diff;
	return diff;
}

function createDiffLineToken(prefix: string, alias: string[], prefixAlias: string): GrammarToken {
	return {
		pattern: RegExp(`^(?:[${prefix}].*(?:\\r\\n?|\\n|(?![\\s\\S])))+`, "m"),
		alias,
		inside: {
			line: {
				pattern: /(.)(?=[\s\S]).*(?:\r\n?|\n)?/,
				lookbehind: true,
			},
			prefix: {
				pattern: /[\s\S]/,
				alias: prefixAlias,
			},
		},
	};
}
