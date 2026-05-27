# Configuration reference (v0.3)

`inclean` is driven by a single `inclean.toml` at the project root.
This document is the reference for the schema, matching model, and
execution behavior. For an end-to-end walkthrough see the
[README](../README.md); for code-level architecture see
[architecture.md](architecture.md).

> **Editor support.** A JSON Schema lives at
> [`schemas/inclean.toml.schema.json`](../schemas/inclean.toml.schema.json) and
> is hosted on `raw.githubusercontent.com` for editors that consume
> the `#:schema` directive.

## Table of contents

- [The `[project]` block](#the-project-block)
- [Rules and `copied_from`](#rules-and-copied_from)
- [The four matching layers](#the-four-matching-layers)
  - [Layer 1 — `file_paths`](#layer-1--file_paths)
  - [Layer 2 — `file_suffixes`](#layer-2--file_suffixes)
  - [Layer 3 — `match_forms`](#layer-3--match_forms)
  - [Layer 4 — `include_match`](#layer-4--include_match)
- [`suppression_comments_regex`](#suppression_comments_regex)
- [Actions](#actions)
- [Trailing comments](#trailing-comments)
- [`${copied}` and other placeholders](#copied-and-other-placeholders)
- [`@std.*` built-in constants](#std-built-in-constants)
- [Conflict detection](#conflict-detection)
- [CLI](#cli)
- [Exit codes](#exit-codes)

## The `[project]` block

```toml
[project]
root = "."                            # optional; defaults to "."
version = "0.3.0"                     # required
min_inclean_version = "0.3.0"         # required
```

- `root` — project root path, relative to the directory containing
  `inclean.toml`. All path-shaped rule fields resolve relative to the
  **resolved** root.
- `version` — CLI version that wrote this config. Written once by
  `inclean init` and never auto-updated.
- `min_inclean_version` — lowest CLI version that can correctly parse
  this config. Same lifecycle as `version`.

Compatibility check (both must hold):

- `CLI_COMPAT_MIN <= config.version` — the config wasn't written for a
  CLI older than the last breaking schema change.
- `config.min_inclean_version <= CLI_CURRENT` — this CLI is new enough.

Either side failing aborts the run with a path-aware error.

## Rules and `copied_from`

A rule is `[[rule]]`:

```toml
[[rule]]
name = "base"
# layer 1–4 and action go here
```

`name` is required and globally unique across the config.

`copied_from = "other-rule"` performs a **single-level copy** (transitive
— the parent's already-resolved value is taken):

- **Top-level field omitted** by the child → inherit the parent's
  resolved value.
- **Top-level field written** by the child → replace the field; inside a
  nested object the inner fields default to schema defaults (null /
  disabled). Use `${copied}` per inner field to pull the parent's value
  explicitly.

The referenced parent must be declared earlier in the config
(forward-only references). Self-references are rejected.

## The four matching layers

For a rule to fire on an `#include`, every layer must pass.

### Layer 1 — `file_paths`

```toml
file_paths = ["src/**/*", "include/**/*"]
# Default: ["**/*"]
```

Globset patterns interpreted relative to the project root. **Full-string
anchored**: `foo.h` matches only the literal `foo.h`, **not** `a/foo.h`.
Use `**/foo.h` to match at any depth. `*` does not cross `/`; only `**`
does.

### Layer 2 — `file_suffixes`

```toml
file_suffixes = [".c", ".h"]
# Default: ["@std.c.extensions", "@std.cpp.extensions"]
```

Literal file extensions. Skipped when the matching `file_paths` glob is
an exact literal path (no wildcards).

### Layer 3 — `match_forms`

```toml
match_forms = ["quote", "angle"]
# Default: ["quote"]
```

The set of `#include` syntaxes the rule applies to:
- `"quote"` — `#include "foo.h"`
- `"angle"` — `#include <foo.h>`
- `"macro"` — `#include MY_HEADER`

Matching `"macro"` is valid in config; **action evaluation against a
macro `#include` always produces an error in v1**.

### Layer 4 — `include_match`

```toml
include_match = ["old_*.h", "legacy/**"]
# Default: ["**"]
```

Globset patterns matched against the include's argument text (no
quotes/angles). Same anchoring rules as Layer 1.

## `suppression_comments_regex`

Marks regions where `inclean` should leave includes alone.

```toml
suppression_comments_regex = {
    block_start = "^USER CODE BEGIN.*$",
    block_end   = "^USER CODE END.*$",
    line        = "^inclean: skip$",
}
```

For each physical line, the engine extracts its comment body (stripping
`//` or same-line `/* */` delimiters and surrounding whitespace) and
matches it against these regexes:

- `line` — matches a single line; if it matches, only that line is
  off-limits.
- `block_start` / `block_end` — once `block_start` matches, every line
  through the next `block_end` (inclusive) is off-limits.

Per-rule. Different rules may suppress different regions of the same
file.

## Actions

Exactly one `action = { type = "...", ... }` per rule (default: `keep`).

### `resolve`

```toml
action = { type = "resolve", relative_to = "include", output_form = "quote" }
```

Probe each entry of `include_directories` (literal directory paths under
the project root) for the include text; if exactly one entry contains the
file, rewrite the path to be relative to `relative_to`. Multiple matches
or no matches → `EvaluationFailure`.

`relative_to`:
- A literal directory path (relative to the project root), or
- `${current_file}` — meaning "the directory of the file being edited".

### `replace`

```toml
action = { type = "replace", with = "lib/${original}" }
```

Substitute the include's argument with `with` (literal text +
placeholders). `output_form` may flip the delimiters.

### `keep`

```toml
action = { type = "keep", output_form = "angle" }
```

Leave the argument unchanged. `output_form` may still rewrite `"foo.h"`
to `<foo.h>` or vice versa. Default: `preserve`.

### `remove`

```toml
action = { type = "remove", keep_blank_line = false, keep_trailing_comment = true }
```

Delete the whole include line.

- `keep_blank_line` (default `false`) — leave an empty line in place.
- `keep_trailing_comment` (default `true`) — when the line had a
  trailing comment, keep that comment on its own line.

### `comment_out`

```toml
action = { type = "comment_out", style = "//" }
```

Wrap the whole include line in a `//` (default) or `/* */` (`style = "/**/"`)
comment, preserving the original indentation and line terminator.

### `error`

```toml
action = { type = "error", message = "use lib/foo.h instead" }
```

Surface a user-facing error for any matched include. Exit code 2.

## Trailing comments

```toml
trailing_comment = {
    transform = {
        match_styles  = ["//", "/**/"],
        content_regex = "^TODO.*$",
        action        = { type = "replace", with = "FIXED" },
    },
    append_if_absent = "  // IWYU pragma: export",
}
```

`trailing_comment.transform` runs first:
- `match_styles` — comment delimiter styles the transform applies to.
- `content_regex` — regex over the comment body (delimiters stripped,
  trimmed). Default `".*"` (matches all).
- `action` — one of `replace { with, output_style? }`, `keep {
  output_style? }`, `remove`, `error { message }`.

After the transform runs (or if no transform was configured),
`append_if_absent` is appended verbatim when the include has no trailing
comment. **You write the full text — delimiters and leading whitespace
included.**

Trailing-comment processing applies to `resolve` / `replace` / `keep`.
`remove`, `comment_out`, and `error` ignore it.

## `${copied}` and other placeholders

`${copied}` resolves at config-load time (not at action-evaluation time)
and may appear in three contexts inside a rule that has `copied_from`:

1. **Scalar context** — a string field's entire value is `"${copied}"`.
   The field inherits the parent's resolved scalar value verbatim.
   Example: `relative_to = "${copied}"` inside a child's `action`.

2. **Array-element context** — an element equal to `"${copied}"` inside
   a string list is a splat: the parent's entire array expands in place.
   Empty / unset parent lists splat to zero elements. Example:
   `file_suffixes = ["${copied}", ".inl"]` extends the parent's list.

3. **Object context** — the whole top-level field is the string
   `"${copied}"`, inheriting the parent's entire resolved object.
   Valid only on `action`, `suppression_comments_regex`, and
   `trailing_comment`. Example:
   ```toml
   [[rule]]
   name = "child"
   copied_from = "base"
   trailing_comment = "${copied}"
   ```
   The child's `trailing_comment` is identical to `base`'s resolved
   trailing_comment.

If a rule lacks `copied_from`, any `${copied}` use is a config error.

Other placeholders (applied at action-evaluation time, on the matched
include):

- `${current_file}` — project-relative path of the file being edited
  (forward-slash separated).
- `${original}` — the original include argument (in an action template)
  or the original trailing-comment body (in a trailing-comment template).
- `$$` — literal `$` inside a substitution string.

## Glob anchoring (footgun)

Every glob field — `file_paths`, `include_match`, internal matchers —
is **full-string anchored**, matching `globset` / `.gitignore`
semantics:

- `foo.h` matches ONLY the literal `foo.h`, not `a/foo.h`.
- Use `**/foo.h` to match `foo.h` at any depth.
- `*` does NOT cross `/`; only `**` does.

`include_directories` is **NOT a glob** — its entries are literal
directory paths under the project root. No `**` expansion; no
`.gitignore` semantics.

## `append_if_absent` line-terminator rule

`trailing_comment.append_if_absent` must not contain `\n` or `\r`. It's
appended onto the same physical line as the include; an embedded line
terminator would silently mangle CRLF↔LF behavior on different
platforms. Config-check rejects newlines in the field at load time.

## `@std.*` built-in constants

Constants are available in string lists (as `"@name"` elements, splatted
into the surrounding list) and in regex/template strings (substituted
in place).

File extensions: `@std.c.extensions`, `@std.cpp.extensions`,
`@std.c.header_extensions`, `@std.c.source_extensions`,
`@std.cpp.header_extensions`, `@std.cpp.source_extensions`.

C system headers (cumulative): `@std.c89.system_headers` through
`@std.c23.system_headers`.

C++ system headers (cumulative): `@std.cpp.c_compat_headers`,
`@std.cpp98.system_headers` through `@std.cpp23.system_headers`.

The `_or` suffix on any list constant materialises it as a regex
alternation: `@std.c89.system_headers_or` becomes
`(?:stdio\.h|stdlib\.h|...)`. Useful inside regex strings.

## Conflict detection

When multiple rules match the same `#include`:

1. Every matched rule's action is evaluated.
2. Each rule produces a "final-line text" (the bytes that rule would
   have written for that include).
3. If all rules' final-line texts are identical (including all-`Keep`),
   there is no conflict and the rewrite (or no-op) goes through.
4. If any rule produces an `error` action result, that wins.
5. If any rule produces an evaluation failure (e.g. `resolve` ambiguous),
   that wins next.
6. Otherwise, the include is reported as a `Conflict` with each rule's
   final-line text included in the diagnostic.

## CLI

```sh
inclean init [PATH]                                          # alias of `config new`
inclean check                                                # alias of `check all`
inclean check config    [-c PATH]
inclean check unfixable [-c PATH] [-j N] [PATHS...]
inclean check all       [-c PATH] [-j N] [PATHS...]
inclean apply           [-c PATH] [-j N] [PATHS...]
inclean diff [-o PATH]  [-c PATH] [-j N] [PATHS...]
inclean config check    [-c PATH]                            # alias of `check config`
inclean config new [PATH]                                    # alias of `init`
inclean config schema [-o PATH] [--check]
```

`check` subcommands:
- `config` — validate the config file only (no source files opened).
- `unfixable` — only report errors / evaluation failures / conflicts.
- `all` — report every per-include outcome. Bare `inclean check` runs this.

`-c PATH` overrides the upward `inclean.toml` walk. `-j N` sets the
worker thread count (default: CPU count). `PATHS...` restricts
processing to source files rooted at any of the given paths.

`apply` and `diff` pre-flight a config-only check; failures surface with
a `config-only check failed:` prefix. Apply does **not** abort when a
run produces unfixable violations: fixable files are written normally;
files containing any unfixable outcome are skipped (no partial writes),
and the unfixable details are printed at the end. The exit code is
non-zero iff any unfixable violation surfaced.

The unfixable report's kinds:

- `error` — a rule's `action = { type = "error" }` matched.
- `evaluation_failure` — `resolve` found zero or multiple matches,
  or a similar runtime failure.
- `conflict` — multiple rules matched the same include and produced
  different final-line texts; the report lists each rule, its final
  text, and a `differs in:` line naming the parts (include path /
  output_form / trailing_comment).
- `trailing_comment_error` —
  `trailing_comment.transform.action = { type = "error" }` matched.

## Exit codes

- `0` — clean.
- `1` — CLI / config error (missing required field, malformed semver,
  IO error, etc.).
- `2` — any rule's `action.type = "error"` matched.
- `3` — any conflict (rules disagreed on final text) or any
  `EvaluationFailure` (e.g. `resolve` ambiguity).
