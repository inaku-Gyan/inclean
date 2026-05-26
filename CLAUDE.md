# CLAUDE.md — Notes for Claude Code working in this repo

## What this project is

inclean is a Rust CLI that rewrites `#include` directives in C/C++ source
trees so the library can be consumed with a clean, minimal `-I` list. The
canonical use case: take an old library whose internal source uses
`#include "bar.h"` (resolving via `-Isrc/internal`) and rewrite every
such line so the consumer only needs `-Ipath/to/lib/include`.

For the module map and pipeline narrative, see
[docs/architecture.md](docs/architecture.md); for the full `inclean.toml`
schema (layers, actions, placeholders, `@std.*` constants), see
[docs/configuration.md](docs/configuration.md). This file keeps only
Claude-facing guidance.

## Design source of truth

The v0.3 refactor is captured in
[refactor.md](refactor.md). Read it before making non-trivial changes.
Key design choices that drive the current code shape:

- **Configuration**: TOML, single file. Exactly one `inclean.toml` per
  project; extra copies anywhere under the resolved project root are a
  hard error. The config **must** declare a `[project]` block with
  `version` and `min_inclean_version`. `[project].root` is relative to
  the config file's directory and resolves to the actual project root.
  All paths in rules are relative to the **resolved** project root.
- **Two-direction version compatibility check**: every load enforces
  `CLI_COMPAT_MIN <= cfg.version` AND `cfg.min_inclean_version <= CLI_CURRENT`.
  Either failure is a hard rejection — no migration shims.
  `CLI_COMPAT_MIN` is hand-maintained in [src/config/discover.rs](src/config/discover.rs).
- **Rule model**: flat list with single-level copy via `copied_from`.
  Rule `name` is globally unique. `copied_from` references must point at
  earlier-declared rules (forward-only). The copy is transitive — the
  child sees the parent's already-resolved value.
- **Asymmetric inheritance**: top-level fields the child omits inherit
  from the parent. Top-level object fields the child writes reset their
  inner fields to schema defaults — the child must use `${copied}` per
  inner field to pull the parent's value.
- **Four-layer matching** (each with a default):
  1. `file_paths` — globset patterns (full-string anchored, `literal_separator`)
  2. `file_suffixes` — literal extensions (skipped if `file_paths` was an exact path)
  3. `match_forms` — set of `"quote"` / `"angle"` / `"macro"`. Matching
     `"macro"` is allowed, but action evaluation against a macro
     `#include` always produces an error in v1.
  4. `include_match` — globset patterns over the include's stripped argument
- **`suppression_comments_regex`** marks off-limits regions per rule.
  The engine extracts each line's comment body (stripping `//` or same-line
  `/* */` delimiters and trimming) and applies the regex to *that*.
- **Six action variants**: `resolve` / `replace` / `keep` / `remove` /
  `comment_out` / `error`. Default action is `keep` with `output_form = Preserve`.
- **Conflict detection is by final-text**, not rule-tree invariants.
  When multiple rules match the same include, the action evaluator runs
  for each; if they all produce the same edit (or all `Keep`), there is
  no conflict. Any divergence is a `Conflict` and exits 3.
- **Trailing-comment processing**: `transform` runs on existing trailing
  comments (delimiter style + content regex); `append_if_absent` writes
  user-provided literal text when no trailing comment survives. Both
  only apply to `resolve` / `replace` / `keep` (the other actions
  rewrite the whole line and ignore trailing config).
- **`${copied}` placeholder** is substituted at copy-resolution time
  (not action time). Two contexts: whole-string `"${copied}"` in a
  scalar field, or array-element splat in a string list.
- **`@std.*` built-in constants** (e.g. `@std.cpp.extensions`,
  `@std.cpp17.system_headers_or`) spread in any string-list field via
  `@name` syntax, and substitute inline in regex / template strings.

## Module layout & pipeline data flow

See [docs/architecture.md](docs/architecture.md) for the per-module
responsibilities and the pipeline walkthrough inside `pipeline::run::run`.

## Dev workflow

```sh
cargo check     # fast type-check
cargo test      # unit + integration tests
cargo clippy    # lints
cargo fmt       # format
```

Golden tests under `tests/golden_tests/<case>/{input,expected}` are
strict tree-equality checks after `pipeline::run` + `pipeline::apply`.
Fixture tests (currently just `init_template`) drive the CLI binary.

## Releasing

See [CONTRIBUTING.md](CONTRIBUTING.md#releasing). In short: bump
`CHANGELOG.md` and `Cargo.toml` in one commit on `main`, then push a
`vX.Y.Z` SemVer tag — that triggers `.github/workflows/release.yml`,
which validates the tag, builds, and publishes to crates.io / PyPI /
GitHub Releases. The workflow's `check-tag` job will fail the release
if the tag doesn't equal `Cargo.toml`'s `version`.

## Conventions

- Use `anyhow::Result` for high-level error returns; `thiserror` for typed
  errors at module boundaries.
- Config-load errors should pinpoint the offending `inclean.toml` path
  and rule name.
- Keep `cli/*` files thin — they parse flags and call into `pipeline::run`.

## Pre-1.0 backward-compat policy

Before v1.0.0, **do not introduce any forward-compat or backward-compat
shim code.** `CLI_COMPAT_MIN` (in [src/config/discover.rs](src/config/discover.rs))
is one half of the version gate; the config's `min_inclean_version` is
the other. Bump `CLI_COMPAT_MIN` whenever the on-disk schema gets a
breaking change, and let `discover` hard-reject older configs.
Concretely, do not write:

- schema migration logic ("if version < X, transform like ..."),
- field-rename fallbacks or deprecated-alias support,
- per-version branching in parse/resolve code,
- "old format" probes or auto-upgrades on read,
- any other code whose only job is making old configs work.

The fix for a user with a stale config is to update their `inclean.toml`,
not to maintain compatibility code. Code clarity beats migration ergonomics
in pre-1.0. Revisit this rule when the project reaches v1.0.0.

## Things to avoid

- Don't reintroduce `extends` or rule-tree invariants. Conflicts are
  detected by comparing final-line text across all matched rules.
- Don't reintroduce `allowed_include_dirs`. Validation against allowed
  dirs is no longer part of v1.
- Don't reintroduce layer 5 (`match_resolved`) or
  `original_include_dirs`. The `resolve` action takes `include_directories`
  literal paths and does its own probe.
- Don't add file-moving, umbrella-header generation, or `extern "C"`
  wrapping. Out of scope for v1.
