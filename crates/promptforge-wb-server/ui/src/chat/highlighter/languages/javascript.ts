import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerClikeLanguage } from "./clike";
import { isRegisteredGrammar } from "./shared";

export function registerJavaScriptLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.javascript;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const clike = registerClikeLanguage(registry);

	const javascript = registry.extend("clike", {
		"class-name": [
			clike["class-name"] as GrammarToken,
			{
				pattern:
					/(^|[^$\w\xA0-\uFFFF])(?!\s)[_$A-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?=\.(?:constructor|prototype))/,
				lookbehind: true,
			},
		],
		keyword: [
			{
				pattern: /((?:^|\})\s*)catch\b/,
				lookbehind: true,
			},
			{
				pattern:
					/(^|[^.]|\.\.\.\s*)\b(?:as|assert(?=\s*\{)|async(?=\s*(?:function\b|\(|[$\w\xA0-\uFFFF]|$))|await|break|case|class|const|continue|debugger|default|delete|do|else|enum|export|extends|finally(?=\s*(?:\{|$))|for|from(?=\s*(?:['"]|$))|function|get(?=\s*(?:[#[$\w\xA0-\uFFFF]|$))|if|implements|import|in|instanceof|interface|let|new|null|of|package|private|protected|public|return|set(?=\s*(?:[#[$\w\xA0-\uFFFF]|$))|static|super|switch|this|throw|try|typeof|undefined|var|void|while|with|yield)\b/,
				lookbehind: true,
			},
		],
		function: /#?(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?=\s*(?:\.\s*(?:apply|bind|call)\s*)?\()/,
		number: {
			pattern:
				/(^|[^\w$])(?:NaN|Infinity|0[bB][01]+(?:_[01]+)*n?|0[oO][0-7]+(?:_[0-7]+)*n?|0[xX][\dA-Fa-f]+(?:_[\dA-Fa-f]+)*n?|\d+(?:_\d+)*n|(?:\d+(?:_\d+)*(?:\.(?:\d+(?:_\d+)*)?)?|\.\d+(?:_\d+)*)(?:[Ee][+-]?\d+(?:_\d+)*)?)(?![\w$])/,
			lookbehind: true,
		},
		operator: /--|\+\+|\*\*=?|=>|&&=?|\|\|=?|[!=]==|<<=?|>>>?=?|[-+*/%&|^!=<>]=?|\.{3}|\?\?=?|\?\.?|[~:]/,
	});

	(javascript["class-name"] as GrammarToken[])[0] = {
		pattern: /(\b(?:class|extends|implements|instanceof|interface|new)\s+)[\w.\\]+/,
		lookbehind: true,
		inside: { punctuation: /[.\\]/ },
	};

	registry.javascript = javascript;
	registry.insertBefore("javascript", "keyword", {
		regex: {
			pattern:
				/((?:^|[^$\w\xA0-\uFFFF."'`\])\s]|\b(?:return|yield))\s*)\/(?:\[(?:[^\]\\\r\n]|\\.)*\]|\\.|[^/\\[\r\n])+\/[dgimyus]{0,7}/,
			lookbehind: true,
			greedy: true,
			inside: {
				"regex-source": {
					pattern: /^(\/)[\s\S]+(?=\/[a-z]*$)/,
					lookbehind: true,
					alias: "language-regex",
				},
				"regex-delimiter": /^\/|\/$/,
				"regex-flags": /^[a-z]+$/,
			},
		},
		"function-variable": {
			pattern:
				/#?(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?=\s*[=:]\s*(?:async\s*)?(?:\bfunction\b|(?:\((?:[^()]|\([^()]*\))*\)|(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*)\s*=>))/,
			alias: "function",
		},
		parameter: [
			{
				pattern:
					/(function(?:\s+(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*)?\s*\(\s*)(?!\s)(?:[^()\s]|\s+(?![\s)])|\([^()]*\))+(?=\s*\))/,
				lookbehind: true,
				inside: javascript,
			},
			{
				pattern: /(^|[^$\w\xA0-\uFFFF])(?!\s)[_$a-z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?=\s*=>)/i,
				lookbehind: true,
				inside: javascript,
			},
			{
				pattern: /(\(\s*)(?!\s)(?:[^()\s]|\s+(?![\s)])|\([^()]*\))+(?=\s*\)\s*=>)/,
				lookbehind: true,
				inside: javascript,
			},
			{
				pattern:
					/((?:\b|\s|^)(?!(?:as|async|await|break|case|catch|class|const|continue|debugger|default|delete|do|else|enum|export|extends|finally|for|from|function|get|if|implements|import|in|instanceof|interface|let|new|null|of|package|private|protected|public|return|set|static|super|switch|this|throw|try|typeof|undefined|var|void|while|with|yield)(?![$\w\xA0-\uFFFF]))(?:(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*\s*)\(\s*|\]\s*\(\s*)(?!\s)(?:[^()\s]|\s+(?![\s)])|\([^()]*\))+(?=\s*\)\s*\{)/,
				lookbehind: true,
				inside: javascript,
			},
		],
		constant: /\b[A-Z](?:[A-Z_]|\dx?)*\b/,
	});
	registry.insertBefore("javascript", "string", {
		hashbang: {
			pattern: /^#!.*/,
			greedy: true,
			alias: "comment",
		},
		"template-string": {
			pattern: /`(?:\\[\s\S]|\$\{(?:[^{}]|\{(?:[^{}]|\{[^}]*\})*\})+\}|(?!\$\{)[^\\`])*`/,
			greedy: true,
			inside: {
				"template-punctuation": {
					pattern: /^`|`$/,
					alias: "string",
				},
				interpolation: {
					pattern: /((?:^|[^\\])(?:\\{2})*)\$\{(?:[^{}]|\{(?:[^{}]|\{[^}]*\})*\})+\}/,
					lookbehind: true,
					inside: {
						"interpolation-punctuation": {
							pattern: /^\$\{|\}$/,
							alias: "punctuation",
						},
						rest: registry.javascript as Grammar,
					},
				},
				string: /[\s\S]+/,
			},
		},
		"string-property": {
			pattern: /((?:^|[,{])[ \t]*)(["'])(?:\\(?:\r\n|[\s\S])|(?!\2)[^\\\r\n])*\2(?=\s*:)/m,
			lookbehind: true,
			greedy: true,
			alias: "property",
		},
	});
	registry.insertBefore("javascript", "operator", {
		"literal-property": {
			pattern: /((?:^|[,{])[ \t]*)(?!\s)[_$a-zA-Z\xA0-\uFFFF](?:(?!\s)[$\w\xA0-\uFFFF])*(?=\s*:)/m,
			lookbehind: true,
			alias: "property",
		},
	});
	registry.js = registry.javascript;
	return registry.javascript as Grammar;
}
