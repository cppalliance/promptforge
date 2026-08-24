import type { Grammar, GrammarToken } from "../core";

export function isRegisteredGrammar(value: unknown): value is Grammar {
	return !!value && typeof value === "object";
}

export function isGrammarToken(value: unknown): value is GrammarToken {
	return !!value && typeof value === "object" && "pattern" in value;
}

export function escapeRegExp(value: string): string {
	return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
