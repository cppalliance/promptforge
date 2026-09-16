// The when-expression language for context keys: the small declarative
// syntax that menus, keybindings, and action preconditions use to say
// "this row applies when editorTextFocus && editorLangId == 'rust'".
// The grammar is the VS Code subset: a key (truthy test), !key,
// key == 'value' and key != 'value' against a single-quoted string
// literal, &&, ||, parentheses, and the literals true and false.
// deserialize parses once at registration and returns a Result - a
// malformed string is a value the registrar reports, never an exception
// thrown later at render. The parsed expression evaluates against a key
// lookup and names the keys it reads through keys(), so subscribers can
// re-evaluate only when a relevant key changes.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { err, ok, type Result } from "./error-catalog";

/** A parse failure: what went wrong and where in the source string. */
export interface ParseError {
  readonly message: string;
  /** The character offset in the source where parsing stopped. */
  readonly offset: number;
}

/**
 * A parsed when-expression. evaluate reads key values through the
 * supplied lookup (undefined for an unset key); keys() returns the names
 * the expression reads, for change filtering.
 */
export interface ContextKeyExpression {
  evaluate(getValue: (key: string) => unknown): boolean;
  keys(): ReadonlySet<string>;
}

type Token =
  | { readonly kind: "ident"; readonly name: string; readonly offset: number }
  | { readonly kind: "string"; readonly value: string; readonly offset: number }
  | { readonly kind: "not" | "and" | "or" | "lparen" | "rparen" | "eq" | "ne"; readonly offset: number };

/** Internal control flow for parse failures; caught at the deserialize boundary. */
class ParseFailure extends Error {
  constructor(readonly parseError: ParseError) {
    super(parseError.message);
  }
}

function fail(message: string, offset: number): never {
  throw new ParseFailure({ message, offset });
}

const NO_KEYS: ReadonlySet<string> = new Set();

function literal(value: boolean): ContextKeyExpression {
  return { evaluate: () => value, keys: () => NO_KEYS };
}

function keyTest(name: string): ContextKeyExpression {
  const keys: ReadonlySet<string> = new Set([name]);
  return { evaluate: (getValue) => Boolean(getValue(name)), keys: () => keys };
}

function comparison(name: string, value: string, negate: boolean): ContextKeyExpression {
  const keys: ReadonlySet<string> = new Set([name]);
  return {
    evaluate: (getValue) => {
      const actual = getValue(name);
      // Unset keys never equal a literal; numbers and booleans compare by
      // their string form so `flag == 'true'` reads naturally.
      const equal = actual !== undefined && actual !== null && String(actual) === value;
      return negate ? !equal : equal;
    },
    keys: () => keys,
  };
}

function negateExpr(inner: ContextKeyExpression): ContextKeyExpression {
  return { evaluate: (getValue) => !inner.evaluate(getValue), keys: () => inner.keys() };
}

function junction(
  kind: "and" | "or",
  left: ContextKeyExpression,
  right: ContextKeyExpression,
): ContextKeyExpression {
  const keys: ReadonlySet<string> = new Set([...left.keys(), ...right.keys()]);
  return {
    evaluate: (getValue) =>
      kind === "and" ? left.evaluate(getValue) && right.evaluate(getValue) : left.evaluate(getValue) || right.evaluate(getValue),
    keys: () => keys,
  };
}

function isIdentStart(ch: string): boolean {
  return (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z") || ch === "_";
}

function isIdentPart(ch: string): boolean {
  return isIdentStart(ch) || (ch >= "0" && ch <= "9") || ch === "." || ch === "-" || ch === "/";
}

function tokenize(input: string): readonly Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < input.length) {
    const ch = input.charAt(i);
    if (ch === " " || ch === "\t" || ch === "\n" || ch === "\r") {
      i += 1;
      continue;
    }
    if (ch === "(") {
      tokens.push({ kind: "lparen", offset: i });
      i += 1;
      continue;
    }
    if (ch === ")") {
      tokens.push({ kind: "rparen", offset: i });
      i += 1;
      continue;
    }
    if (ch === "!") {
      if (input.charAt(i + 1) === "=") {
        tokens.push({ kind: "ne", offset: i });
        i += 2;
      } else {
        tokens.push({ kind: "not", offset: i });
        i += 1;
      }
      continue;
    }
    if (ch === "=") {
      if (input.charAt(i + 1) !== "=") {
        fail("expected '=='", i);
      }
      tokens.push({ kind: "eq", offset: i });
      i += 2;
      continue;
    }
    if (ch === "&") {
      if (input.charAt(i + 1) !== "&") {
        fail("expected '&&'", i);
      }
      tokens.push({ kind: "and", offset: i });
      i += 2;
      continue;
    }
    if (ch === "|") {
      if (input.charAt(i + 1) !== "|") {
        fail("expected '||'", i);
      }
      tokens.push({ kind: "or", offset: i });
      i += 2;
      continue;
    }
    if (ch === "'") {
      const end = input.indexOf("'", i + 1);
      if (end === -1) {
        fail("unterminated string literal", i);
      }
      tokens.push({ kind: "string", value: input.slice(i + 1, end), offset: i });
      i = end + 1;
      continue;
    }
    if (isIdentStart(ch)) {
      let j = i + 1;
      while (j < input.length && isIdentPart(input.charAt(j))) {
        j += 1;
      }
      tokens.push({ kind: "ident", name: input.slice(i, j), offset: i });
      i = j;
      continue;
    }
    fail(`unexpected character '${ch}'`, i);
  }
  return tokens;
}

/**
 * Recursive-descent parser over the token stream. Precedence, tightest
 * first: comparison and !, then &&, then ||; parentheses override.
 */
class Parser {
  private pos = 0;

  constructor(private readonly tokens: readonly Token[]) {}

  parse(): ContextKeyExpression {
    const expr = this.parseOr();
    const rest = this.peek();
    if (rest !== undefined) {
      fail("unexpected trailing input", rest.offset);
    }
    return expr;
  }

  private peek(): Token | undefined {
    return this.tokens[this.pos];
  }

  private parseOr(): ContextKeyExpression {
    let left = this.parseAnd();
    while (this.peek()?.kind === "or") {
      this.pos += 1;
      left = junction("or", left, this.parseAnd());
    }
    return left;
  }

  private parseAnd(): ContextKeyExpression {
    let left = this.parseUnary();
    while (this.peek()?.kind === "and") {
      this.pos += 1;
      left = junction("and", left, this.parseUnary());
    }
    return left;
  }

  private parseUnary(): ContextKeyExpression {
    const token = this.peek();
    if (token?.kind === "not") {
      this.pos += 1;
      return negateExpr(this.parseUnary());
    }
    return this.parsePrimary();
  }

  private parsePrimary(): ContextKeyExpression {
    const token = this.peek();
    if (token === undefined) {
      fail("expected an expression", this.tokens.length > 0 ? (this.tokens[this.tokens.length - 1]?.offset ?? 0) + 1 : 0);
    }
    if (token.kind === "lparen") {
      this.pos += 1;
      const inner = this.parseOr();
      const closing = this.peek();
      if (closing?.kind !== "rparen") {
        fail("expected ')'", closing?.offset ?? token.offset);
      }
      this.pos += 1;
      return inner;
    }
    if (token.kind === "ident") {
      this.pos += 1;
      if (token.name === "true") {
        return literal(true);
      }
      if (token.name === "false") {
        return literal(false);
      }
      const operator = this.peek();
      if (operator?.kind === "eq" || operator?.kind === "ne") {
        this.pos += 1;
        const value = this.peek();
        if (value?.kind !== "string") {
          fail("expected a quoted string value", value?.offset ?? operator.offset);
        }
        this.pos += 1;
        return comparison(token.name, value.value, operator.kind === "ne");
      }
      return keyTest(token.name);
    }
    fail("expected a key, '(', 'true', or 'false'", token.offset);
  }
}

/** The when-expression entry point: parse a string into an evaluable expression. */
export const ContextKeyExpr = {
  /**
   * Parses `when`. Returns the expression on success; on failure returns
   * a ParseError value naming the problem and its offset - callers report
   * it once at registration instead of throwing at render.
   */
  deserialize(when: string): Result<ContextKeyExpression, ParseError> {
    try {
      return ok(new Parser(tokenize(when)).parse());
    } catch (failure) {
      if (failure instanceof ParseFailure) {
        return err(failure.parseError);
      }
      throw failure;
    }
  },
} as const;
