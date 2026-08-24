import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerJavaScriptLanguage } from "./javascript";
import { isRegisteredGrammar } from "./shared";

export function registerTypeScriptLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.typescript;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const javascript = registerJavaScriptLanguage(registry);
	const javascriptKeywords = javascript.keyword;
	const typescript = registry.extend("javascript", {
		"class-name": {
			pattern:
				/(\b(?:class|extends|implements|instanceof|interface|new|type)\s+)(?!keyof\b)(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?:\s*<(?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*>)?/,
			lookbehind: true,
			greedy: true,
		},
		keyword: [
			...(Array.isArray(javascriptKeywords) ? javascriptKeywords : [javascriptKeywords as RegExp | GrammarToken]),
			/\b(?:abstract|declare|implements|interface|keyof|namespace|private|protected|public|readonly|type)\b/,
		],
		builtin:
			/\b(?:Array|Boolean|Function|Number|Promise|String|Symbol|any|bigint|boolean|never|number|object|string|unknown|void)\b/,
		parameter: undefined,
		"literal-property": undefined,
	});
	registry.typescript = typescript;
	const typeInside = registry.extend("typescript", {});
	delete typeInside["class-name"];
	(typescript["class-name"] as GrammarToken).inside = typeInside;
	registry.insertBefore("typescript", "function", {
		decorator: {
			pattern: /@[$\w\xA0-\uFFFF]+/,
			inside: {
				at: {
					pattern: /^@/,
					alias: "operator",
				},
				function: /^[\s\S]+/,
			},
		},
		"generic-function": {
			pattern: /#?(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*\s*<(?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*>(?=\s*\()/,
			greedy: true,
			inside: {
				function: /^#?(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*/,
				generic: {
					pattern: /<[\s\S]+/,
					alias: "class-name",
					inside: typeInside,
				},
			},
		},
	});
	registry.ts = registry.typescript;
	return registry.typescript as Grammar;
}
