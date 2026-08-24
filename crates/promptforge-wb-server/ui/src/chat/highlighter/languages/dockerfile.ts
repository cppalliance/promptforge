import type { Grammar, LanguagesRegistry } from "../core";
import { isRegisteredGrammar } from "./shared";

export function registerDockerfileLanguage(registry: LanguagesRegistry): Grammar {
	const existing = registry.dockerfile ?? registry.docker;

	if (isRegisteredGrammar(existing)) {
		return existing;
	}

	const stringRule = {
		pattern: /"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'/,
		greedy: true,
	};
	const commentRule = {
		pattern: /(^[ \t]*)#.*/m,
		lookbehind: true,
		greedy: true,
	};
	const docker: Grammar = {
		instruction: {
			pattern:
				/(^[ \t]*)(?:ADD|ARG|CMD|COPY|ENTRYPOINT|ENV|EXPOSE|FROM|HEALTHCHECK|LABEL|MAINTAINER|ONBUILD|RUN|SHELL|STOPSIGNAL|USER|VOLUME|WORKDIR)(?=\s)(?:\\.|[^\r\n\\])*(?:\\$(?:\s|#.*$)*(?![\s#])(?:\\.|[^\r\n\\])*)*/im,
			lookbehind: true,
			greedy: true,
			inside: {
				options: {
					pattern:
						/(^ONBUILD(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))\w+(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))|^\w+(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+)(?:(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+))*/i,
					lookbehind: true,
					greedy: true,
					inside: {
						property: {
							pattern: /(^|\s)--[\w-]+/,
							lookbehind: true,
						},
						string: [
							stringRule,
							{
								pattern: /(=)(?!["'])(?:[^\s\\]|\\.)+/,
								lookbehind: true,
							},
						],
						operator: /\\$/m,
						punctuation: /=/,
					},
				},
				keyword: [
					{
						pattern:
							/(^ONBUILD(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))HEALTHCHECK(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))(?:--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+)(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))*|^HEALTHCHECK(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))(?:--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+)(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))*)(?:CMD|NONE)\b/i,
						lookbehind: true,
						greedy: true,
					},
					{
						pattern:
							/(^ONBUILD(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))FROM(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))(?:--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+)(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))*(?!--)[^ \t\\]+(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))|^FROM(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n]))(?:--[\w-]+=(?:"(?:[^"\\\r\n]|\\(?:\r\n|[\s\S]))*"|'(?:[^'\\\r\n]|\\(?:\r\n|[\s\S]))*'|(?!["'])(?:[^\s\\]|\\.)+)(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))*(?!--)[^ \t\\]+(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))AS/i,
						lookbehind: true,
						greedy: true,
					},
					{
						pattern: /(^ONBUILD(?:[ \t]+(?![ \t])|\\[\r\n](?:\s|\\[\r\n]|#.*(?!.))*(?![\s#]|\\[\r\n])))\w+/i,
						lookbehind: true,
						greedy: true,
					},
					{
						pattern: /^\w+/,
						greedy: true,
					},
				],
				comment: commentRule,
				string: stringRule,
				variable: /\$(?:\w+|\{[^{}"'\\]*\})/,
				operator: /\\$/m,
			},
		},
		comment: commentRule,
	};

	registry.docker = docker;
	registry.dockerfile = registry.docker;
	return registry.docker;
}
