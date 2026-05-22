# inclean

_English | [简体中文](README.zh-CN.md)_

A C/C++ `#include` path normalizer.

Many legacy C/C++ libraries `#include` headers by bare filename
(`#include "bar.h"`) even though the actual header lives several
directories deep (`src/internal/bar.h`). To consume such a library a
caller must add every internal directory to their `-I` list — which
pollutes their include namespace and breaks the library's
encapsulation.

`inclean` does a one-shot, source-level normalization. It scans every
source file in the library and rewrites each `#include` so it resolves
cleanly against a small, explicit set of allowed include directories.
After running `inclean`, consumers only `-I` the allowed directories.

---

## Install

`inclean` is published to **crates.io**, **PyPI**, and as prebuilt
binaries on **GitHub Releases**. Pick whichever ecosystem you already
have on your machine.

### Via PyPI (Python wheels — no Rust toolchain needed)

Three options:

```sh
uv tool install inclean      # isolated, fastest
pipx install inclean         # isolated
pip install inclean          # into the current environment
```

The wheels ship the native binary built by maturin; `inclean` lands on
your `PATH` the same way `ruff` or `uv` do. Requires Python ≥ 3.8.

### Via cargo

Two options:

```sh
cargo binstall inclean       # prebuilt binary from GitHub Releases (no compilation)
cargo install inclean        # build from crates.io
```

`cargo binstall` fetches inclean's binstall metadata from crates.io and downloads the
matching prebuilt archive from this repo's GitHub Releases (no
compilation).
`cargo binstall` is a third-party cargo subcommand — install it first
via [cargo-bins/cargo-binstall](https://github.com/cargo-bins/cargo-binstall)
if you don't already have it.

`cargo install` downloads the source from crates.io and compiles it locally.

### Prebuilt binaries (no package manager)

Download a tarball for your platform from the
[latest GitHub Release](https://github.com/inaku-Gyan/inclean/releases/latest)
and put `inclean` (or `inclean.exe`) on your `PATH`. Released targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

## Quick start

`inclean` is driven by an `inclean.toml` placed at the root of the
library you want to clean up. A typical workflow:

```sh
inclean init                # write a documented starter inclean.toml
$EDITOR inclean.toml        # tell it where your headers live
inclean check               # dry-run: report every proposed change
inclean diff                # see the rewrites as a unified diff
inclean apply               # write the rewrites in place
```

Every command except `explain` takes an optional `[DIR]` argument — the
directory containing the root `inclean.toml`. It defaults to `.`.

### Example

Take a "flat" library where headers live at
`include/mylib/internal/foo.h` but internal `#include`s use just the
basename. The fixture under
[tests/fixtures/flat-library/](tests/fixtures/flat-library/) ships
this config:

```toml
[project]
root = "."
version = "0.2.0"

[[rule]]
name = "base"
paths = ["src/**", "include/**"]
forms = ["quote"]
allowed_include_dirs = ["include"]
original_include_dirs = ["include/mylib/internal"]
```

A source line `#include "foo.h"` is rewritten to
`#include "mylib/internal/foo.h"` — so the consumer only needs
`-Iinclude`.

## Commands

| Command                                  | Purpose                                                                       |
| ---------------------------------------- | ----------------------------------------------------------------------------- |
| `inclean init [DIR]`                     | Generate a documented starter `inclean.toml`. Refuses to overwrite.           |
| `inclean check [DIR] [-l/--level LEVEL]` | Read-only check at one of three depths. Never writes.                         |
| `inclean diff [DIR]`                     | Print a unified diff of every proposed rewrite.                               |
| `inclean apply [DIR]`                    | Apply rewrites in place. Refuses if any rule-tree conflict is present.        |
| `inclean explain FILE [INCLUDE]`         | Trace, layer-by-layer, which rule matches a given `#include` — debugging aid. |
| `inclean schema [-o PATH] [--check]`     | Emit the JSON Schema for `inclean.toml` (stdout by default).                  |

`inclean check` runs at one of three levels (`-l config | rules |
full`, default `full`). Each level is a strict superset of the
previous; see [docs/configuration.md](docs/configuration.md#inclean-check-levels)
for the full breakdown.

## Editor support

`inclean.toml` ships with a JSON Schema for editor completion and
validation. Editors that understand the `#:schema` directive (VS Code
with [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml),
Helix, Zed) automatically pick it up:

```toml
#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v0.2.0/schemas/inclean.schema.json

[project]
root = "."
version = "0.2.0"
```

`inclean init` writes both the `#:schema` line (for the editor) and
the `[project].version` field (the CLI's own version gate), each
pinned to the CLI version that generated the file. To upgrade schema
validation, edit the `v0.2.0` segment in the URL to a newer release
tag; to always track the development schema, replace it with `main`
(not recommended for shared repos — new fields will appear in the
schema before your CLI knows about them).

**`#:schema` and `[project].version` are independent.** The
`#:schema` URL is purely for editor tooling; the CLI reads only
`[project].version` and compares it to its built-in
`MIN_SUPPORTED_INCLEAN_TOML_VERSION`. inclean is pre-1.0 and does
not ship migration shims for breaking schema changes — see
[CLAUDE.md](CLAUDE.md#pre-10-backward-compat-policy).

You can also dump a local copy:

```sh
inclean schema --output inclean.schema.json
```

## Documentation

- **[docs/configuration.md](docs/configuration.md)** — full
  `inclean.toml` schema: the five-layer matching model, inheritance,
  `@std.*` constants, actions, placeholders, exit codes.
- **[docs/architecture.md](docs/architecture.md)** — code-level
  architecture: module map, pipeline phases, key invariants.
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — toolchain, dev workflow,
  conventions, scope.
- **[CHANGELOG.md](CHANGELOG.md)** — release history.

## Status

`0.2.0` — current. Introduces JSON Schema generation, editor
`#:schema` support, and a required `[project].version` field that
the CLI uses as a hard version gate. inclean is pre-1.0 / beta and
does not provide migration shims between breaking schema changes;
see [CLAUDE.md](CLAUDE.md#pre-10-backward-compat-policy) for the
project policy.

## License

[BSD 3-Clause](LICENSE).
