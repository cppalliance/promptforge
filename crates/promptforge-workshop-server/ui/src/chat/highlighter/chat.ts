export * from "./core";

import {
	type CreateHighlighterOptions as CoreCreateHighlighterOptions,
	highlight as coreHighlight,
	createHighlighter as createCoreHighlighter,
	type Grammar,
	type Highlighter,
	type LanguageCollection,
	type LanguageDefinition,
	languages,
} from "./core";
import { registerBuiltInLanguages } from "./languages/index";

let builtInLanguagesRegistered = false;

ensureBuiltInLanguages();

export type LanguageLoadResult = LanguageDefinition | { default?: LanguageDefinition } | null | undefined;

export interface ChatHighlighter {
	registerLanguage: Highlighter["registerLanguage"];
	loadLanguage: (language: string) => Promise<boolean>;
	highlight: (code: string, language: string) => Promise<string>;
}

export interface CreateHighlighterOptions extends CoreCreateHighlighterOptions {
	loadLanguage?: (language: string) => Promise<LanguageLoadResult>;
}

export function highlight(code: string, language: string): string {
	ensureBuiltInLanguages();
	return coreHighlight(code, language);
}

export function createHighlighter(options: CreateHighlighterOptions = {}): ChatHighlighter {
	const { loadLanguage, languages: extraLanguages } = options;
	const highlighter = createCoreHighlighter();

	registerBuiltInLanguages(highlighter.languages);
	registerLanguageCollection(highlighter, extraLanguages);

	async function load(language: string): Promise<boolean> {
		const id = language.toLowerCase();

		if (highlighter.languages[language] || highlighter.languages[id]) {
			return true;
		}

		if (!loadLanguage) {
			return false;
		}

		try {
			const definition = resolveLanguageDefinition(await loadLanguage(language));

			if (!definition) {
				return false;
			}

			highlighter.registerLanguage(definition);
			return true;
		} catch {
			return false;
		}
	}

	return {
		registerLanguage: highlighter.registerLanguage,
		loadLanguage: load,
		async highlight(code: string, language: string): Promise<string> {
			await load(language);
			return highlighter.highlight(code, language);
		},
	};
}

function registerLanguageCollection(highlighter: Highlighter, collection: LanguageCollection | undefined): void {
	if (!collection) {
		return;
	}

	if (Array.isArray(collection)) {
		for (const definition of collection) {
			highlighter.registerLanguage(definition);
		}

		return;
	}

	for (const [language, grammar] of Object.entries(collection)) {
		if (isGrammar(grammar)) {
			highlighter.registerLanguage(language, grammar);
		}
	}
}

function isGrammar(value: unknown): value is Grammar {
	return !!value && typeof value === "object" && !Array.isArray(value) && !(value instanceof RegExp);
}

function resolveLanguageDefinition(result: LanguageLoadResult): LanguageDefinition | null {
	if (isLanguageDefinition(result)) {
		return result;
	}

	if (result && typeof result === "object" && "default" in result && isLanguageDefinition(result.default)) {
		return result.default;
	}

	return null;
}

function isLanguageDefinition(value: unknown): value is LanguageDefinition {
	return (
		!!value &&
		typeof value === "object" &&
		typeof (value as LanguageDefinition).id === "string" &&
		!!(value as LanguageDefinition).grammar
	);
}

function ensureBuiltInLanguages(): void {
	if (builtInLanguagesRegistered) {
		return;
	}

	registerBuiltInLanguages(languages);
	builtInLanguagesRegistered = true;
}
