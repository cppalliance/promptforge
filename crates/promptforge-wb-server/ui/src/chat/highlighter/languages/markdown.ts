import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerMarkupLanguage } from "./markup";
import { escapeRegExp, isRegisteredGrammar } from "./shared";
import { registerYamlLanguage } from "./yaml";

export function registerMarkdownLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.markdown;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	registerMarkupLanguage(registry);
	const yaml = registerYamlLanguage(registry);
	const markdown = registry.extend("markup", {});
	const inner = /(?:\\.|[^\\\n\r]|(?:\n|\r\n?)(?![\r\n]))/.source;
	const createInline = (source: string): RegExp =>
		RegExp(`${/((?:^|[^\\])(?:\\{2})*)/.source}(?:${source.replace(/<inner>/g, inner)})`);
	const tableCell = /(?:\\.|``(?:[^`\r\n]|`(?!`))+``|`[^`\r\n]+`|[^\\|\r\n`])+/.source;
	const tableRow = /\|?__(?:\|__)+\|?(?:(?:\n|\r\n?)|(?![\s\S]))/.source.replace(/__/g, tableCell);
	const tableLine = /\|?[ \t]*:?-{3,}:?[ \t]*(?:\|[ \t]*:?-{3,}:?[ \t]*)+\|?(?:\n|\r\n?)/.source;

	const fencedCodeBlocks = [
		...createMarkdownFencedCodePatterns(registry),
		{
			pattern: /^(```.*(?:\n|\r\n?))[\s\S]+?(?=(?:\n|\r\n?)^```$)/m,
			lookbehind: true,
		},
	];

	registry.markdown = markdown;
	registry.insertBefore("markdown", "prolog", {
		"front-matter-block": {
			pattern: /(^(?:\s*[\r\n])?)---(?!.)[\s\S]*?[\r\n]---(?!.)/,
			lookbehind: true,
			greedy: true,
			inside: {
				punctuation: /^---|---$/,
				"front-matter": {
					pattern: /\S+(?:\s+\S+)*/,
					alias: ["yaml", "language-yaml"],
					inside: yaml,
				},
			},
		},
		blockquote: {
			pattern: /^>(?:[\t ]*>)*/m,
			alias: "punctuation",
		},
		table: {
			pattern: RegExp(`^${tableRow}${tableLine}(?:${tableRow})*`, "m"),
			inside: {
				"table-data-rows": {
					pattern: RegExp(`^(${tableRow}${tableLine})(?:${tableRow})*$`),
					lookbehind: true,
					inside: {
						"table-data": {
							pattern: RegExp(tableCell),
							inside: markdown,
						},
						punctuation: /\|/,
					},
				},
				"table-line": {
					pattern: RegExp(`^(${tableRow})${tableLine}$`),
					lookbehind: true,
					inside: {
						punctuation: /\||:?-{3,}:?/,
					},
				},
				"table-header-row": {
					pattern: RegExp(`^${tableRow}$`),
					inside: {
						"table-header": {
							pattern: RegExp(tableCell),
							alias: "important",
							inside: markdown,
						},
						punctuation: /\|/,
					},
				},
			},
		},
		code: [
			{
				pattern: /((?:^|\n)[ \t]*\n|(?:^|\r\n?)[ \t]*\r\n?)(?: {4}|\t).+(?:(?:\n|\r\n?)(?: {4}|\t).+)*/,
				lookbehind: true,
				alias: "keyword",
			},
			{
				pattern: /^```[\s\S]*?^```$/m,
				greedy: true,
				inside: {
					"code-block": fencedCodeBlocks,
					"code-language": {
						pattern: /^(```).+/,
						lookbehind: true,
					},
					punctuation: /```/,
				},
			},
		],
		title: [
			{
				pattern: /\S.*(?:\n|\r\n?)(?:==+|--+)(?=[ \t]*$)/m,
				alias: "important",
				inside: {
					punctuation: /==+$|--+$/,
				},
			},
			{
				pattern: /(^\s*)#.+/m,
				lookbehind: true,
				alias: "important",
				inside: {
					punctuation: /^#+|#+$/,
				},
			},
		],
		hr: {
			pattern: /(^\s*)([*-])(?:[\t ]*\2){2,}(?=\s*$)/m,
			lookbehind: true,
			alias: "punctuation",
		},
		list: {
			pattern: /(^\s*)(?:[*+-]|\d+\.)(?=[\t ].)/m,
			lookbehind: true,
			alias: "punctuation",
		},
		"url-reference": {
			pattern:
				/!?\[[^\]]+\]:[\t ]+(?:\S+|<(?:\\.|[^>\\])+>)(?:[\t ]+(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\((?:\\.|[^)\\])*\)))?/,
			inside: {
				variable: {
					pattern: /^(!?\[)[^\]]+/,
					lookbehind: true,
				},
				string: /(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\((?:\\.|[^)\\])*\))$/,
				punctuation: /^[[\]!:]|[<>]/,
			},
			alias: "url",
		},
		bold: {
			pattern: createInline(
				/\b__(?:(?!_)<inner>|_(?:(?!_)<inner>)+_)+__\b|\*\*(?:(?!\*)<inner>|\*(?:(?!\*)<inner>)+\*)+\*\*/.source,
			),
			lookbehind: true,
			greedy: true,
			inside: {
				content: {
					pattern: /(^..)[\s\S]+(?=..$)/,
					lookbehind: true,
					inside: {},
				},
				punctuation: /\*\*|__/,
			},
		},
		italic: {
			pattern: createInline(
				/\b_(?:(?!_)<inner>|__(?:(?!_)<inner>)+__)+_\b|\*(?:(?!\*)<inner>|\*\*(?:(?!\*)<inner>)+\*\*)+\*/.source,
			),
			lookbehind: true,
			greedy: true,
			inside: {
				content: {
					pattern: /(^.)[\s\S]+(?=.$)/,
					lookbehind: true,
					inside: {},
				},
				punctuation: /[*_]/,
			},
		},
		strike: {
			pattern: createInline("(~~?)(?:(?!~)<inner>)+\\2"),
			lookbehind: true,
			greedy: true,
			inside: {
				content: {
					pattern: /(^~~?)[\s\S]+(?=\1$)/,
					lookbehind: true,
					inside: {},
				},
				punctuation: /~~?/,
			},
		},
		"code-snippet": {
			pattern: /(^|[^\\`])(?:``[^`\r\n]+(?:`[^`\r\n]+)*``(?!`)|`[^`\r\n]+`(?!`))/,
			lookbehind: true,
			greedy: true,
			alias: ["code", "keyword"],
		},
		url: {
			pattern: createInline(
				/!?\[(?:(?!\])<inner>)+\](?:\([^\s)]+(?:[\t ]+"(?:\\.|[^"\\])*")?\)|[ \t]?\[(?:(?!\])<inner>)+\])/.source,
			),
			lookbehind: true,
			greedy: true,
			inside: {
				operator: /^!/,
				content: {
					pattern: /(^\[)[^\]]+(?=\])/,
					lookbehind: true,
					inside: {},
				},
				variable: {
					pattern: /(^\][ \t]?\[)[^\]]+(?=\]$)/,
					lookbehind: true,
				},
				url: {
					pattern: /(^\]\()[^\s)]+/,
					lookbehind: true,
				},
				string: {
					pattern: /(^[ \t]+)"(?:\\.|[^"\\])*"(?=\)$)/,
					lookbehind: true,
				},
			},
		},
	});

	registry.md = registry.markdown;
	const recursiveTokens = ["url", "bold", "italic", "strike"] as const;
	const nestedTokens = ["url", "bold", "italic", "strike", "code-snippet"] as const;
	const registeredMarkdown = registry.markdown as Grammar;

	for (const token of recursiveTokens) {
		const tokenValue = registeredMarkdown[token] as GrammarToken;
		const content = (tokenValue.inside as Grammar).content as GrammarToken;
		const inside = content.inside as Grammar;

		for (const nestedToken of nestedTokens) {
			if (token !== nestedToken) {
				inside[nestedToken] = registeredMarkdown[nestedToken];
			}
		}
	}

	return registeredMarkdown;
}

function createMarkdownFencedCodePatterns(registry: LanguagesRegistry): GrammarToken[] {
	const languages = [
		"javascript",
		"js",
		"typescript",
		"ts",
		"jsx",
		"tsx",
		"json",
		"yaml",
		"yml",
		"css",
		"markup",
		"html",
		"xml",
		"svg",
		"bash",
		"sh",
		"shell",
		"python",
		"py",
		"diff",
		"sql",
	];
	const patterns: GrammarToken[] = [];

	for (const language of languages) {
		const grammar = registry[language];

		if (!isRegisteredGrammar(grammar)) {
			continue;
		}

		patterns.push({
			pattern: RegExp(
				`^(\`\`\`[^\\S\\r\\n]*${escapeRegExp(language)}(?=[\\t \\r\\n])[^\\r\\n]*(?:\\n|\\r\\n?))[\\s\\S]+?(?=(?:\\n|\\r\\n?)^\`\`\`$)`,
				"im",
			),
			lookbehind: true,
			alias: `language-${language}`,
			inside: grammar,
		});
	}

	return patterns;
}
