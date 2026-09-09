# YAML gotchas

DSL files are YAML. Every `${…}` expression sits inside a YAML
scalar, and YAML's plain-scalar rules mean a handful of characters
can silently terminate or reshape a value before it reaches the
expression engine. The failure mode is almost always **silent
misparse**: the file "loads," the step runs, the value on the wire
is wrong. If you're seeing "why did this step run with the wrong
value" and no error near the offending line, this page is where to
look first.

**When in doubt, quote.** Double-quoting a `${…}` scalar
(`x: "${…}"`) neutralises every trap listed here. `dsl-lint` warns
on the most common one (`: ` inside an unquoted `${…}` — see issue
#91) but the safe habit is broader than the automated check.

## `: ` (space + colon + space) — the ternary trap

**Trap:**

```yaml
step:
  assign:
    x: ${a ? b : c}
```

YAML terminates the plain scalar at the first `: ` (space + colon
+ space — the mapping-value indicator). Depending on the parser,
`x` becomes `${a ? b` and the rest is either a broken mapping key
or a hard parse error.

**Fix:**

```yaml
step:
  assign:
    x: "${a ? b : c}"
```

`dsl-lint` flags this shape: `line N: unquoted ${...} scalar
contains ': '`.

## `, ` (comma + space) inside a flow context

**Trap:**

```yaml
step:
  assign: { x: ${format(a, b)}, y: 42 }
```

Inside a flow-mapping (curly braces) or flow-sequence (square
brackets), a bare `,` terminates the current element. The `,` in
`format(a, b)` looks like the boundary between the `x:` value and
the `y:` key.

**Fix:**

```yaml
step:
  assign: { x: "${format(a, b)}", y: 42 }
```

Or reshape into block style, which has no flow-context comma:

```yaml
step:
  assign:
    x: ${format(a, b)}
    y: 42
```

## `#` — comment start after a space

**Trap:**

```yaml
step:
  assign:
    hash: ${some_expr} # note the hash prefix
```

YAML treats ` #` (space + hash) as a comment start. If your
expression itself contains `#` at a word boundary, the parser cuts
the value at that point. In practice this rarely bites inside
`${…}` (rare character), but if you're building a string that
should INCLUDE a `#`, quote it.

**Fix:**

```yaml
step:
  assign:
    hash: "${some_expr}"
    also: "value #with a hash"
```

## Special characters at scalar start

The following characters at the very start of a plain scalar are
YAML metasyntax and change how the value is parsed:

- `[` `{` — start of a flow sequence / mapping. `x: [foo]` becomes
  a one-element list, not the string `"[foo]"`.
- `!` — tag prefix. `x: !something` fails with "unknown tag."
- `|` `>` — block-scalar indicators. `x: |` starts a multi-line
  literal block, absorbing subsequent indented lines.
- `&` `*` — anchor / alias. `x: &foo` declares an anchor.
- `'` `"` — start of a quoted scalar; if the closing quote is
  missing, parse fails.
- `%` — directive marker (only meaningful at document start; but
  interpreted specially by some tooling).
- `@` `` ` `` — reserved for future use by the YAML spec.

**Fix:** if your value legitimately starts with any of these,
quote:

```yaml
x: "[not a list]"
y: "|literal-pipe"
z: "*ref"
```

## `\n` and multi-line scalars

**Trap:**

```yaml
step:
  return:
    body: ${some_long_expression}    # single line, safe
    also: ${first_line}
          ${second_line}             # DOES NOT do string
                                     # concatenation — second line
                                     # is a new plain-scalar
                                     # continuation of `also`,
                                     # and its indentation matters.
```

YAML plain scalars can span multiple lines, but continuation lines
must be indented more than the key's indent, and blank lines
between them fold. This is rarely what you want inside a `${…}`.

**Fix:** put the whole expression on one line and quote it, or use
a block scalar (`|` for literal, `>` for folded):

```yaml
step:
  return:
    body: |
      first line
      second line
```

## Unicode homoglyphs

**Trap:** copy-pasted YAML sometimes carries a Unicode `:` (`：`, U+FF1A)
or `-` (`–`, U+2013) that looks like ASCII but doesn't parse.

**Fix:** open the file in an editor that highlights non-ASCII
whitespace / punctuation. `hexdump -C file.yml | grep <suspect-line>`
if in doubt.

## Summary

- **When in doubt, quote.** `x: "${…}"` is safe.
- `dsl-lint` catches the `: ` ternary case programmatically.
- Prefer block style over flow style (`{ … }` / `[ … ]`) in DSL
  authoring — block style has fewer YAML metasyntax hazards, and
  matches the sample DSLs shipped under `DSL/samples/`.
