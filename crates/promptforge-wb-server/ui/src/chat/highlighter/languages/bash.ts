import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerBashLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.bash;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const entity = /\\(?:[abceEfnrtv\\"]|O?[0-7]{1,3}|U[0-9a-fA-F]{8}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{1,2})/;
	const environment = /\$(?:HOME|PATH|PWD|SHELL|TERM|USER)\b/;
	const commandSubstitution: GrammarToken = {
		pattern: /\$\((?:\([^)]+\)|[^()])+\)|`[^`]+`/,
		greedy: true,
		inside: {
			variable: /^\$\(|^`|\)$|`$/,
		},
	};
	const arithmetic: GrammarToken = {
		pattern: /\$?\(\([\s\S]+?\)\)/,
		greedy: true,
		inside: {
			variable: [
				{
					pattern: /(^\$\(\([\s\S]+)\)\)/,
					lookbehind: true,
				},
				/^\$\(\(/,
			],
			number: /\b0x[\dA-Fa-f]+\b|(?:\b\d+(?:\.\d*)?|\B\.\d+)(?:[Ee]-?\d+)?/,
			operator: /--|\+\+|\*\*=?|<<=?|>>=?|&&|\|\||[=!+\-*/%<>^&|]=?|[?~:]/,
			punctuation: /\(\(?|\)\)?|,|;/,
		},
	};
	const braceExpansion: GrammarToken = {
		pattern: /\$\{[^}]+\}/,
		greedy: true,
		inside: {
			operator: /:[-=?+]?|[!/]|##?|%%?|\^\^?|,,?/,
			punctuation: /[[\]]/,
		},
	};
	const variable = [arithmetic, commandSubstitution, braceExpansion, /\$(?:\w+|[#?*!@$])/];
	const commandAfterHeredoc: GrammarToken = {
		pattern: /(^(["']?)\w+\2)[ \t]+\S.*/,
		lookbehind: true,
		alias: "punctuation",
		inside: null,
	};
	const insideString: Grammar = {
		bash: commandAfterHeredoc,
		environment: {
			pattern: environment,
			alias: "constant",
		},
		variable,
		entity,
	};

	const bash: Grammar = {
		shebang: {
			pattern: /^#!\s*\/.*/,
			alias: "important",
		},
		comment: {
			pattern: /(^|[^"{\\$])#.*/,
			lookbehind: true,
			greedy: true,
		},
		string: [
			{
				pattern: /((?:^|[^<])<<-?\s*)(\w+)\s[\s\S]*?(?:\r?\n|\r)\2/,
				lookbehind: true,
				greedy: true,
				inside: insideString,
			},
			{
				pattern: /((?:^|[^<])<<-?\s*)(["'])(\w+)\2\s[\s\S]*?(?:\r?\n|\r)\3/,
				lookbehind: true,
				greedy: true,
				inside: {
					bash: commandAfterHeredoc,
				},
			},
			{
				pattern: /(^|[^\\](?:\\\\)*)"(?:\\[\s\S]|\$\([^)]+\)|\$(?!\()|`[^`]+`|[^"\\`$])*"/,
				lookbehind: true,
				greedy: true,
				inside: insideString,
			},
			{
				pattern: /(^|[^$\\])'[^']*'/,
				lookbehind: true,
				greedy: true,
			},
			{
				pattern: /\$'(?:[^'\\]|\\[\s\S])*'/,
				greedy: true,
				inside: {
					entity,
				},
			},
		],
		"function-name": [
			{
				pattern: /(\bfunction\s+)[\w-]+(?=(?:\s*\(?:\s*\))?\s*\{)/,
				lookbehind: true,
				alias: "function",
			},
			{
				pattern: /\b[\w-]+(?=\s*\(\s*\)\s*\{)/,
				alias: "function",
			},
		],
		"for-or-select": {
			pattern: /((?:^|[;&|]\s*|\b(?:do|then|else)\s+)for\s+)\w+/,
			lookbehind: true,
			alias: "variable",
		},
		"assign-left": {
			pattern: /(^|[\s;|&]|[<>]\()\w+(?:\.\w+)*(?=\+?=)/,
			lookbehind: true,
			alias: "variable",
			inside: {
				environment: {
					pattern: /(^|[\s;|&]|[<>]\()(?:HOME|PATH|PWD|SHELL|TERM|USER)\b/,
					lookbehind: true,
					alias: "constant",
				},
			},
		},
		environment: {
			pattern: environment,
			alias: "constant",
		},
		variable,
		parameter: {
			pattern: /(^|\s)-{1,2}[\w-]+/,
			lookbehind: true,
			alias: "variable",
		},
		function: {
			pattern:
				/(^|[\s;|&]|[<>]\()(?:basename|cat|cd|chmod|cp|curl|diff|docker|find|git|grep|ls|mkdir|mv|node|npm|pnpm|rm|sed|sh|sort|sudo|tail|tar|touch|yarn)(?=$|[)\s;|&])/,
			lookbehind: true,
		},
		keyword: {
			pattern:
				/(^|[\s;|&]|[<>]\()(?:case|do|done|elif|else|esac|fi|for|function|if|in|select|then|until|while)(?=$|[)\s;|&])/,
			lookbehind: true,
		},
		builtin: {
			pattern:
				/(^|[\s;|&]|[<>]\()(?:alias|break|cd|command|continue|declare|echo|eval|exec|exit|export|local|printf|pwd|read|return|set|shift|source|test|type|unset)(?=$|[)\s;|&])/,
			lookbehind: true,
			alias: "class-name",
		},
		boolean: {
			pattern: /(^|[\s;|&]|[<>]\()(?:false|true)(?=$|[)\s;|&])/,
			lookbehind: true,
		},
		operator: /\d?<>|>\||\+=|=[=~]?|!=?|<<[<-]?|[&\d]?>>|\d[<>]&?|[<>][&=]?|&[>&]?|\|[&|]?/,
		punctuation: /\$?\(\(?|\)\)?|\.\.|[{}[\];\\]/,
		number: {
			pattern: /(^|\s)(?:[1-9]\d*|0)(?:[.,]\d+)?\b/,
			lookbehind: true,
		},
	};
	const commandSubstitutionInside = commandSubstitution.inside;

	commandAfterHeredoc.inside = bash;

	if (commandSubstitutionInside) {
		for (const token of [
			"comment",
			"function-name",
			"for-or-select",
			"assign-left",
			"parameter",
			"string",
			"environment",
			"function",
			"keyword",
			"builtin",
			"boolean",
			"operator",
			"punctuation",
			"number",
		]) {
			commandSubstitutionInside[token] = bash[token];
		}
	}

	registry.bash = bash;
	registry.sh = registry.bash;
	registry.shell = registry.bash;
	return bash;
}
