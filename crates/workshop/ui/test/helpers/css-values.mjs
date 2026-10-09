// Source-text readers for the stylesheet contract tests. jsdom applies no
// layout, so a test that pins a declared value reads the stylesheet itself:
// rulesOf() flattens a sheet into { at, selectors, body } rows (the
// at-rule headers around a rule, such as an @container query, ride along in
// `at`), declaration() reads one property out of a body, and resolver() builds
// a function that follows var(--token) chains through the token sheets
// (look's tokens, sizes, and semantic sheets and the app's component
// sheet) down to the literal a stylesheet declares.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const lookDir = path.join(uiDir, "..", "look");

/** Reads a file under crates/workshop/ui. */
export const readUi = (relative) => readFile(path.join(uiDir, relative), "utf8");

/** Reads a file under crates/workshop/look. */
export const readLook = (relative) => readFile(path.join(lookDir, relative), "utf8");

export const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, "");

/**
 * Every style rule in a sheet as { at, selectors, body }: `at` lists the
 * at-rule headers wrapped around it (outermost first), `selectors` the
 * whitespace-normalized selector list, `body` the declarations.
 */
export function rulesOf(css) {
  const code = stripComments(css);
  const rules = [];
  const open = [];
  let headerStart = 0;
  for (let i = 0; i < code.length; i += 1) {
    const char = code[i];
    if (char === "{") {
      open.push({ header: code.slice(headerStart, i).replace(/\s+/g, " ").trim(), bodyStart: i + 1 });
      headerStart = i + 1;
    } else if (char === "}") {
      const closed = open.pop();
      const body = code.slice(closed.bodyStart, i);
      if (!closed.header.startsWith("@") && !body.includes("{")) {
        rules.push({
          at: open.map((entry) => entry.header).filter((header) => header.startsWith("@")),
          selectors: closed.header.split(",").map((selector) => selector.replace(/\s+/g, " ").trim()),
          body: body.replace(/\s+/g, " ").trim(),
        });
      }
      headerStart = i + 1;
    } else if (char === ";") {
      headerStart = i + 1;
    }
  }
  return rules;
}

/** One property's value out of a rule body, or undefined. */
export function declaration(body, property) {
  for (const part of body.split(";")) {
    const colon = part.indexOf(":");
    if (colon >= 0 && part.slice(0, colon).trim() === property) {
      return part.slice(colon + 1).trim();
    }
  }
  return undefined;
}

/**
 * The value `property` has in the rules whose selector list holds
 * `selector`, outside any at-rule unless `at` says which; the last such rule
 * that declares it wins, as the cascade breaks a tie by source order.
 */
export function valueIn(rules, selector, property, at = null) {
  let found;
  for (const rule of rules) {
    if (!rule.selectors.includes(selector)) continue;
    if (at === null ? rule.at.length > 0 : !rule.at.some((header) => header.includes(at))) continue;
    const value = declaration(rule.body, property);
    if (value !== undefined) found = value;
  }
  return found;
}

/**
 * Builds resolve(value): follows var(--name) and var(--name, fallback)
 * references through every token sheet until only literals remain, and
 * lowercases the result so a hex color compares however the sheet cases it.
 */
export async function resolver() {
  const sheets = [
    await readLook("tokens.css"),
    await readLook("sizes.css"),
    await readLook("semantic.css"),
    await readUi("src/tokens/component.css"),
  ];
  const tokens = new Map();
  for (const sheet of sheets) {
    for (const match of stripComments(sheet).matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      tokens.set(match[1], match[2].replace(/\s+/g, " ").trim());
    }
  }
  const resolve = (value, depth = 0) => {
    if (value === undefined || depth > 20) return value;
    let out = "";
    let i = 0;
    while (i < value.length) {
      const start = value.indexOf("var(", i);
      if (start === -1) {
        out += value.slice(i);
        break;
      }
      out += value.slice(i, start);
      // Scan to the var()'s balanced closing parenthesis, noting the first top-level comma.
      let nesting = 0;
      let comma = -1;
      let end = start + 3;
      for (; end < value.length; end += 1) {
        if (value[end] === "(") nesting += 1;
        else if (value[end] === ")") {
          nesting -= 1;
          if (nesting === 0) break;
        } else if (value[end] === "," && nesting === 1 && comma === -1) comma = end;
      }
      const inner = value.slice(start + 4, end);
      const name = (comma === -1 ? inner : value.slice(start + 4, comma)).trim();
      const fallback = comma === -1 ? undefined : value.slice(comma + 1, end).trim();
      const target = tokens.get(name);
      if (target !== undefined) out += resolve(target, depth + 1);
      else if (fallback !== undefined) out += resolve(fallback, depth + 1);
      else out += value.slice(start, end + 1);
      i = end + 1;
    }
    return out.toLowerCase();
  };
  resolve.tokens = tokens;
  return resolve;
}
