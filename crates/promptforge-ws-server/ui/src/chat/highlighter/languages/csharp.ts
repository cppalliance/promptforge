import type { Grammar, LanguagesRegistry } from "../core";
import { registerClikeLanguage } from "./clike";
import { isRegisteredGrammar } from "./shared";

export function registerCSharpLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.csharp;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	registerClikeLanguage(registry);

	const name = /@?\b[A-Za-z_]\w*\b/.source;
	const keywords =
		/\b(?:abstract|add|alias|and|ascending|as|async|await|base|bool|break|byte|by|case|catch|char|checked|class|const|continue|decimal|default|delegate|descending|do|double|dynamic|else|enum|event|explicit|extern|false|finally|fixed|float|for|foreach|from(?=\s*(?:\w|$))|get|global|goto|group|if|implicit|in|init(?=\s*;)|int|interface|internal|into|is|join|let|lock|long|namespace|new|null|nameof|not|notnull|object|on|operator|or|orderby|out|override|params|partial|private|protected|public|readonly|record|ref|remove|return|sbyte|sealed|select|set|short|sizeof|stackalloc|static|string|struct|switch|this|throw|true|try|typeof|uint|ulong|unchecked|unmanaged|unsafe|ushort|using|value|var|virtual|void|volatile|when|where|while|with(?=\s*{)|yield)\b/;

	const csharp = registry.extend("clike", {
		string: [
			{
				pattern: /(^|[^$\\])@"(?:""|\\[\s\S]|[^\\"])*"(?!")/,
				lookbehind: true,
				greedy: true,
			},
			{
				pattern: /(^|[^@$\\])"(?:\\.|[^\\"\r\n])*"/,
				lookbehind: true,
				greedy: true,
			},
		],
		"class-name": [
			{
				pattern:
					/(\b(?:class|enum|interface|record|struct)\s+)@?\b[A-Za-z_]\w*\b(?:\s*<(?:[^<>;=+\-*/%&|^]|<(?:[^<>;=+\-*/%&|^]|<[^<>]*>)*>)*>)?/,
				lookbehind: true,
				inside: {
					keyword: keywords,
					punctuation: /[<>()?,.:[\]]/,
				},
			},
			{
				pattern:
					/\b(?:bool|byte|char|decimal|double|dynamic|float|int|long|object|sbyte|short|string|uint|ulong|ushort|var|void|[A-Z]\w*(?:\s*\.\s*[A-Z]\w*)*)(?=\s+(?!with\s*\{)@?\b[A-Za-z_]\w*\b(?:\s*[=,;:{)\]]|\s+(?:in|when)\b))/,
				inside: {
					keyword: keywords,
					punctuation: /[<>()?,.:[\]]/,
				},
			},
			{
				pattern: /(\bcatch\s*\(\s*)@?\b[A-Za-z_]\w*\b/,
				lookbehind: true,
			},
			{
				pattern: /(\bnew\s+)@?\b[A-Za-z_]\w*(?:\s*\.\s*@?\b[A-Za-z_]\w*)*(?=\s*[[({])/,
				lookbehind: true,
				inside: {
					punctuation: /\./,
				},
			},
		],
		keyword: keywords,
		number:
			/(?:\b0(?:x[\da-f_]*[\da-f]|b[01_]*[01])|(?:\B\.\d+(?:_+\d+)*|\b\d+(?:_+\d+)*(?:\.\d+(?:_+\d+)*)?)(?:e[-+]?\d+(?:_+\d+)*)?)(?:[dflmu]|lu|ul)?\b/i,
		operator: />>=?|<<=?|[-=]>|([-+&|])\1|~|\?\?=?|[-+*/%&|^!=<>]=?/,
		punctuation: /\?\.?|::|[{}[\];(),.:]/,
	});

	registry.csharp = csharp;
	registry.insertBefore("csharp", "number", {
		range: {
			pattern: /\.\./,
			alias: "operator",
		},
	});
	registry.insertBefore("csharp", "punctuation", {
		"named-parameter": {
			pattern: RegExp(/([(,]\s*)/.source + name + /(?=\s*:)/.source),
			lookbehind: true,
			alias: "punctuation",
		},
	});
	registry.insertBefore("csharp", "class-name", {
		namespace: {
			pattern: RegExp(`${/(\b(?:namespace|using)\s+)/.source}${name}(?:\\s*\\.\\s*${name})*(?=\\s*[;{])`),
			lookbehind: true,
			inside: {
				punctuation: /\./,
			},
		},
		preprocessor: {
			pattern: /(^[\t ]*)#.*/m,
			lookbehind: true,
			alias: "property",
			inside: {
				directive: {
					pattern: /(#)\b(?:define|elif|else|endif|endregion|error|if|line|nullable|pragma|region|undef|warning)\b/,
					lookbehind: true,
					alias: "keyword",
				},
			},
		},
		"constructor-invocation": {
			pattern: /(\bnew\s+)@?\b[A-Za-z_]\w*(?:\s*\.\s*@?\b[A-Za-z_]\w*)*(?=\s*[[({])/,
			lookbehind: true,
			inside: {
				punctuation: /\./,
			},
			alias: "class-name",
		},
		attribute: {
			pattern:
				/((?:^|[^\s\w>)?])\s*\[\s*)(?:(?:assembly|event|field|method|module|param|property|return|type)\s*:\s*)?@?\b[A-Za-z_]\w*(?:\s*\.\s*@?\b[A-Za-z_]\w*)*(?:\s*\([^()\r\n]*\))?(?:\s*,\s*@?\b[A-Za-z_]\w*(?:\s*\.\s*@?\b[A-Za-z_]\w*)*(?:\s*\([^()\r\n]*\))?)*(?=\s*\])/,
			lookbehind: true,
			greedy: true,
			inside: {
				target: {
					pattern: /^(?:assembly|event|field|method|module|param|property|return|type)(?=\s*:)/,
					alias: "keyword",
				},
				"class-name": {
					pattern: /@?\b[A-Za-z_]\w*(?:\s*\.\s*@?\b[A-Za-z_]\w*)*/,
					inside: {
						punctuation: /\./,
					},
				},
				punctuation: /[:,]/,
			},
		},
	});
	registry.insertBefore("csharp", "string", {
		"interpolation-string": [
			{
				pattern: /(^|[^\\])(?:\$@|@\$)"(?:""|\\[\s\S]|\{\{|[^\\{"])*"/,
				lookbehind: true,
				greedy: true,
			},
			{
				pattern: /(^|[^@\\])\$"(?:\\.|\{\{|[^\\"{])*"/,
				lookbehind: true,
				greedy: true,
			},
		],
		char: {
			pattern: /'(?:[^\r\n'\\]|\\.|\\[Uux][\da-fA-F]{1,8})'/,
			greedy: true,
		},
	});

	registry.cs = registry.csharp;
	registry.dotnet = registry.csharp;
	registry["c#"] = registry.csharp;
	return registry.csharp as Grammar;
}
