import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerPhpLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.php;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const comment = /\/\*[\s\S]*?\*\/|\/\/.*|#(?!\[).*/;
	const number =
		/\b0b[01]+(?:_[01]+)*\b|\b0o[0-7]+(?:_[0-7]+)*\b|\b0x[\da-f]+(?:_[\da-f]+)*\b|(?:\b\d+(?:_\d+)*\.?(?:\d+(?:_\d+)*)?|\B\.\d+)(?:e[+-]?\d+)?/i;
	const operator = /<?=>|\?\?=?|\.{3}|\??->|[!=]=?=?|::|\*\*=?|--|\+\+|&&|\|\||<<|>>|[?~]|[/^|%*&<>.+-]=?/;
	const punctuation = /[{}[\](),:;]/;
	const constant = [
		{
			pattern: /\b(?:false|true)\b/i,
			alias: "boolean",
		},
		{
			pattern: /(::\s*)\b[a-z_]\w*\b(?!\s*\()/i,
			greedy: true,
			lookbehind: true,
		},
		{
			pattern: /(\b(?:case|const)\s+)\b[a-z_]\w*(?=\s*[;=])/i,
			greedy: true,
			lookbehind: true,
		},
		/\b(?:null)\b/i,
		/\b[A-Z_][A-Z0-9_]*\b(?!\s*\()/,
	];

	const php: Grammar = {
		delimiter: {
			pattern: /\?>$|^<\?(?:php(?=\s)|=)?/i,
			alias: "important",
		},
		comment,
		variable: /\$+(?:\w+\b|(?=\{))/,
		package: {
			pattern: /(namespace\s+|use\s+(?:function\s+)?)(?:\\?\b[a-z_]\w*)+\b(?!\\)/i,
			lookbehind: true,
			inside: {
				punctuation: /\\/,
			},
		},
		"class-name-definition": {
			pattern: /(\b(?:class|enum|interface|trait)\s+)\b[a-z_]\w*(?!\\)\b/i,
			lookbehind: true,
			alias: "class-name",
		},
		"function-definition": {
			pattern: /(\bfunction\s+)[a-z_]\w*(?=\s*\()/i,
			lookbehind: true,
			alias: "function",
		},
		keyword: [
			{
				pattern: /(\(\s*)\b(?:array|bool|boolean|float|int|integer|object|string)\b(?=\s*\))/i,
				alias: "type-casting",
				greedy: true,
				lookbehind: true,
			},
			{
				pattern:
					/([(,?]\s*)\b(?:array(?!\s*\()|bool|callable|(?:false|null)(?=\s*\|)|float|int|iterable|mixed|object|self|static|string)\b(?=\s*\$)/i,
				alias: "type-hint",
				greedy: true,
				lookbehind: true,
			},
			{
				pattern:
					/(\)\s*:\s*(?:\?\s*)?)\b(?:array(?!\s*\()|bool|callable|(?:false|null)(?=\s*\|)|float|int|iterable|mixed|never|object|self|static|string|void)\b/i,
				alias: "return-type",
				greedy: true,
				lookbehind: true,
			},
			{
				pattern: /\b(?:array(?!\s*\()|bool|float|int|iterable|mixed|object|string|void)\b/i,
				alias: "type-declaration",
				greedy: true,
			},
			{
				pattern: /(\|\s*)(?:false|null)\b|\b(?:false|null)(?=\s*\|)/i,
				alias: "type-declaration",
				greedy: true,
				lookbehind: true,
			},
			{
				pattern: /\b(?:parent|self|static)(?=\s*::)/i,
				alias: "static-context",
				greedy: true,
			},
			{
				pattern: /(\byield\s+)from\b/i,
				lookbehind: true,
			},
			/\bclass\b/i,
			{
				pattern:
					/((?:^|[^\s>:]|(?:^|[^-])>|(?:^|[^:]):)\s*)\b(?:abstract|and|array|as|break|callable|case|catch|clone|const|continue|declare|default|die|do|echo|else|elseif|empty|enddeclare|endfor|endforeach|endif|endswitch|endwhile|enum|eval|exit|extends|final|finally|fn|for|foreach|function|global|goto|if|implements|include|include_once|instanceof|insteadof|interface|isset|list|match|namespace|never|new|or|parent|print|private|protected|public|readonly|require|require_once|return|self|static|switch|throw|trait|try|unset|use|var|while|xor|yield|__halt_compiler)\b/i,
				lookbehind: true,
			},
		],
		"argument-name": {
			pattern: /([(,]\s*)\b[a-z_]\w*(?=\s*:(?!:))/i,
			lookbehind: true,
		},
		"class-name": [
			{
				pattern: /(\b(?:extends|implements|instanceof|new(?!\s+self|\s+static))\s+|\bcatch\s*\()\b[a-z_]\w*(?!\\)\b/i,
				greedy: true,
				lookbehind: true,
			},
			{
				pattern: /(\|\s*)\b[a-z_]\w*(?!\\)\b/i,
				greedy: true,
				lookbehind: true,
			},
			{
				pattern: /\b[a-z_]\w*(?!\\)\b(?=\s*\|)/i,
				greedy: true,
			},
			{
				pattern: /\b[a-z_]\w*(?=\s*\$)/i,
				alias: "type-declaration",
				greedy: true,
			},
			{
				pattern: /\b[a-z_]\w*(?=\s*::)/i,
				alias: "static-context",
				greedy: true,
			},
			{
				pattern: /([(,?]\s*)[a-z_]\w*(?=\s*\$)/i,
				alias: "type-hint",
				greedy: true,
				lookbehind: true,
			},
			{
				pattern: /(\)\s*:\s*(?:\?\s*)?)\b[a-z_]\w*(?!\\)\b/i,
				alias: "return-type",
				greedy: true,
				lookbehind: true,
			},
		],
		constant,
		function: {
			pattern: /(^|[^\\\w])\\?[a-z_](?:[\w\\]*\w)?(?=\s*\()/i,
			lookbehind: true,
			inside: {
				punctuation: /\\/,
			},
		},
		property: {
			pattern: /(->\s*)\w+/,
			lookbehind: true,
		},
		number,
		operator,
		punctuation,
	};

	const stringInterpolation = {
		pattern: /\{\$(?:\{(?:\{[^{}]+\}|[^{}]+)\}|[^{}])+\}|(^|[^\\{])\$+(?:\w+(?:\[[^\r\n[\]]+\]|->\w+)?)/,
		lookbehind: true,
		inside: php,
	};
	const string = [
		{
			pattern: /<<<'([^']+)'[\r\n](?:.*[\r\n])*?\1;/,
			alias: "nowdoc-string",
			greedy: true,
		},
		{
			pattern: /<<<(?:"([^"]+)"[\r\n](?:.*[\r\n])*?\1;|([a-z_]\w*)[\r\n](?:.*[\r\n])*?\2;)/i,
			alias: "heredoc-string",
			greedy: true,
			inside: {
				interpolation: stringInterpolation,
			},
		},
		{
			pattern: /`(?:\\[\s\S]|[^\\`])*`/,
			alias: "backtick-quoted-string",
			greedy: true,
		},
		{
			pattern: /'(?:\\[\s\S]|[^\\'])*'/,
			alias: "single-quoted-string",
			greedy: true,
		},
		{
			pattern: /"(?:\\[\s\S]|[^\\"])*"/,
			alias: "double-quoted-string",
			greedy: true,
			inside: {
				interpolation: stringInterpolation,
			},
		},
	];

	registry.php = php;
	registry.insertBefore("php", "variable", {
		string,
	});

	return php;
}
