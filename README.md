# inclean

A C/C++ `#include` path normalizer.

Many legacy C/C++ libraries `#include` headers by bare filename (e.g.
`#include "bar.h"`) even though the actual header lives several directories
deep (e.g. `src/internal/bar.h`). To consume such a library, users must add
all the internal directories to their `-I` list, polluting their own
include namespace and breaking the library's encapsulation.

inclean performs a one-shot, source-level normalization: it scans every
source file in the library and rewrites each `#include` to a form that
resolves cleanly against an explicit, minimal set of allowed include
directories. After running inclean, consumers only need to `-I` the
allowed directories.

## Status

Alpha. M1 + M2 + M3 of the milestone plan are complete: configuration
loading, rule inheritance, the five-layer matching engine, `auto` /
`rewrite` / `keep` / `error` actions with `${...}` placeholder
substitution, post-action `allowed_include_dirs` validation, rule-tree
conflict enforcement (child ⊆ parent + cross-chain disjoint), and the
five CLI subcommands.

See `/home/inaku/.claude/plans/c-c-inclean-iterative-tome.md` for the
design plan and remaining work (M4: layer-5 resolved-file matching,
parallelism, perf).

## Usage

Every command except `explain` takes a `[DIR]` positional argument — the
directory that contains the project's root `inclean.toml`. Defaults to `.`.

```sh
inclean init  [DIR]               # generate a starter inclean.toml in DIR
inclean check [DIR] [MODE]        # three-mode read-only check (see below)
inclean diff  [DIR]                # show unified diff of would-be rewrites
inclean apply [DIR]                # apply rewrites in place
inclean explain FILE [INCLUDE]     # trace which rule matches an include
```

`inclean check` has three modes (mutually-exclusive flags):

| Mode | Flag | What it does |
|---|---|---|
| Syntax | `--syntax-only` | Just config-level structural checks (TOML syntax, `[project]` sigil, `extends` graph, rule-name uniqueness, `@std.*` constants, template syntax). No source file is opened. |
| Rules | `--no-rewrites` | Above + scan source, enforce **rule-tree invariants**: every child rule's match set must be a subset of its ancestors', and rules on different chains must not overlap on any single `#include`. No action evaluation. |
| Full | _(default)_ | Above + evaluate actions and validate post-action includes against `allowed_include_dirs`. |

## Configuration

inclean is configured by `inclean.toml` placed at the project root. The root
config **must** declare a `[project]` block whose `root` field is set
explicitly — this distinguishes the root config from any sub-configs.
Sub-directory `inclean.toml` files may not declare a `[project]` block; they
only contribute additional `[[rule]]` entries.

Rules form a single-inheritance tree via the `extends` field; rule names are
globally unique across the project.

A minimal config:

```toml
[project]
root = "."

[[rule]]
name = "base"
paths = ["src/**", "include/**"]
# extensions defaults to ["@std.c_extensions", "@std.cpp_extensions"]
forms = ["quote"]
allowed_include_dirs = ["include"]
original_include_dirs = ["src", "src/internal"]
# action defaults to { type = "auto", relative_to = "allowed", form = "quote" }
```

See the design plan for the full rule schema, the five-layer matching model,
inheritance semantics, `@std.*` built-in constants, and validation rules.

## Building

```sh
cargo build --release
```

Requires Rust 1.91+ (2021 edition).

## License

See `LICENSE`.
