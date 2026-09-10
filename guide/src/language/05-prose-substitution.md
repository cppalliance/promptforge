# Prose Substitution

Prose blocks are not static text. When a Lua block reads its `prose` value, `{{ }}` placeholders in the pending Markdown are replaced with live values from the run. This chapter teaches the substitution language: the namespaces, the path syntax, the escapes, and the errors. It is a small language, and learning it well keeps your prompts honest, because substitution never computes anything.

## The namespaces

Each placeholder names a namespace and, for most of them, a key:

- `{{ args }}` inserts the run's input string.
- `{{ item }}` inserts the current member when the section runs as an arm of a fanout.
- `{{ var.key }}` inserts a field of the `var` clipboard.
- `{{ sys.key }}` inserts runtime metadata.
- A bare name, such as `{{ kind }}`, inserts a section-local Lua global.

So `hi {{ args }}!` with the run argument `Acme Corp` reads as `hi Acme Corp!`.

## Dotted paths and structured values

Dotted paths index into nested values. With `var.row = { a = 1 }`, the placeholder `{{ var.row.a }}` renders `1`. A placeholder that resolves to a whole table or array renders as compact JSON, so `{{ var.row }}` renders `{"a":1}`.

## Escapes

To emit a literal `{{`, `}}`, or backslash, escape it with a backslash. The text `\{{ args }}` renders as the literal characters `{{ args }}`.

## One pass, no arithmetic

Substitution is a single pass over prose only. Replacement output is never rescanned, so a substituted value that happens to contain `{{ }}` stays literal. No arithmetic is performed: compute in Lua, keep the result in `var` or a global, and reference it. Lua source is never substituted.

Substitution is also lazy. It runs on the first read of `prose`, not at block entry, so a placeholder that would fail costs nothing in a block that never reads its prose.

## Hard errors

Substitution failures are ordinary Lua errors raised at the read site, with specific messages, so a block can catch them with `pcall`. The failures cover an unknown namespace or global, a missing key, a null value, a bare `{{ var }}` or `{{ sys }}`, dotted indexing into a string, an unclosed `{{`, empty path segments, and non-JSON globals.

One placeholder has a precondition. Using `{{ item }}` outside a fanout arm is an error because no collection member exists.

## How item renders

Inside a fanout arm, `{{ item }}` renders the current collection member by type. Strings render verbatim. Numbers and booleans render in natural string form, so `1.5` renders as `1.5` and `true` as `true`. Arrays and objects render as compact JSON.
