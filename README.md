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

Pre-alpha. The crate skeleton compiles; functionality is being filled in
milestone by milestone. See `/home/inaku/.claude/plans/c-c-inclean-iterative-tome.md`
for the design plan and milestones.

## Usage (planned)

Every command except `explain` takes a `[DIR]` positional argument — the
directory that contains the project's root `inclean.toml`. Defaults to `.`.

```sh
inclean init    [DIR]          # generate a starter inclean.toml in DIR
inclean lint    [DIR]          # validate the configuration only
inclean check   [DIR]          # report rewrites + validation errors (CI friendly)
inclean diff    [DIR]          # show unified diff of would-be rewrites
inclean apply   [DIR]          # apply rewrites in place
inclean explain FILE [INCLUDE] # trace which rule matches an include
```

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
extensions = ["@std.all_extensions"]
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
