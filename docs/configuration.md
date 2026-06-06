# inclean Configuration Syntax

_English | [简体中文](configuration.zh-CN.md)_

`inclean` reads a TOML file named `inclean.toml`. By default the CLI walks
upward from the current directory to find it; `-c PATH` selects a specific
file. Unknown fields are rejected.

```toml
#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v1.2.3/schemas/inclean.toml.schema.json

[project]
root = "."
version = "1.2.3"
min_inclean_version = "1.1.0"

[[rule]]
name = "rewrite-public-headers"
file_paths = ["src/**/*"]
include_forms = ["quote"]
include_match = ["**/*.h"]
include_directories = ["include"]
action = { type = "resolve", relative_to = "include", output_form = "quote" }
```

## File Shape

An `inclean.toml` has one `[project]` table and repeated `[[rule]]` tables.
Rules are resolved and evaluated in declaration order. More than one rule may
match the same `#include`; `inclean` compares the final text each matching rule
would produce and reports a conflict when they disagree.

`action` edits the include path/form. `trailing_comment` edits the same-line
trailing comment. These two dimensions are compared separately for conflicts.
The whole-field value `"skip"` opts that dimension out of conflict detection;
a `keep` action still participates because it contributes an explicit outcome.

## `[project]`

| Field                 | Required | Default | Meaning                                                                                                                            |
| --------------------- | -------- | ------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `root`                | no       | `"."`   | Project root, relative to the config file. All rule paths are relative to this resolved root. Must point to an existing directory. |
| `version`             | yes      | none    | Config schema version written by the CLI that generated the file.                                                                  |
| `min_inclean_version` | yes      | none    | Oldest CLI version expected to parse this config correctly.                                                                        |

`#:schema` is optional and only helps editor tooling. The CLI uses the
`version` fields above for its own compatibility check.

## Rule Matching

A rule matches only when all configured layers pass:

1. `file_paths` and `file_suffixes` select the source file.
2. `suppression_comments_regex` may suppress source lines for this rule.
3. `include_forms` selects quote, angle, or macro includes.
4. `include_match` matches the stripped include argument.
5. If `include_directories` is non-empty, directory resolution selects a real
   header and `include_resolved_match` filters that resolved path.

Shared syntaxes are documented once: [glob syntax](#glob-syntax),
[placeholders](#placeholders), and [built-in constants](#constants).

<a id="glob-syntax"></a>

### Glob Syntax

`file_paths`, `include_match`, and `include_resolved_match` use ordered signed
glob lists.

- Patterns are full-string anchored: `foo.h` matches only `foo.h`; use
  `**/foo.h` for any depth.
- `*` does not cross `/`; `**` does.
- A leading unescaped `!` negates a pattern.
- Later matching patterns override earlier ones.
- Escape a literal leading bang as `'\!weird.h'` in TOML single quotes, or
  `"\\!weird.h"` in TOML double quotes.
- `include_directories` is not a glob list. Its entries are literal
  directories under the project root.

### Rule Identity And Copying

| Field         | Required | Default | Meaning                                                                                   |
| ------------- | -------- | ------- | ----------------------------------------------------------------------------------------- |
| `name`        | yes      | none    | Globally unique rule name used in diagnostics.                                            |
| `copied_from` | no       | none    | Name of an earlier rule to inherit from. Forward references and self-copies are rejected. |

When `copied_from` is set, omitted top-level fields inherit the parent's
already resolved value. Written top-level fields replace the whole field.
For object-valued fields, omitted inner fields reset to their defaults instead
of inheriting. Use `${copied}` when you want explicit inheritance:

```toml
[[rule]]
name = "base"
file_paths = ["src/**/*"]
include_match = ["**"]
action = { type = "resolve", relative_to = "include" }

[[rule]]
name = "only-public"
copied_from = "base"
include_match = ["${copied}", "!private/**"]
action = { type = "resolve", relative_to = "${copied}", output_form = "quote" }
```

Valid `${copied}` forms:

| Location           | Syntax                      | Meaning                                                                                                                                  |
| ------------------ | --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Whole object field | `action = "${copied}"`      | Copy the whole parent object. Valid for `action`, `trailing_comment`, and `suppression_comments_regex`.                                  |
| Array element      | `["${copied}", "!x/**"]`    | Splice the parent list at that position.                                                                                                 |
| Inner string field | `relative_to = "${copied}"` | Copy the parent's scalar value for that inner field. The parent action must be the same variant when variant-specific fields are copied. |

`${copied}` is evaluated during copy resolution. It is separate from runtime
[placeholders](#placeholders).

### File Selection

| Field           | Default                                        | Meaning                                                                                      |
| --------------- | ---------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `file_paths`    | `["**/*"]`                                     | Ordered signed [glob list](#glob-syntax) matched against project-root-relative source paths. |
| `file_suffixes` | `["@std.c.extensions", "@std.cpp.extensions"]` | Literal extensions, including the leading dot. May use [built-in constants](#constants).     |

An exact literal `file_paths` match skips `file_suffixes`; a wildcard match
must also pass `file_suffixes`.

### Suppression Regions

`suppression_comments_regex` marks source lines that this rule must not edit.
It can be omitted, an object, or the whole-field string `"${copied}"`.

```toml
suppression_comments_regex = {
    block_start = "^inclean: off$",
    block_end = "^inclean: on$",
    line = "^inclean: skip$",
}
```

| Inner field   | Meaning                                                                              |
| ------------- | ------------------------------------------------------------------------------------ |
| `line`        | Suppresses only lines whose probe text matches the regex.                            |
| `block_start` | Starts an off-limits block on the matching line.                                     |
| `block_end`   | Ends the block on the matching line. If omitted, the block continues to end of file. |

For matching, `inclean` strips a leading `//` or same-line `/* ... */`
delimiter when present, trims whitespace, and applies the regex to that text.
Non-comment lines are matched as trimmed raw text. Regex strings may use
[built-in constants](#constants).

### Include Matching

| Field           | Default         | Meaning                                                                                                                                                                        |
| --------------- | --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `include_forms` | `["quote"]`     | Include forms this rule matches: `"quote"` for `#include "x.h"` or `#define X "x.h"`, `"angle"` for `<x.h>`, and `"macro"` for unexpanded `#include X`.                       |
| `macro_rewrite` | `"definitions"` | Expanded macro rewrite target: `"definitions"` edits matching `#define` header tokens; `"use_site"` edits the `#include MACRO` argument and requires all branches to agree.    |
| `include_match` | `["**"]`        | Ordered signed [glob list](#glob-syntax) over the include argument with delimiters stripped, for example `mylib/foo.h`.                                                        |

`#include MACRO` is handled specially. `inclean` scans rule-eligible source
files for simple object-like definitions whose replacement is exactly one
header token:

```c
#define MY_HEADER "foo.h"
#define SYS_HEADER <foo.h>
```

Every header-like definition with the same macro name is treated as a possible
branch. Matching and `${current_file}` use the `#include MACRO` use-site
context; the definition provides only the effective quote/angle form, path, and
editable range. With the default `macro_rewrite = "definitions"`, each matched
branch writes its own `#define` value. With `macro_rewrite = "use_site"`, the
`#include MACRO` argument is rewritten instead, and all matched branches must
produce the same final include argument. Unexpanded macro includes can still
match `include_forms = ["macro"]`, but non-`skip` actions report an error.

### Header Resolution

| Field                    | Default   | Meaning                                                                                                                                                                    |
| ------------------------ | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `include_directories`    | `[]`      | Literal directory paths under the project root. Not [globs](#glob-syntax), no implicit `/**`, and no `.gitignore` semantics.                                               |
| `include_resolved_match` | `["**"]`  | Ordered signed [glob list](#glob-syntax) matched against the resolved header path, project-root relative when possible. Has no effect when `include_directories` is empty. |
| `include_on_unresolved`  | `"error"` | Policy when no candidate is found after `include_resolved_match`: `"error"`, `"skip"`, or `"allow"`.                                                                       |
| `include_on_ambiguous`   | `"error"` | Policy when multiple include directories resolve the same include argument: `"error"`, `"skip"`, or `"first"`.                                                             |

Resolution probes each directory as:

```text
<project root>/<include_directories item>/<include argument>
```

`include_on_unresolved = "allow"` keeps the rule matched without a resolved
header, which is useful for non-`resolve` actions. It is invalid with
`action = { type = "resolve", ... }`. `include_on_ambiguous = "first"` selects
the first matching candidate in `include_directories` order.

<a id="placeholders"></a>

## Placeholders

Runtime placeholders are expanded in action and trailing-comment template
strings after a rule matches:

| Placeholder       | In action templates                                               | In trailing-comment templates                                        |
| ----------------- | ----------------------------------------------------------------- | -------------------------------------------------------------------- |
| `${current_file}` | Project-root-relative path of the file being edited.              | Project-root-relative path of the file being edited.                 |
| `${original}`     | Original include argument with quotes or angle brackets stripped. | Original trailing-comment body with delimiters stripped and trimmed. |

Supported action template fields include `action.relative_to`, `action.with`,
and `action.message`. Supported trailing-comment template fields include
`trailing_comment.transform.action.with` and `.message`.

## Actions

`action` can be `"skip"`, `"${copied}"`, or a tagged object. If neither a rule
nor its copied ancestors set `action`, the effective value is `"skip"`.

Shared values:

- `output_form`: `"quote"`, `"angle"`, or `"preserve"`; default
  `"preserve"` where supported.
- `message`: optional string accepted by action variants. `error` uses it as
  the user-facing error text. Other variants currently do not change rewrite
  text with it.
- Action strings may use [placeholders](#placeholders) and
  [built-in constants](#constants).

| Action      | Syntax                                                                       | Effect                                                                                                                                                                   |
| ----------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| skip        | `action = "skip"`                                                            | Rule may still match and run `trailing_comment`, but contributes no action candidate.                                                                                    |
| copy        | `action = "${copied}"`                                                       | Copy the whole parent action. Requires `copied_from`.                                                                                                                    |
| resolve     | `{ type = "resolve", relative_to = "...", output_form = "quote" }`           | Rewrite to the header chosen by `include_directories`, expressed relative to `relative_to`. `relative_to = "${current_file}"` means the current source file's directory. |
| replace     | `{ type = "replace", with = "lib/${original}", output_form = "quote" }`      | Replace the include argument with `with`. No header lookup is required.                                                                                                  |
| keep        | `{ type = "keep", output_form = "angle" }`                                   | Keep the include argument; optionally change quote/angle form.                                                                                                           |
| remove      | `{ type = "remove", keep_blank_line = false, keep_trailing_comment = true }` | Delete the whole include line. By default no blank line is kept and a recognized trailing comment is preserved on its own line.                                          |
| comment_out | `{ type = "comment_out", style = "//" }`                                     | Comment out the whole include line. `style` is `"//"` or `"/**/"`; default `"//"`.                                                                                       |
| error       | `{ type = "error", message = "..." }`                                        | Report a configured error.                                                                                                                                               |

`resolve`, `replace`, and `keep` can be combined with `trailing_comment`.
`remove`, `comment_out`, and `error` ignore `trailing_comment`.

## Trailing Comments

`trailing_comment` can be `"skip"`, `"${copied}"`, or an object. If neither a
rule nor its copied ancestors set it, the effective value is `"skip"`.

```toml
trailing_comment = {
    transform = {
        match_styles = ["//", "/**/"],
        content_regex = "^IWYU pragma:",
        action = { type = "keep", output_style = "//" },
    },
    append_if_absent = "  // IWYU pragma: export",
}
```

Only same-line `// ...` and closed same-line `/* ... */` comments count as
trailing comments. Cross-line block comments are left alone and do not trigger
`append_if_absent`.

| Field              | Default | Meaning                                                                                                                                         |
| ------------------ | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `transform`        | none    | Optional transform applied when an existing trailing comment matches.                                                                           |
| `append_if_absent` | none    | Literal text appended when the action outcome leaves no trailing comment. Include leading whitespace and delimiters yourself. Must be one line. |

`transform` fields:

| Field           | Default          | Meaning                                         |
| --------------- | ---------------- | ----------------------------------------------- |
| `match_styles`  | `["//", "/**/"]` | Existing comment styles allowed to match.       |
| `content_regex` | `".*"`           | Regex matched against the trimmed comment body. |
| `action`        | required         | One of the trailing-comment actions below.      |

Trailing-comment actions:

| Action  | Syntax                                                             | Effect                                                    |
| ------- | ------------------------------------------------------------------ | --------------------------------------------------------- |
| replace | `{ type = "replace", with = "IWYU: export", output_style = "//" }` | Replace the comment body.                                 |
| keep    | `{ type = "keep", output_style = "preserve" }`                     | Keep the comment body; optionally change delimiter style. |
| remove  | `{ type = "remove" }`                                              | Remove the trailing comment.                              |
| error   | `{ type = "error", message = "..." }`                              | Report an unfixable trailing-comment error.               |

`output_style` is `"//"`, `"/**/"`, or `"preserve"`; default is
`"preserve"`. Trailing-comment template strings may use
[placeholders](#placeholders) and [built-in constants](#constants).

<a id="constants"></a>

## Built-in Constants

Built-in constants start with `@std.`.

In `file_suffixes`, an item that is exactly `"@name"` spreads the constant's
list into the surrounding list:

```toml
file_suffixes = ["@std.c.extensions", "@std.cpp.extensions"]
```

In scalar string fields such as suppression regexes and action or
trailing-comment templates, `@name` is substituted into the string. List
constants become a regex alternation with escaped items. Use `@@` for a literal
`@`.

`include_match` and `include_resolved_match` are glob lists and do not currently
expand `@std.*` constants.

Available list constants:

| Constant                                                                                                                                                                     | Meaning                                                           |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `@std.c.header_extensions`                                                                                                                                                   | `.h`                                                              |
| `@std.c.source_extensions`                                                                                                                                                   | `.c`                                                              |
| `@std.c.extensions`                                                                                                                                                          | C header and source extensions.                                   |
| `@std.cpp.header_extensions`                                                                                                                                                 | `.hh`, `.hpp`, `.hxx`, `.h++`                                     |
| `@std.cpp.source_extensions`                                                                                                                                                 | `.cc`, `.cpp`, `.cxx`, `.c++`, `.inl`, `.ipp`                     |
| `@std.cpp.extensions`                                                                                                                                                        | C++ header and source extensions.                                 |
| `@std.c89.system_headers`, `@std.c95.system_headers`, `@std.c99.system_headers`, `@std.c11.system_headers`, `@std.c17.system_headers`, `@std.c23.system_headers`             | C standard library header sets, cumulative by standard version.   |
| `@std.cpp98.system_headers`, `@std.cpp11.system_headers`, `@std.cpp14.system_headers`, `@std.cpp17.system_headers`, `@std.cpp20.system_headers`, `@std.cpp23.system_headers` | C++ standard library header sets, cumulative by standard version. |
| `@std.cpp.c_compat_headers`                                                                                                                                                  | C-compatible C++ header names such as `cstdio` and `cstdlib`.     |

For regex strings, any list constant can also be used as `@name_or` to request
an alternation explicitly, for example `@std.cpp17.system_headers_or`.

## Common Patterns

Rewrite includes so consumers only need `-I include`:

```toml
[[rule]]
name = "public-headers"
file_paths = ["src/**/*", "include/**/*"]
include_forms = ["quote"]
include_match = ["**/*.h"]
include_directories = ["include"]
action = { type = "resolve", relative_to = "include", output_form = "quote" }
```

Prefix selected includes without resolving them:

```toml
[[rule]]
name = "lib-prefix"
file_paths = ["src/**/*"]
include_match = ["foo.h", "bar.h"]
action = { type = "replace", with = "lib/${original}" }
```

Keep standard library includes untouched:

```toml
[[rule]]
name = "stdlib"
include_forms = ["angle"]
include_match = ["vector", "string", "memory", "utility"]
action = { type = "keep" }
```

Add or normalize a trailing comment without changing the include path:

```toml
[[rule]]
name = "export-private"
include_directories = ["include"]
include_resolved_match = ["include/private/**"]
include_on_unresolved = "skip"
action = "skip"
trailing_comment = {
    transform = { action = { type = "replace", with = "IWYU pragma: export" } },
    append_if_absent = "  // IWYU pragma: export",
}
```
