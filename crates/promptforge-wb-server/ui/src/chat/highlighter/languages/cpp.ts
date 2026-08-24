import type { Grammar, GrammarToken, LanguagesRegistry } from "../core";
import { registerCLanguage } from "./c";
import { isRegisteredGrammar } from "./shared";

export function registerCppLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.cpp;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	registerCLanguage(registry);

	const keyword =
		/\b(?:alignas|alignof|asm|auto|bool|break|case|catch|char|char16_t|char32_t|char8_t|class|co_await|co_return|co_yield|compl|concept|const|const_cast|consteval|constexpr|constinit|continue|decltype|default|delete|do|double|dynamic_cast|else|enum|explicit|export|extern|final|float|for|friend|goto|if|import|inline|int|int16_t|int32_t|int64_t|int8_t|long|module|mutable|namespace|new|noexcept|nullptr|operator|override|private|protected|public|register|reinterpret_cast|requires|return|short|signed|sizeof|static|static_assert|static_cast|struct|switch|template|this|thread_local|throw|try|typedef|typeid|typename|uint16_t|uint32_t|uint64_t|uint8_t|union|unsigned|using|virtual|void|volatile|wchar_t|while)\b/;

	const cpp = registry.extend("c", {
		"class-name": [
			{
				pattern:
					/(\b(?:class|concept|enum|struct|typename)\s+)(?!alignas|alignof|asm|auto|bool|break|case|catch|char|class|const|constexpr|continue|decltype|default|delete|do|double|else|enum|explicit|extern|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|operator|private|protected|public|return|short|signed|sizeof|static|struct|switch|template|this|throw|try|typedef|typename|union|unsigned|using|virtual|void|volatile|while)\w+/,
				lookbehind: true,
			},
			/\b[A-Z]\w*(?=\s*::\s*\w+\s*\()/,
			/\b[A-Z_]\w*(?=\s*::\s*~\w+\s*\()/i,
			/\b\w+(?=\s*<(?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*>\s*::\s*\w+\s*\()/,
		],
		keyword,
		number: {
			pattern:
				/(?:\b0b[01']+|\b0x(?:[\da-f']+(?:\.[\da-f']*)?|\.[\da-f']+)(?:p[+-]?[\d']+)?|(?:\b[\d']+(?:\.[\d']*)?|\B\.[\d']+)(?:e[+-]?[\d']+)?)[ful]{0,4}/i,
			greedy: true,
		},
		operator:
			/>>=?|<<=?|->|--|\+\+|&&|\|\||[?:~]|<=>|[-+*/%&|^!=<>]=?|\b(?:and|and_eq|bitand|bitor|not|not_eq|or|or_eq|xor|xor_eq)\b/,
		boolean: /\b(?:false|true)\b/,
	});

	registry.cpp = cpp;
	registry.insertBefore("cpp", "string", {
		module: {
			pattern:
				/(\b(?:import|module)\s+)(?:"(?:\\(?:\r\n|[\s\S])|[^"\\\r\n])*"|<[^<>\r\n]*>|\b(?!alignas|alignof|asm|auto|bool|break|case|catch|char|class|const|constexpr|continue|decltype|default|delete|do|double|else|enum|explicit|extern|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|operator|private|protected|public|return|short|signed|sizeof|static|struct|switch|template|this|throw|try|typedef|typename|union|unsigned|using|virtual|void|volatile|while)\w+(?:\s*\.\s*\w)*\b(?:\s*:\s*\b(?!alignas|alignof|asm|auto|bool|break|case|catch|char|class|const|constexpr|continue|decltype|default|delete|do|double|else|enum|explicit|extern|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|operator|private|protected|public|return|short|signed|sizeof|static|struct|switch|template|this|throw|try|typedef|typename|union|unsigned|using|virtual|void|volatile|while)\w+(?:\s*\.\s*\w)*\b)?|:\s*\b(?!alignas|alignof|asm|auto|bool|break|case|catch|char|class|const|constexpr|continue|decltype|default|delete|do|double|else|enum|explicit|extern|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|operator|private|protected|public|return|short|signed|sizeof|static|struct|switch|template|this|throw|try|typedef|typename|union|unsigned|using|virtual|void|volatile|while)\w+(?:\s*\.\s*\w)*\b)/,
			lookbehind: true,
			greedy: true,
			inside: {
				string: /^[<"][\s\S]+/,
				operator: /:/,
				punctuation: /\./,
			},
		},
		"raw-string": {
			pattern: /R"([^()\\ ]{0,16})\([\s\S]*?\)\1"/,
			alias: "string",
			greedy: true,
		},
	});
	registry.insertBefore("cpp", "keyword", {
		"generic-function": {
			pattern: /\b(?!operator\b)[a-z_]\w*\s*<(?:[^<>]|<[^<>]*>)*>(?=\s*\()/i,
			inside: {
				function: /^\w+/,
				generic: {
					pattern: /<[\s\S]+/,
					alias: "class-name",
					inside: cpp,
				},
			},
		},
	});
	registry.insertBefore("cpp", "operator", {
		"double-colon": {
			pattern: /::/,
			alias: "punctuation",
		},
	});
	const cppWithBaseClause = registry.insertBefore("cpp", "class-name", {
		"base-clause": {
			pattern: /(\b(?:class|struct)\s+\w+\s*:\s*)[^;{}"'\s]+(?:\s+[^;{}"'\s]+)*(?=\s*[;{])/,
			lookbehind: true,
			greedy: true,
			inside: registry.extend("cpp", {}),
		},
	});

	const baseClause = cppWithBaseClause["base-clause"] as GrammarToken;

	if (isRegisteredGrammar(baseClause.inside)) {
		registry.insertBefore(
			"inside",
			"double-colon",
			{
				"class-name": /\b[a-z_]\w*\b(?!\s*::)/i,
			},
			baseClause as unknown as Record<string, unknown>,
		);
	}

	registry["c++"] = registry.cpp;
	return registry.cpp as Grammar;
}
