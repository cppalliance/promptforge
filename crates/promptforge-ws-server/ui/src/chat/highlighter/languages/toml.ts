import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerTomlLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.toml;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const key = /(?:[\w-]+|'[^'\n\r]*'|"(?:\\.|[^\\"\r\n])*")/.source;
	const insertKey = (pattern: string): string => pattern.replace(/__/g, key);
	const toml: Grammar = {
		comment: {
			pattern: /#.*/,
			greedy: true,
		},
		table: {
			pattern: RegExp(insertKey(/(^[\t ]*\[\s*(?:\[\s*)?)__(?:\s*\.\s*__)*(?=\s*\])/.source), "m"),
			lookbehind: true,
			greedy: true,
			alias: "class-name",
		},
		key: {
			pattern: RegExp(insertKey(/(^[\t ]*|[{,]\s*)__(?:\s*\.\s*__)*(?=\s*=)/.source), "m"),
			lookbehind: true,
			greedy: true,
			alias: "property",
		},
		string: {
			pattern: /"""(?:\\[\s\S]|[^\\])*?"""|'''[\s\S]*?'''|'[^'\n\r]*'|"(?:\\.|[^\\"\r\n])*"/,
			greedy: true,
		},
		date: [
			{
				pattern: /\b\d{4}-\d{2}-\d{2}(?:[T\s]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?)?\b/i,
				alias: "number",
			},
			{
				pattern: /\b\d{2}:\d{2}:\d{2}(?:\.\d+)?\b/,
				alias: "number",
			},
		],
		number:
			/(?:\b0(?:x[\da-zA-Z]+(?:_[\da-zA-Z]+)*|o[0-7]+(?:_[0-7]+)*|b[10]+(?:_[10]+)*))\b|[-+]?\b\d+(?:_\d+)*(?:\.\d+(?:_\d+)*)?(?:[eE][+-]?\d+(?:_\d+)*)?\b|[-+]?\b(?:inf|nan)\b/,
		boolean: /\b(?:false|true)\b/,
		punctuation: /[.,=[\]{}]/,
	};

	registry.toml = toml;
	return toml;
}
