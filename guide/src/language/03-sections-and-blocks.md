# Sections and Blocks

You have seen the run from the outside. This chapter goes inside a section and teaches the pieces you write there: the exact fence forms, the thematic break that resets prose capture, list sections, and the shared library. These are the parts you touch in every prompt, so it pays to learn their exact shapes now.

## The two fence forms

A Lua block opens with an exact, unindented fence line. Only two forms are valid:

````markdown
```lua
var.greeting = "hello"
```
````

````markdown
```lua shared
function shout(s)
  return s:upper()
end
```
````

The marker is recognized only as an exact, unindented opening line. Near-miss forms, and marker-looking lines nested inside longer code blocks, stay in prose. An unclosed fence is a parse error that names the phase. The removed `lua prompt` form fails with a fence error naming the two valid forms.

## The shared library

One `lua shared` fence in the H1 body defines a shared library. It is replayed as every section's first chunk, so its functions and globals are available in every section and every fanout arm. Because each section gets its own Lua state, the shared library is how you give every section the same helpers without repeating them.

## The thematic break

A `---` rule inside a section is a thematic break, and it does exactly one thing: it resets the pending prose buffer. Prose above the break is excluded from the next Lua block's `prose` value, and only the Markdown below the break is captured:

````markdown
## Draft

Working notes the model should never see.

---

Write the actual instructions here.

```lua
return models.infer(prose)
```
````

A break carries no control-flow meaning. It never ends a section, skips a section, or stops a call, and everything below it - Lua fences included - parses and runs normally. Use breaks to keep commentary inside a section without letting it leak into `prose`.

One formatting rule matters here. A blank line must precede a `---` rule. A prose line directly followed by `---` parses as a setext heading underline, not a rule, so `Some prose` immediately followed by `---` becomes a new section named `Some prose`.

## List sections

A section with no Lua blocks whose every nonblank prose line is a list marker is a list section. Its items are pre-parsed at load time:

````markdown
## Topics

- alpha
- beta
````

The item markers are `- `, `* `, `N. `, and `N) `. Blank lines are ignored. Empty items, non-list content, and empty list sections are parse errors.

From Lua, `list_from_section(heading)` returns a visible list section's items as an array of strings, with the bullet and number markers stripped. Over the list above, `list_from_section('## Topics')` yields the strings `alpha` and `beta`.

## Naming a section exactly

Calls such as `jump`, `call`, `fanout`, and `list_from_section` take a heading reference. Write it exactly: one or more `#` markers, whitespace, then a non-empty name. Forms like `###Name` with no whitespace, or a bare name with no markers, are rejected, so a malformed heading can never be silently reinterpreted.

## The frozen preamble

Remember that the H1 pass runs first with full host access. The tool and model bindings declared there become structurally frozen for the rest of the run: once the preamble's Lua state is gone, nothing can add or change a binding. Sections select from what the preamble declared; they do not declare their own.
