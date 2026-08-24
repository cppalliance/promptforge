import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerJavaScriptLanguage } from "./javascript";
import { isGrammarToken, isRegisteredGrammar } from "./shared";
import { registerTypeScriptLanguage } from "./typescript";

export function registerCssLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.css;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const css: Grammar = {
		comment: {
			pattern: /\/\*[\s\S]*?\*\//,
			greedy: true,
		},
		atrule: {
			pattern: /@[\w-](?:[^;{\s]|\s+(?!\s))*?(?:;|(?=\s*\{))/,
			inside: {
				rule: /^@[\w-]+/,
				"selector-function-argument": {
					pattern: /(\bselector\s*\(\s*(?![\s)]))(?:[^()\s]|\s+(?![\s)])|\((?:[^()]|\([^()]*\))*\))+(?=\s*\))/,
					lookbehind: true,
					alias: "selector",
				},
				keyword: {
					pattern: /(^|[^\w-])(?:and|not|only|or)(?![\w-])/,
					lookbehind: true,
				},
				function: {
					pattern: /(^|[^-a-z0-9])[-a-z0-9]+(?=\()/i,
					lookbehind: true,
				},
				property: /[-_a-zA-Z\xA0-\uFFFF][-\w\xA0-\uFFFF]*(?=\s*:)/,
				punctuation: /[():]/,
			},
		},
		url: {
			pattern: /url\((?:(["'])(?:\\[\s\S]|(?!\1)[^\\])*\1|.*?)\)/i,
			greedy: true,
			inside: {
				function: /^url/i,
				punctuation: /^\(|\)$/,
				string: {
					pattern: /^("|')(?:\\[\s\S]|(?!\1)[^\\])*\1$/,
					alias: "url",
				},
			},
		},
		selector: {
			pattern: /(^|[{}]\s*)[^{}\s][^{}]*\S(?=\s*\{)/,
			lookbehind: true,
		},
		string: {
			pattern: /(["'])(?:\\[\s\S]|(?!\1)[^\\])*\1/,
			greedy: true,
		},
		property: /[-_a-zA-Z\xA0-\uFFFF][-\w\xA0-\uFFFF]*(?=\s*:)/,
		important: /!important\b/i,
		function: /[-a-z0-9]+(?=\()/i,
		punctuation: /[(){};:,]/,
	};
	const atrule = css.atrule as GrammarToken;
	if (atrule.inside) {
		atrule.inside.rest = css;
	}
	registry.css = css;
	return css;
}

export function registerMarkupLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.markup;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const markup: Grammar = {
		comment: {
			pattern: /<!--(?:(?!<!--)[\s\S])*?-->/,
			greedy: true,
		},
		prolog: {
			pattern: /<\?[\s\S]+?\?>/,
			greedy: true,
		},
		doctype: {
			pattern: /<!DOCTYPE(?:[^>"'[\]]|"[^"]*"|'[^']*')+(?:\[(?:[^<>"'\]]|"[^"]*"|'[^']*'|<(?!!--))*\]\s*)?>/i,
			greedy: true,
			inside: {
				"internal-subset": {
					pattern: /(^[^[]*\[)[\s\S]+(?=\]>$)/,
					lookbehind: true,
					greedy: true,
					inside: null,
				},
				string: {
					pattern: /"[^"]*"|'[^']*'/,
					greedy: true,
				},
				punctuation: /^<!|>$|[[\]]/,
				"doctype-tag": /^DOCTYPE/i,
				name: /[^\s<>'"]+/,
			},
		},
		cdata: {
			pattern: /<!\[CDATA\[[\s\S]*?\]\]>/i,
			greedy: true,
		},
		tag: {
			pattern: /<\/?(?!\d)[^\s>/=$<%]+(?:\s+[^\s>/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+))?)*\s*\/?>/,
			greedy: true,
			inside: {
				tag: {
					pattern: /^<\/?[^\s>/]+/,
					inside: {
						punctuation: /^<\/?/,
						namespace: /^[^\s>/:]+:/,
					},
				},
				"special-attr": [],
				"attr-value": {
					pattern: /=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+)/,
					inside: {
						punctuation: [
							{
								pattern: /^=/,
								alias: "attr-equals",
							},
							{
								pattern: /^(\s*)["']|["']$/,
								lookbehind: true,
							},
						],
						entity: [
							{
								pattern: /&[\da-z]{1,8};/i,
								alias: "named-entity",
							},
							/&#x?[\da-f]{1,8};/i,
						],
					},
				},
				"attr-name": /[^\s>/=]+/,
				punctuation: /\/?>/,
			},
		},
		entity: [
			{
				pattern: /&[\da-z]{1,8};/i,
				alias: "named-entity",
			},
			/&#x?[\da-f]{1,8};/i,
		],
	};
	registry.markup = markup;

	const css = registry.css;
	const javascript = registry.javascript;

	if (isRegisteredGrammar(css)) {
		addMarkupInlinedLanguage(registry, "style", "css", css);
		addMarkupAttributeLanguage(registry, "style", "css", css);
	}

	if (isRegisteredGrammar(javascript)) {
		addMarkupInlinedLanguage(registry, "script", "javascript", javascript);
	}

	registry.html = registry.markup;
	registry.xml = registry.markup;
	registry.svg = registry.markup;
	return registry.markup as Grammar;
}

function addMarkupInlinedLanguage(
	registry: LanguagesRegistry,
	tagName: string,
	language: string,
	grammar: Grammar,
): void {
	const includedCdataInside: Grammar = {
		[`language-${language}`]: {
			pattern: /(^<!\[CDATA\[)[\s\S]+?(?=\]\]>$)/i,
			lookbehind: true,
			inside: grammar,
		},
		cdata: /^<!\[CDATA\[|\]\]>$/i,
	};
	const inside: Grammar = {
		"included-cdata": {
			pattern: /<!\[CDATA\[[\s\S]*?\]\]>/i,
			inside: includedCdataInside,
		},
		[`language-${language}`]: {
			pattern: /[\s\S]+/,
			inside: grammar,
		},
	};

	registry.insertBefore("markup", "cdata", {
		[tagName]: {
			pattern: RegExp(
				/(<__[^>]*>)(?:<!\[CDATA\[(?:[^\]]|\](?!\]>))*\]\]>|(?!<!\[CDATA\[)[\s\S])*?(?=<\/__>)/.source.replace(
					/__/g,
					() => tagName,
				),
				"i",
			),
			lookbehind: true,
			greedy: true,
			inside,
		},
	});
}

function addMarkupAttributeLanguage(
	registry: LanguagesRegistry,
	attrName: string,
	language: string,
	grammar: Grammar,
): void {
	const markup = registry.markup;
	const tag = isRegisteredGrammar(markup) ? markup.tag : undefined;
	const tagInside = isGrammarToken(tag) ? tag.inside : undefined;
	const specialAttr = tagInside?.["special-attr"];

	if (!Array.isArray(specialAttr)) {
		return;
	}

	specialAttr.push({
		pattern: RegExp(
			`${/(^|["'\s])/.source}(?:${attrName})${/\s*=\s*(?:"[^"]*"|'[^']*'|[^\s'">=]+(?=[\s>]))/.source}`,
			"i",
		),
		lookbehind: true,
		inside: {
			"attr-name": /^[^\s=]+/,
			"attr-value": {
				pattern: /=[\s\S]+/,
				inside: {
					value: {
						pattern: /(^=\s*(["']|(?!["'])))\S[\s\S]*(?=\2$)/,
						lookbehind: true,
						alias: [language, `language-${language}`],
						inside: grammar,
					},
					punctuation: [
						{
							pattern: /^=/,
							alias: "attr-equals",
						},
						/"|'/,
					],
				},
			},
		},
	});
}

export function registerJsxLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.jsx;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const javascript = registerJavaScriptLanguage(registry);
	registerMarkupLanguage(registry);

	const jsx = registry.extend("markup", javascript);
	const space = /(?:\s|\/\/.*(?!.)|\/\*(?:[^*]|\*(?!\/))\*\/)/.source;
	const braces = /(?:\{(?:\{(?:\{[^{}]*\}|[^{}])*\}|[^{}])*\})/.source;
	const re = (source: string, flags?: string): RegExp =>
		RegExp(
			source
				.replace(/<S>/g, () => space)
				.replace(/<BRACES>/g, () => braces)
				.replace(/<SPREAD>/g, () => spread),
			flags,
		);
	let spread = /(?:\{<S>*\.{3}(?:[^{}]|<BRACES>)*\})/.source;
	spread = re(spread).source;

	const tag = jsx.tag as GrammarToken;
	tag.pattern = re(
		/<\/?(?:[\w.:-]+(?:<S>+(?:[\w.:$-]+(?:=(?:"(?:\\[\s\S]|[^\\"])*"|'(?:\\[\s\S]|[^\\'])*'|[^\s{'"/>=]+|<BRACES>))?|<SPREAD>))*<S>*\/?)?>/
			.source,
	);

	const tagInside = tag.inside as Grammar;
	const tagName = tagInside.tag as GrammarToken;
	tagName.pattern = /^<\/?[^\s>/]*/;
	(tagInside["attr-value"] as GrammarToken).pattern =
		/=(?!\{)(?:"(?:\\[\s\S]|[^\\"])*"|'(?:\\[\s\S]|[^\\'])*'|[^\s'">]+)/;
	(tagName.inside as Grammar)["class-name"] = /^[A-Z]\w*(?:\.[A-Z]\w*)*$/;
	tagInside.comment = javascript.comment;

	registry.jsx = jsx;
	registry.insertBefore("jsx", "entity", {
		"plain-text": [
			{
				pattern: /([^=]>)[^<>{}=()]+(?=<|\{)/,
				lookbehind: true,
				greedy: true,
			},
			{
				pattern: /[^<>{}]+(?=<\/)/,
				greedy: true,
			},
		],
	});
	registry.insertBefore(
		"inside",
		"attr-name",
		{
			spread: {
				pattern: re(/<SPREAD>/.source),
				inside: jsx,
			},
		},
		tag as unknown as Record<string, unknown>,
	);
	registry.insertBefore(
		"inside",
		"special-attr",
		{
			script: {
				pattern: re(/=<BRACES>/.source),
				alias: "language-javascript",
				inside: {
					"script-punctuation": {
						pattern: /^=(?=\{)/,
						alias: "punctuation",
					},
					rest: jsx,
				},
			},
		},
		tag as unknown as Record<string, unknown>,
	);

	return registry.jsx as Grammar;
}

export function registerTsxLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.tsx;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	registerJsxLanguage(registry);
	const typescript = registerTypeScriptLanguage(registry);
	const tsx = registry.extend("jsx", typescript);
	delete tsx.parameter;
	delete tsx["literal-property"];

	const tag = tsx.tag as GrammarToken;
	tag.pattern = RegExp(`${/(^|[^\w$]|(?=<\/))/.source}(?:${tag.pattern.source})`, tag.pattern.flags);
	tag.lookbehind = true;
	const tagInside = tag.inside as Grammar;
	const script = tagInside.script as GrammarToken | undefined;
	const spread = tagInside.spread as GrammarToken | undefined;

	if (script?.inside) {
		script.inside.rest = tsx;
	}

	if (spread) {
		spread.inside = tsx;
	}

	registry.tsx = tsx;
	return registry.tsx as Grammar;
}
