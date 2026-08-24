import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerClikeLanguage } from "./clike";
import { isRegisteredGrammar } from "./shared";

export function registerCLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.c;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	registerClikeLanguage(registry);

	const c = registry.extend("clike", {
		comment: {
			pattern: /\/\/(?:[^\r\n\\]|\\(?:\r\n?|\n|(?![\r\n])))*|\/\*[\s\S]*?(?:\*\/|$)/,
			greedy: true,
		},
		string: {
			pattern: /"(?:\\(?:\r\n|[\s\S])|[^"\\\r\n])*"/,
			greedy: true,
		},
		"class-name": {
			pattern: /(\b(?:enum|struct)\s+(?:__attribute__\s*\(\([\s\S]*?\)\)\s*)?)\w+|\b[a-z]\w*_t\b/,
			lookbehind: true,
		},
		keyword:
			/\b(?:_Alignas|_Alignof|_Atomic|_Bool|_Complex|_Generic|_Imaginary|_Noreturn|_Static_assert|_Thread_local|__attribute__|asm|auto|break|case|char|const|continue|default|do|double|else|enum|extern|float|for|goto|if|inline|int|long|register|return|short|signed|sizeof|static|struct|switch|typedef|typeof|union|unsigned|void|volatile|while)\b/,
		function: /\b[a-z_]\w*(?=\s*\()/i,
		number:
			/(?:\b0x(?:[\da-f]+(?:\.[\da-f]*)?|\.[\da-f]+)(?:p[+-]?\d+)?|(?:\b\d+(?:\.\d*)?|\B\.\d+)(?:e[+-]?\d+)?)[ful]{0,4}/i,
		operator: />>=?|<<=?|->|([-+&|:])\1|[?:~]|[-+*/%&|^!=<>]=?/,
	});

	registry.c = c;
	registry.insertBefore("c", "string", {
		char: {
			pattern: /'(?:\\(?:\r\n|[\s\S])|[^'\\\r\n]){0,32}'/,
			greedy: true,
		},
	});
	registry.insertBefore("c", "string", {
		macro: {
			pattern: /(^[\t ]*)#\s*[a-z](?:[^\r\n\\/]|\/(?!\*)|\/\*(?:[^*]|\*(?!\/))*\*\/|\\(?:\r\n|[\s\S]))*/im,
			lookbehind: true,
			greedy: true,
			alias: "property",
			inside: {
				string: [
					{
						pattern: /^(#\s*include\s*)<[^>]+>/,
						lookbehind: true,
					},
					c.string as GrammarToken,
				],
				char: c.char as GrammarToken,
				comment: c.comment as GrammarToken,
				"macro-name": [
					{
						pattern: /(^#\s*define\s+)\w+\b(?!\()/i,
						lookbehind: true,
					},
					{
						pattern: /(^#\s*define\s+)\w+\b(?=\()/i,
						lookbehind: true,
						alias: "function",
					},
				],
				directive: {
					pattern: /^(#\s*)[a-z]+/,
					lookbehind: true,
					alias: "keyword",
				},
				"directive-hash": /^#/,
				punctuation: /##|\\(?=[\r\n])/,
				expression: {
					pattern: /\S[\s\S]*/,
					inside: c,
				},
			},
		},
	});
	registry.insertBefore("c", "function", {
		constant:
			/\b(?:EOF|NULL|SEEK_CUR|SEEK_END|SEEK_SET|__DATE__|__FILE__|__LINE__|__TIMESTAMP__|__TIME__|__func__|stderr|stdin|stdout)\b/,
	});
	delete c.boolean;

	return c;
}
