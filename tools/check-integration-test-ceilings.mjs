import {
  readdirSync,
  readFileSync,
  statSync,
} from "node:fs";
import { dirname, join, posix, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REQUIRED_SUITES = Object.freeze([
  "crates/gateway/tests/it/realtime_stt",
  "crates/workshop-server/tests/it/chat_gate",
  "crates/workshop-server/tests/it/realtime_relay",
]);

const SUPPORTED_TEST_ATTRIBUTES = new Set(["test", "tokio::test"]);
const ITEM_KEYWORDS = new Set([
  "const",
  "enum",
  "fn",
  "impl",
  "mod",
  "static",
  "struct",
  "trait",
  "type",
  "union",
  "use",
]);

function fail(message) {
  throw new Error(message);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function repoPath(value) {
  return value.replaceAll("\\", "/");
}

function requireNormalizedRepoPath(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    fail(`${label} must be a non-empty repository-relative path`);
  }
  if (value !== repoPath(value)) {
    fail(`${label} must use normalized repository separators: ${value}`);
  }
  const parts = value.split("/");
  if (
    value.startsWith("/") ||
    /^[A-Za-z]:/.test(value) ||
    parts.some((part) => part.length === 0 || part === "." || part === "..")
  ) {
    fail(`${label} must be a normalized repository-relative path: ${value}`);
  }
}

export function physicalLineCount(source) {
  if (source.length === 0) {
    return 0;
  }
  const lines = source.split(/\r\n|\n|\r/);
  if (/(?:\r\n|\n|\r)$/.test(source)) {
    lines.pop();
  }
  return lines.length;
}

function rawStringAt(source, start) {
  let cursor = start;
  if (source[cursor] === "b") {
    cursor += 1;
  }
  if (source[cursor] !== "r") {
    return undefined;
  }
  cursor += 1;
  let hashes = 0;
  while (source[cursor] === "#") {
    hashes += 1;
    cursor += 1;
  }
  if (source[cursor] !== '"') {
    return undefined;
  }
  const contentStart = cursor + 1;
  const closing = `"${"#".repeat(hashes)}`;
  const closingStart = source.indexOf(closing, contentStart);
  if (closingStart < 0) {
    fail("unterminated Rust raw string");
  }
  return {
    end: closingStart + closing.length,
    value: source.slice(contentStart, closingStart),
  };
}

function quotedStringAt(source, start) {
  const quote = source[start] === "b" ? start + 1 : start;
  if (source[quote] !== '"') {
    return undefined;
  }
  let value = "";
  for (let cursor = quote + 1; cursor < source.length; cursor += 1) {
    const character = source[cursor];
    if (character === '"') {
      return { end: cursor + 1, value };
    }
    if (character !== "\\") {
      value += character;
      continue;
    }
    cursor += 1;
    const escaped = source[cursor];
    const simple = {
      0: "\0",
      '"': '"',
      "'": "'",
      "\\": "\\",
      n: "\n",
      r: "\r",
      t: "\t",
    };
    if (Object.hasOwn(simple, escaped)) {
      value += simple[escaped];
    } else if (escaped === "x") {
      const digits = source.slice(cursor + 1, cursor + 3);
      if (!/^[0-9A-Fa-f]{2}$/.test(digits)) {
        fail("invalid Rust hexadecimal string escape");
      }
      value += String.fromCharCode(Number.parseInt(digits, 16));
      cursor += 2;
    } else if (escaped === "u" && source[cursor + 1] === "{") {
      const close = source.indexOf("}", cursor + 2);
      const digits = source.slice(cursor + 2, close);
      if (close < 0 || !/^[0-9A-Fa-f_]+$/.test(digits)) {
        fail("invalid Rust Unicode string escape");
      }
      value += String.fromCodePoint(Number.parseInt(digits.replaceAll("_", ""), 16));
      cursor = close;
    } else if (escaped === "\n" || escaped === "\r") {
      if (escaped === "\r" && source[cursor + 1] === "\n") {
        cursor += 1;
      }
      while (/\s/.test(source[cursor + 1] ?? "")) {
        cursor += 1;
      }
    } else {
      fail(`unsupported Rust string escape: \\${escaped}`);
    }
  }
  fail("unterminated Rust string");
}

function rustTokens(source) {
  const tokens = [];
  for (let cursor = 0; cursor < source.length; ) {
    if (/\s/.test(source[cursor])) {
      cursor += 1;
      continue;
    }
    if (source.startsWith("//", cursor)) {
      const newline = source.indexOf("\n", cursor + 2);
      cursor = newline < 0 ? source.length : newline + 1;
      continue;
    }
    if (source.startsWith("/*", cursor)) {
      let depth = 1;
      cursor += 2;
      while (cursor < source.length && depth > 0) {
        if (source.startsWith("/*", cursor)) {
          depth += 1;
          cursor += 2;
        } else if (source.startsWith("*/", cursor)) {
          depth -= 1;
          cursor += 2;
        } else {
          cursor += 1;
        }
      }
      if (depth !== 0) {
        fail("unterminated Rust block comment");
      }
      continue;
    }

    const rawString = rawStringAt(source, cursor);
    if (rawString !== undefined) {
      tokens.push({ kind: "string", value: rawString.value });
      cursor = rawString.end;
      continue;
    }
    const quotedString = quotedStringAt(source, cursor);
    if (quotedString !== undefined) {
      tokens.push({ kind: "string", value: quotedString.value });
      cursor = quotedString.end;
      continue;
    }
    if (
      source[cursor] === "'" &&
      (source[cursor + 2] === "'" ||
        (source[cursor + 1] === "\\" && source[cursor + 3] === "'"))
    ) {
      cursor += source[cursor + 1] === "\\" ? 4 : 3;
      continue;
    }
    if (/[A-Za-z_]/.test(source[cursor])) {
      let end = cursor + 1;
      while (/[A-Za-z0-9_]/.test(source[end] ?? "")) {
        end += 1;
      }
      tokens.push({ kind: "identifier", value: source.slice(cursor, end) });
      cursor = end;
      continue;
    }
    if (source.startsWith("::", cursor)) {
      tokens.push({ kind: "punctuation", value: "::" });
      cursor += 2;
      continue;
    }
    tokens.push({ kind: "punctuation", value: source[cursor] });
    cursor += 1;
  }
  return tokens;
}

function matchingDelimiter(tokens, openIndex) {
  const pairs = { "(": ")", "[": "]", "{": "}" };
  const stack = [pairs[tokens[openIndex]?.value]];
  if (stack[0] === undefined) {
    fail("expected an opening Rust delimiter");
  }
  for (let cursor = openIndex + 1; cursor < tokens.length; cursor += 1) {
    const value = tokens[cursor].value;
    if (Object.hasOwn(pairs, value)) {
      stack.push(pairs[value]);
    } else if (value === stack.at(-1)) {
      stack.pop();
      if (stack.length === 0) {
        return cursor;
      }
    }
  }
  fail("unterminated Rust delimiter");
}

function attributeAt(tokens, start) {
  if (tokens[start]?.value !== "#" || tokens[start + 1]?.value !== "[") {
    return undefined;
  }
  const end = matchingDelimiter(tokens, start + 1);
  const path = [];
  for (let cursor = start + 2; cursor < end; cursor += 1) {
    const token = tokens[cursor];
    if (token.kind === "identifier" || token.value === "::") {
      path.push(token.value);
    } else {
      break;
    }
  }
  return { end, path: path.join("") };
}

function testAttributeKind(path) {
  if (SUPPORTED_TEST_ATTRIBUTES.has(path)) {
    return "supported";
  }
  if (path === "test" || path.endsWith("::test")) {
    return "unsupported";
  }
  return undefined;
}

function includeAt(tokens, start) {
  if (
    tokens[start]?.value !== "include" ||
    tokens[start + 1]?.value !== "!" ||
    !["(", "[", "{"].includes(tokens[start + 2]?.value)
  ) {
    return undefined;
  }
  const end = matchingDelimiter(tokens, start + 2);
  const macroArguments = tokens.slice(start + 3, end);
  if (macroArguments.length !== 1 || macroArguments[0].kind !== "string") {
    fail("include! in a manifested integration suite must use one string literal");
  }
  return { end, path: macroArguments[0].value };
}

function analyzeRust(source, label) {
  const tokens = rustTokens(source);
  const includes = [];
  let depth = 0;
  let pendingAttributes = [];
  let tests = 0;

  for (let cursor = 0; cursor < tokens.length; cursor += 1) {
    const attribute = attributeAt(tokens, cursor);
    if (attribute !== undefined) {
      const kind = testAttributeKind(attribute.path);
      if (depth > 0 && (kind !== undefined || attribute.path === "cfg_attr")) {
        fail(`macro-generated test is unsupported in ${label}`);
      }
      if (depth === 0) {
        pendingAttributes.push(attribute);
      }
      cursor = attribute.end;
      continue;
    }

    const token = tokens[cursor];
    if (token.value === "{") {
      if (pendingAttributes.some((entry) => testAttributeKind(entry.path))) {
        fail(`test attribute does not annotate a free function in ${label}`);
      }
      pendingAttributes = [];
      depth += 1;
      continue;
    }
    if (token.value === "}") {
      pendingAttributes = [];
      depth -= 1;
      if (depth < 0) {
        fail(`unbalanced Rust delimiter in ${label}`);
      }
      continue;
    }
    if (depth > 0) {
      continue;
    }

    const include = includeAt(tokens, cursor);
    if (include !== undefined) {
      if (
        pendingAttributes.some(
          (entry) => entry.path === "cfg" || entry.path === "cfg_attr",
        )
      ) {
        fail(`cfg-gated include! is unsupported in ${label}`);
      }
      includes.push(include.path);
      pendingAttributes = [];
      cursor = include.end;
      continue;
    }
    if (
      token.kind === "identifier" &&
      tokens[cursor + 1]?.value === "!" &&
      token.value !== "include"
    ) {
      fail(`macro-generated test is unsupported in ${label}: ${token.value}!`);
    }

    if (token.value === "fn" && pendingAttributes.length > 0) {
      const testAttributes = pendingAttributes.filter(
        (entry) => testAttributeKind(entry.path) !== undefined,
      );
      if (
        testAttributes.some(
          (entry) => testAttributeKind(entry.path) === "unsupported",
        )
      ) {
        fail(`unsupported Rust test attribute in ${label}`);
      }
      if (
        pendingAttributes.some(
          (entry) => entry.path === "cfg" || entry.path === "cfg_attr",
        ) &&
        (testAttributes.length > 0 ||
          pendingAttributes.some((entry) => entry.path === "cfg_attr"))
      ) {
        fail(`cfg-gated test is unsupported in ${label}`);
      }
      if (testAttributes.length > 1) {
        fail(`multiple Rust test attributes annotate one function in ${label}`);
      }
      tests += testAttributes.length;
      pendingAttributes = [];
      continue;
    }
    if (ITEM_KEYWORDS.has(token.value)) {
      if (pendingAttributes.some((entry) => testAttributeKind(entry.path))) {
        fail(`test attribute does not annotate a free function in ${label}`);
      }
      pendingAttributes = [];
    }
  }
  if (depth !== 0) {
    fail(`unbalanced Rust delimiter in ${label}`);
  }
  return { includes, tests };
}

function discoveredRustFiles(root, suitePath) {
  const suiteRoot = join(root, ...suitePath.split("/"));
  const files = [];

  function visit(directory) {
    let entries;
    try {
      entries = readdirSync(directory, { withFileTypes: true });
    } catch (error) {
      if (error?.code === "ENOENT") {
        fail(`missing integration test suite directory: ${suitePath}`);
      }
      throw error;
    }
    for (const entry of entries) {
      const entryPath = join(directory, entry.name);
      if (entry.isDirectory()) {
        visit(entryPath);
      } else if (entry.name.endsWith(".rs")) {
        files.push(repoPath(relative(suiteRoot, entryPath)));
      }
    }
  }

  visit(suiteRoot);
  return files.sort();
}

function requireExactSuites(suites, requiredSuites) {
  const actual = Object.keys(suites).sort();
  const expected = [...requiredSuites].sort();
  const missing = expected.filter((suite) => !actual.includes(suite));
  const extra = actual.filter((suite) => !expected.includes(suite));
  if (missing.length > 0 || extra.length > 0) {
    fail(
      `integration ceiling manifest suite coverage differs: missing ${JSON.stringify(missing)}, extra ${JSON.stringify(extra)}`,
    );
  }
}

export function checkIntegrationTestCeilings(
  root,
  manifest,
  { requiredSuites = REQUIRED_SUITES } = {},
) {
  if (!isObject(manifest) || manifest.version !== 1 || !isObject(manifest.suites)) {
    fail("integration ceiling manifest must be a version 1 object with suites");
  }
  if (
    !Array.isArray(requiredSuites) ||
    requiredSuites.length === 0 ||
    requiredSuites.some((suite) => typeof suite !== "string") ||
    new Set(requiredSuites).size !== requiredSuites.length
  ) {
    fail("required integration suites must be a non-empty array of unique paths");
  }
  requireExactSuites(manifest.suites, requiredSuites);

  const results = [];
  for (const [suitePath, suite] of Object.entries(manifest.suites)) {
    requireNormalizedRepoPath(suitePath, "suite path");
    if (
      !isObject(suite) ||
      !Number.isInteger(suite.testTotal) ||
      suite.testTotal < 0 ||
      !isObject(suite.entry) ||
      typeof suite.entry.path !== "string" ||
      !Number.isInteger(suite.entry.ceiling) ||
      suite.entry.ceiling < 1 ||
      !isObject(suite.files) ||
      Object.keys(suite.files).length === 0
    ) {
      fail(
        `${suitePath} must declare an entry, non-negative testTotal, and non-empty files`,
      );
    }
    requireNormalizedRepoPath(suite.entry.path, `${suitePath} entry path`);
    const expectedEntryPath = `${suitePath}.rs`;
    if (suite.entry.path !== expectedEntryPath) {
      fail(`${suitePath} entry path must be ${expectedEntryPath}`);
    }

    const entryPath = join(root, ...suite.entry.path.split("/"));
    let entrySource;
    try {
      if (!statSync(entryPath).isFile()) {
        fail(`missing suite entry module: ${suite.entry.path}`);
      }
      entrySource = readFileSync(entryPath, "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") {
        fail(`missing suite entry module: ${suite.entry.path}`);
      }
      throw error;
    }
    const entryLines = physicalLineCount(entrySource);
    if (entryLines > suite.entry.ceiling) {
      fail(
        `${suite.entry.path} has ${entryLines} physical lines, ceiling is ${suite.entry.ceiling}`,
      );
    }
    const entryAnalysis = analyzeRust(entrySource, suite.entry.path);

    const expectedFiles = Object.keys(suite.files).sort();
    const expectedIncludes = expectedFiles.map((file) =>
      posix.relative(
        posix.dirname(suite.entry.path),
        posix.join(suitePath, file),
      ),
    );
    const includeCounts = new Map();
    for (const included of entryAnalysis.includes) {
      includeCounts.set(included, (includeCounts.get(included) ?? 0) + 1);
    }
    for (const expectedInclude of expectedIncludes) {
      const count = includeCounts.get(expectedInclude) ?? 0;
      if (count > 1) {
        fail(
          `${suitePath} suite include must appear exactly once: ${expectedInclude}, found ${count}`,
        );
      }
    }
    const missingIncludes = expectedIncludes.filter(
      (included) => !includeCounts.has(included),
    );
    const extraIncludes = [...includeCounts.keys()]
      .filter((included) => !expectedIncludes.includes(included))
      .sort();
    if (missingIncludes.length > 0 || extraIncludes.length > 0) {
      fail(
        `${suitePath} suite include coverage differs: missing ${JSON.stringify(missingIncludes)}, extra ${JSON.stringify(extraIncludes)}`,
      );
    }

    let actualTestTotal = entryAnalysis.tests;
    for (const file of expectedFiles) {
      requireNormalizedRepoPath(file, `${suitePath} file path`);
      if (!file.endsWith(".rs")) {
        fail(`${suitePath} manifest entry is not a Rust file: ${file}`);
      }
      const ceiling = suite.files[file];
      if (!Number.isInteger(ceiling) || ceiling < 1) {
        fail(`${suitePath}/${file} ceiling must be a positive integer`);
      }

      const filePath = join(root, ...suitePath.split("/"), ...file.split("/"));
      let source;
      try {
        if (!statSync(filePath).isFile()) {
          fail(`missing manifested integration test file: ${suitePath}/${file}`);
        }
        source = readFileSync(filePath, "utf8");
      } catch (error) {
        if (error?.code === "ENOENT") {
          fail(`missing manifested integration test file: ${suitePath}/${file}`);
        }
        throw error;
      }

      const lines = physicalLineCount(source);
      if (lines > ceiling) {
        fail(
          `${suitePath}/${file} has ${lines} physical lines, ceiling is ${ceiling}`,
        );
      }
      const analysis = analyzeRust(source, `${suitePath}/${file}`);
      if (analysis.includes.length > 0) {
        fail(`${suitePath}/${file} must not contain nested include! topology`);
      }
      actualTestTotal += analysis.tests;
    }

    const actualFiles = discoveredRustFiles(root, suitePath);
    const missing = expectedFiles.filter((file) => !actualFiles.includes(file));
    if (missing.length > 0) {
      fail(
        `missing manifested integration test file: ${suitePath}/${missing[0]}`,
      );
    }
    const extra = actualFiles.filter((file) => !expectedFiles.includes(file));
    if (extra.length > 0) {
      fail(`unmanifested integration test file: ${suitePath}/${extra[0]}`);
    }
    if (actualTestTotal !== suite.testTotal) {
      fail(
        `${suitePath} has ${actualTestTotal} tests, expected exactly ${suite.testTotal}`,
      );
    }

    results.push({
      files: actualFiles.length + 1,
      path: suitePath,
      tests: actualTestTotal,
    });
  }
  return results;
}

function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const manifestPath = join(root, "tools", "integration-test-ceilings.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const results = checkIntegrationTestCeilings(root, manifest, {
    requiredSuites: REQUIRED_SUITES,
  });
  for (const result of results) {
    console.log(`${result.path}: ${result.files} files, ${result.tests} tests`);
  }
}

const invokedPath =
  process.argv[1] === undefined
    ? undefined
    : pathToFileURL(resolve(process.argv[1])).href;
if (invokedPath === import.meta.url) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
