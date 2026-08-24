import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerPythonLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.python;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const python: Grammar = {
		comment: {
			pattern: /(^|[^\\])#.*/,
			lookbehind: true,
			greedy: true,
		},
		"string-interpolation": {
			pattern: /(?:f|fr|rf)(?:("""|''')[\s\S]*?\1|("|')(?:\\.|(?!\2)[^\\\r\n])*\2)/i,
			greedy: true,
			inside: {
				interpolation: {
					pattern: /((?:^|[^{])(?:\{\{)*)\{(?!\{)(?:[^{}]|\{(?!\{)(?:[^{}]|\{(?!\{)(?:[^{}])+\})+\})+\}/,
					lookbehind: true,
					inside: {
						"format-spec": {
							pattern: /(:)[^:(){}]+(?=\}$)/,
							lookbehind: true,
						},
						"conversion-option": {
							pattern: /![sra](?=[:}]$)/,
							alias: "punctuation",
						},
						punctuation: /^\{|\}$/,
					},
				},
				string: /[\s\S]+/,
			},
		},
		"triple-quoted-string": {
			pattern: /(?:[rub]|br|rb)?("""|''')[\s\S]*?\1/i,
			greedy: true,
			alias: "string",
		},
		string: {
			pattern: /(?:[rub]|br|rb)?("|')(?:\\.|(?!\1)[^\\\r\n])*\1/i,
			greedy: true,
		},
		function: {
			pattern: /((?:^|\s)def[ \t]+)[a-zA-Z_]\w*(?=\s*\()/,
			lookbehind: true,
		},
		"class-name": {
			pattern: /(\bclass\s+)\w+/i,
			lookbehind: true,
		},
		decorator: {
			pattern: /(^[\t ]*)@\w+(?:\.\w+)*/m,
			lookbehind: true,
			alias: ["annotation", "punctuation"],
			inside: {
				punctuation: /\./,
			},
		},
		keyword:
			/\b(?:and|as|assert|async|await|break|case|class|continue|def|del|elif|else|except|finally|for|from|global|if|import|in|is|lambda|match|nonlocal|not|or|pass|print|raise|return|try|while|with|yield)\b/,
		builtin:
			/\b(?:abs|all|any|bool|bytes|dict|enumerate|filter|float|format|input|int|isinstance|len|list|map|max|min|object|open|range|repr|reversed|round|set|str|sum|super|tuple|type|zip)\b/,
		boolean: /\b(?:False|None|True)\b/,
		number:
			/\b0(?:b(?:_?[01])+|o(?:_?[0-7])+|x(?:_?[a-f0-9])+)\b|(?:\b\d+(?:_\d+)*(?:\.(?:\d+(?:_\d+)*)?)?|\B\.\d+(?:_\d+)*)(?:e[+-]?\d+(?:_\d+)*)?j?(?!\w)/i,
		operator: /[-+%=]=?|!=|:=|\*\*?=?|\/\/?=?|<[<=>]?|>[=>]?|[&|^~]/,
		punctuation: /[{}[\];(),.:]/,
	};
	const interpolation = (python["string-interpolation"] as GrammarToken).inside?.interpolation;

	if (isRegisteredGrammar(interpolation) && isRegisteredGrammar(interpolation.inside)) {
		interpolation.inside.rest = python;
	}

	registry.python = python;
	registry.py = registry.python;
	return python;
}
