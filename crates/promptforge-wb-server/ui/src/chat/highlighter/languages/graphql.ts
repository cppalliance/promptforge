import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerGraphqlLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.graphql;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const graphql: Grammar = {
		comment: /#.*/,
		description: {
			pattern: /(?:"""(?:[^"]|(?!""")")*"""|"(?:\\.|[^\\"\r\n])*")(?=\s*[a-z_])/i,
			greedy: true,
			alias: "string",
		},
		string: {
			pattern: /"""(?:[^"]|(?!""")")*"""|"(?:\\.|[^\\"\r\n])*"/,
			greedy: true,
		},
		number: /(?:\B-|\b)\d+(?:\.\d+)?(?:e[+-]?\d+)?\b/i,
		boolean: /\b(?:false|true)\b/,
		variable: /\$[a-z_]\w*/i,
		directive: {
			pattern: /@[a-z_]\w*/i,
			alias: "function",
		},
		"attr-name": {
			pattern: /\b[a-z_]\w*(?=\s*(?:\((?:[^()"]|"(?:\\.|[^\\"\r\n])*")*\))?:)/i,
			greedy: true,
		},
		"atom-input": {
			pattern: /\b[A-Z]\w*Input\b/,
			alias: "class-name",
		},
		scalar: /\b(?:Boolean|Float|ID|Int|String)\b/,
		constant: /\b[A-Z][A-Z_\d]*\b/,
		"class-name": {
			pattern: /(\b(?:enum|implements|interface|on|scalar|type|union)\s+|&\s*|:\s*|\[)[A-Z_]\w*/,
			lookbehind: true,
		},
		fragment: {
			pattern: /(\bfragment\s+|\.{3}\s*(?!on\b))[a-zA-Z_]\w*/,
			lookbehind: true,
			alias: "function",
		},
		"definition-mutation": {
			pattern: /(\bmutation\s+)[a-zA-Z_]\w*/,
			lookbehind: true,
			alias: "function",
		},
		"definition-query": {
			pattern: /(\bquery\s+)[a-zA-Z_]\w*/,
			lookbehind: true,
			alias: "function",
		},
		keyword:
			/\b(?:directive|enum|extend|fragment|implements|input|interface|mutation|on|query|repeatable|scalar|schema|subscription|type|union)\b/,
		operator: /[!=|&]|\.{3}/,
		"property-query": /\w+(?=\s*\()/,
		object: /\w+(?=\s*\{)/,
		punctuation: /[!(){}[\]:=,]/,
		property: /\w+/,
	};

	registry.graphql = graphql;
	registry.gql = registry.graphql;
	return graphql;
}
