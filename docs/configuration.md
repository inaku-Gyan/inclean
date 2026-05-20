# Configuration reference

`inclean` is driven by `inclean.toml` files. This document is the
exhaustive reference for the schema, matching model, and execution
behavior. For an end-to-end walkthrough, see the
[README](../README.md); for code-level architecture, see
[architecture.md](architecture.md).

## Table of contents

- [Configuration reference](#configuration-reference)
  - [Table of contents](#table-of-contents)
  - [File layout](#file-layout)
  - [The rule tree](#the-rule-tree)
  - [The five matching layers](#the-five-matching-layers)
    - [Layer 1 — `paths` (file path glob)](#layer-1--paths-file-path-glob)
    - [Layer 2 — `extensions` (file extension)](#layer-2--extensions-file-extension)
    - [Layer 3 — `forms` (include syntax)](#layer-3--forms-include-syntax)
    - [Layer 4 — `match` (regex on include content)](#layer-4--match-regex-on-include-content)
    - [Layer 5 — `match_resolved` (resolved physical file)](#layer-5--match_resolved-resolved-physical-file)
  - [Actions](#actions)
    - [`auto`](#auto)
    - [`rewrite`](#rewrite)
    - [`keep`](#keep)
    - [`error`](#error)
  - [`${...}` placeholders](#-placeholders)
  - [`@std.*` built-in constants](#std-built-in-constants)
  - [`allowed_include_dirs` semantics](#allowed_include_dirs-semantics)
  - [`inclean check` levels](#inclean-check-levels)
  - [Exit codes](#exit-codes)

## File layout

A project has one **root** `inclean.toml` at the project root, plus any
number of optional sub-directory `inclean.toml` files that contribute
extra rules.

- **The root config must declare `[project]` with `root` set.** This
  marks it as the root and pins the project root path (typically `"."`).
- **Sub-directory configs must not declare `[project]`.** They may only
  add `[[rule]]` entries.
- All path-shaped fields in a rule (`paths`, `allowed_include_dirs`,
  `original_include_dirs`, `match_resolved.under`) are interpreted as
  **project-root-relative**, regardless of which config file the rule
  was written in.

Minimal config:

```toml
[project]
root = "."

[[rule]]
name = "base"
paths = ["src/**", "include/**"]
forms = ["quote"]
allowed_include_dirs = ["include"]
original_include_dirs = ["include/mylib/internal"]
```

## The rule tree

Rules form a **single-inheritance tree** via the optional `extends`
field, which names a parent rule. A few invariants:

- Rule **names are globally unique** across every `inclean.toml` in the
  project. Duplicate names are a load-time error.
- A rule with no `extends` is the **root** of its tree (conventionally
  named `base`). There can be any number of trees.
- Inheritance cycles are detected at load time.
- There is **no `[defaults]` block** and no project-level fallback for
  rule fields. To share defaults across rules, write a `base` rule and
  have others `extends = "base"`.

When a rule field is unspecified, the value is inherited from the
parent (recursively up the chain). When both child and parent specify a
field, the rule semantics are **AND-combined at match time**: an
include must satisfy _both_ the child's and the parent's predicate.
This is enforced by the rule-tree invariants:

- **Child ⊆ Parent.** If a child rule matches an include, every
  ancestor of that child must also match the same include.
- **Cross-chain disjoint.** Two rules that share no ancestor relation
  must not both match the same include.

Violations of either invariant are surfaced as
[conflicts](#inclean-check-levels) at `rules` level and above.

## The five matching layers

For a rule to match an `#include`, all five layers must pass. Layers
1, 2, 3 have built-in defaults if unspecified; layer 4 defaults to
"match anything"; layer 5 is opt-in.

### Layer 1 — `paths` (file path glob)

Gitignore-style globs, project-root-relative, matched against the
source file containing the `#include`. `*` does **not** cross `/`; use
`**` to match across directories.

```toml
paths = ["src/**", "include/**/*.h"]
```

If `paths` is omitted, the rule applies to every source file.

### Layer 2 — `extensions` (file extension)

Limits the rule to files with one of the listed extensions. Skipped
when layer 1 specifies an exact path (no wildcards).

Default: `["@std.c_extensions", "@std.cpp_extensions"]` — see the
[`@std.*` constants](#std-built-in-constants) section.

```toml
extensions = [".h", ".hpp", ".cpp"]
# or, using a built-in:
extensions = ["@std.cpp_extensions"]
```

### Layer 3 — `forms` (include syntax)

Restricts the rule to one or more `#include` forms:

| Value     | Matches                                |
| --------- | -------------------------------------- |
| `"quote"` | `#include "foo.h"`                     |
| `"angle"` | `#include <foo.h>`                     |
| `"macro"` | `#include FOO_HEADER` (macro-expanded) |

`"macro"` may appear in a rule and is matched, but action evaluation
on a macro-form include is always an error in v1 (placeholder text is
not expanded). This lets a rule explicitly catch and reject macro
includes.

Default: `["quote", "angle"]`.

### Layer 4 — `match` (regex on include content)

A regular expression applied to the **stripped** include content (no
quotes or angle brackets). Capture groups become available to the
action template as `${1}`, `${2}`, …

```toml
match = '^old_(.+)$'
```

`@std.*` constants may be substituted into the regex via `@name`
(`@@` for a literal `@`). See `@std.*` below.

### Layer 5 — `match_resolved` (resolved physical file)

Opt-in. When set, the include text is resolved against the rule's
`original_include_dirs` (preprocessor-style: `<dir>/<include_text>`)
and the matching is then constrained against the resolved file's
project-root-relative path.

```toml
original_include_dirs = ["src", "src/internal"]
match_resolved = { under = "src/internal" }
```

Both sub-fields are optional but at least one must be present:

- **`under`** — the resolved path must start with this directory
  (project-root-relative).
- **`match`** — a regex applied to the resolved path.

Layer 5 also enforces resolution **uniqueness**: if the include text
resolves under more than one `original_include_dirs` entry, the
include is reported as a `Layer5Ambiguous` outcome (exit code 3). The
user is expected to narrow `original_include_dirs` to disambiguate.

When layer 5 runs successfully, the action gains `${resolved.*}`
placeholders (see below).

## Actions

A rule's `action` decides what happens when an include matches. There
are four kinds, tagged by `type`. If `action` is omitted, the default
is:

```toml
action = { type = "auto", relative_to = "allowed", form = "quote" }
```

### `auto`

Resolves the include via `original_include_dirs`, then re-emits a path
relative to the chosen base.

```toml
action = { type = "auto", relative_to = "allowed", form = "quote" }
```

- **`relative_to`** — `"allowed"` (default) makes the path relative to
  the first `allowed_include_dirs` entry under which the resolved file
  lives. `"file_dir"` makes it relative to the directory of the source
  file being edited.
- **`form`** — `"quote"` (default), `"angle"`, or `"preserve"` (keep
  the original include's form).

`auto` is a hard error when the resolved file is not under any of the
rule's `allowed_include_dirs` (with `relative_to = "allowed"`); the
file's apply is aborted.

### `rewrite`

Replace the include's text with `to`, supporting `${...}`
placeholders.

```toml
action = { type = "rewrite", to = "mylib/${1}.h", form = "quote" }
```

### `keep`

Leave the include unchanged. The match still counts as matched (and
post-action validation still runs against `allowed_include_dirs`).

```toml
action = { type = "keep" }
```

### `error`

Abort processing of the file with the given message. Used to forbid
specific patterns.

```toml
action = { type = "error", message = "deprecated header: ${0}" }
```

Triggers exit code 2 (distinct from infrastructure failures, which use
exit 3).

## `${...}` placeholders

Available in `rewrite.to` and `error.message`:

| Placeholder            | Meaning                                                     |
| ---------------------- | ----------------------------------------------------------- |
| `${0}`                 | The full stripped include text                              |
| `${1}`, `${2}`, …      | Capture groups from layer-4 `match`                         |
| `${file.path}`         | Source file path (project-root-relative)                    |
| `${file.relpath}`      | Same as `${file.path}`                                      |
| `${file.dir}`          | Directory of the source file                                |
| `${resolved.path}`     | Resolved file's project-root-relative path _(layer 5 only)_ |
| `${resolved.relpath}`  | Same as `${resolved.path}`                                  |
| `${resolved.dir}`      | Directory of the resolved file                              |
| `${resolved.basename}` | Basename of the resolved file                               |
| `$$`                   | Literal `$`                                                 |

`${resolved.*}` placeholders require layer 5 to have run; using them
in a rule without `match_resolved` is an error.

## `@std.*` built-in constants

`@std.*` constants are spread into string-list fields via `@name`, or
substituted into strings (regex bodies) via the same syntax. Use `@@`
for a literal `@`.

There are two flavors:

- **List-shaped** — used in `paths`, `extensions`, etc. The `@name`
  item is replaced by the constant's elements.
- **String-shaped (via `_or` suffix)** — appending `_or` to any list
  constant turns it into a regex alternation `(?:item1|item2|...)`
  suitable for embedding in a `match` regex.

Available constants (defined in
[src/config/constants.rs](../src/config/constants.rs)):

**Extension lists**

- `std.c_header_extensions` — `.h`
- `std.c_source_extensions` — `.c`
- `std.c_extensions` — both of the above
- `std.cpp_header_extensions` — `.hh`, `.hpp`, `.hxx`, `.h++`
- `std.cpp_source_extensions` — `.cc`, `.cpp`, `.cxx`, `.c++`, `.inl`, `.ipp`
- `std.cpp_extensions` — both of the above

**C standard headers**

- `std.c89.system_headers` through `std.c23.system_headers`

**C++ standard headers**

- `std.cpp98.system_headers` through `std.cpp23.system_headers`
- `std.cpp.c_compat_headers` — C headers with a `<c…>` C++ alias

Use the `_or` suffix to turn any of the lists above into a regex
alternation, e.g. `@std.c11.system_headers_or` becomes
`(?:assert\.h|ctype\.h|…)`.

## `allowed_include_dirs` semantics

After a rule's action has been evaluated, the resulting include text
is validated by checking that it resolves under one of the rule's
`allowed_include_dirs`. The semantics:

- **Empty list (`allowed_include_dirs = []` or unspecified-and-no-inherit)
  means "this rule does not participate in validation."** This is the
  idiom for allow-listing rules that match system headers or other
  externally-provided code where validation does not apply.
- **Quote and angle includes are both validated**; macro-form includes
  are skipped (action evaluation already errored on them).
- A failed validation produces a `validation_error` on the include and
  contributes to exit code 3.

## `inclean check` levels

`inclean check [-l|--level config|rules|full]` runs a read-only check
at one of three levels. Each level is a strict superset of the
previous.

| Level              | What it does                                                                                                                                                                                          | Failures contribute exit                                              |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `config`           | Just config-level structural checks (TOML syntax, `[project]` sigil rule, `extends` graph, name uniqueness, `@std.*` constants, action template syntax, layer-5 rejection). No source file is opened. | 1 (config error)                                                      |
| `rules`            | `config` + scans every source file. For each `#include`, computes the full set of matching rules and asserts the rule-tree invariants (`child ⊆ parent`, cross-chain disjoint). No action evaluation. | 3 (rule-tree conflict, layer-5 ambiguity)                             |
| `full` _(default)_ | `rules` + evaluates the matched rule's action on each include and validates the post-action text against `allowed_include_dirs`.                                                                      | 2 (`action.error`), 3 (eval failure, validation, conflict, ambiguity) |

`inclean diff` and `inclean apply` always run at `full` level.

## Exit codes

| Code | Meaning                                                                                                                                                                                                |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0    | Clean run.                                                                                                                                                                                             |
| 1    | Infrastructure/config error (parse failure, missing root, invalid regex).                                                                                                                              |
| 2    | At least one include matched an `action = { type = "error", … }` rule.                                                                                                                                 |
| 3    | At least one of: rule-tree conflict, layer-5 ambiguity, action evaluation failure (e.g. `auto` could not resolve under `allowed_include_dirs`), post-action `allowed_include_dirs` validation failure. |

`inclean apply` additionally refuses to write any file when conflicts
or ambiguities are present, and skips writing individual files whose
includes produced errors or validation failures.
