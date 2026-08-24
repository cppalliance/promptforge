import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerJsonLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.json;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const json: Grammar = {
		property: {
			pattern: /(^|[^\\])"(?:\\.|[^\\"\r\n])*"(?=\s*:)/,
			lookbehind: true,
			greedy: true,
		},
		string: {
			pattern: /(^|[^\\])"(?:\\.|[^\\"\r\n])*"(?!\s*:)/,
			lookbehind: true,
			greedy: true,
		},
		comment: {
			pattern: /\/\/.*|\/\*[\s\S]*?(?:\*\/|$)/,
			greedy: true,
		},
		number: /-?\b\d+(?:\.\d+)?(?:e[+-]?\d+)?\b/i,
		punctuation: /[{}[\],]/,
		operator: /:/,
		boolean: /\b(?:false|true)\b/,
		null: {
			pattern: /\bnull\b/,
			alias: "keyword",
		},
	};
	registry.json = json;
	return json;
}
