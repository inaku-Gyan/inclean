# inclean

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

From source (Rust 1.91+):

```sh
cargo install --path .
```

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

| Command | Purpose |
|---|---|
| `inclean init [DIR]` | Generate a documented starter `inclean.toml`. Refuses to overwrite. |
| `inclean check [DIR] [-l/--level LEVEL]` | Read-only check at one of three depths. Never writes. |
| `inclean diff [DIR]` | Print a unified diff of every proposed rewrite. |
| `inclean apply [DIR]` | Apply rewrites in place. Refuses if any rule-tree conflict is present. |
| `inclean explain FILE [INCLUDE]` | Trace, layer-by-layer, which rule matches a given `#include` — debugging aid. |

`inclean check` runs at one of three levels (`-l config | rules |
full`, default `full`). Each level is a strict superset of the
previous; see [docs/configuration.md](docs/configuration.md#inclean-check-levels)
for the full breakdown.

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

`0.1` — feature-complete for v1. See [CHANGELOG.md](CHANGELOG.md). The
synthetic 10k-file perf benchmark in [tests/perf.rs](tests/perf.rs)
runs `full` mode in well under 100 ms on a release build
(`cargo test --release --test perf -- --ignored --nocapture`).

## License

[BSD 3-Clause](LICENSE).
