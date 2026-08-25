import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerYamlLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.yaml;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const anchorOrAlias = /[*&][^\s[\]{},]+/;
	const tag = /!(?:<[\w\-%#;/?:@&=+$,.!~*'()[\]]+>|(?:[a-zA-Z\d-]*!)?[\w\-%#;/?:@&=+$.~*'()]+)?/;
	const properties = `(?:${tag.source}(?:[ \t]+${anchorOrAlias.source})?|${anchorOrAlias.source}(?:[ \t]+${tag.source})?)`;
	const excludedControlRanges = "\\x00-\\x08\\x0e-\\x1f\\x7f-\\x84\\x86-\\x9f\\ud800-\\udfff\\ufffe\\uffff";
	const plainCharacter = `[^\\s${excludedControlRanges},[\\]{}]`;
	const plainKeyCharacter = `[^\\s${excludedControlRanges}!"#%&'*,\\-:>?@[\\]\`{|}]`;
	const plainKey = `(?:${plainKeyCharacter}|[?:-]<PLAIN>)(?:[ \t]*(?:(?![#:])<PLAIN>|:<PLAIN>))*`.replace(
		/<PLAIN>/g,
		() => plainCharacter,
	);
	const string = /"(?:[^"\\\r\n]|\\.)*"|'(?:[^'\\\r\n]|\\.)*'/.source;
	const createValuePattern = (value: string, flags = ""): RegExp =>
		RegExp(
			/([:\-,[{]\s*(?:\s<<prop>>[ \t]+)?)(?:<<value>>)(?=[ \t]*(?:$|,|\]|\}|(?:[\r\n]\s*)?#))/.source
				.replace(/<<prop>>/g, () => properties)
				.replace(/<<value>>/g, () => value),
			`${flags.replace(/m/g, "")}m`,
		);

	const yaml: Grammar = {
		scalar: {
			pattern: RegExp(
				/([-:]\s*(?:\s<<prop>>[ \t]+)?[|>])[ \t]*(?:((?:\r?\n|\r)[ \t]+)\S[^\r\n]*(?:\2[^\r\n]+)*)/.source.replace(
					/<<prop>>/g,
					() => properties,
				),
			),
			lookbehind: true,
			alias: "string",
		},
		comment: /#.*/,
		key: {
			pattern: RegExp(
				/((?:^|[:\-,[{\r\n?])[ \t]*(?:<<prop>>[ \t]+)?)<<key>>(?=\s*:\s)/.source
					.replace(/<<prop>>/g, () => properties)
					.replace(/<<key>>/g, () => `(?:${plainKey}|${string})`),
			),
			lookbehind: true,
			greedy: true,
			alias: "atrule",
		},
		directive: {
			pattern: /(^[ \t]*)%.+/m,
			lookbehind: true,
			alias: "important",
		},
		datetime: {
			pattern: createValuePattern(
				/\d{4}-\d\d?-\d\d?(?:[tT]|[ \t]+)\d\d?:\d{2}:\d{2}(?:\.\d*)?(?:[ \t]*(?:Z|[-+]\d\d?(?::\d{2})?))?|\d{4}-\d{2}-\d{2}|\d\d?:\d{2}(?::\d{2}(?:\.\d*)?)?/
					.source,
			),
			lookbehind: true,
			alias: "number",
		},
		boolean: {
			pattern: createValuePattern(/false|true/.source, "i"),
			lookbehind: true,
			alias: "important",
		},
		null: {
			pattern: createValuePattern(/null|~/.source, "i"),
			lookbehind: true,
			alias: "important",
		},
		string: {
			pattern: createValuePattern(string),
			lookbehind: true,
			greedy: true,
		},
		number: {
			pattern: createValuePattern(
				/[+-]?(?:0x[\da-f]+|0o[0-7]+|(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?|\.inf|\.nan)/.source,
				"i",
			),
			lookbehind: true,
		},
		tag,
		important: anchorOrAlias,
		punctuation: /---|[:[\]{}\-,|>?]|\.\.\./,
	};

	registry.yaml = yaml;
	registry.yml = registry.yaml;
	return yaml;
}
