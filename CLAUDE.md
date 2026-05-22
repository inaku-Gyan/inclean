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

The original design plan lives in
`/home/inaku/.claude/plans/c-c-inclean-iterative-tome.md` (local to the
maintainer's machine, not committed). Read it before making non-trivial
changes. Key design choices that drive the code shape:

- **Configuration**: TOML, hierarchical (`inclean.toml` at project root,
  optional `inclean.toml` in any sub-directory). The root config **must**
  declare `[project]` with `root` set; sub-configs **must not** declare
  `[project]` at all.
- **Rule model**: pure rule tree with single inheritance via `extends`. Rule
  `name` is globally unique across all configs in the project. There is
  **no `[defaults]` block** — users write a `base` rule and others extend it.
- **`[project]` block is minimal**: only `root`. Everything else
  (`allowed_include_dirs`, `original_include_dirs`) lives on rules.
- **Five-layer matching** (each layer has a default if unspecified):
  1. `paths` — gitignore-style file globs
  2. `extensions` — file extension filter (skipped if layer 1 is an exact path)
  3. `forms` — set of `"quote"` / `"angle"` / `"macro"`; `"macro"` always errors in v1
  4. `match` — regex on the stripped include content (no quotes/angles)
  5. `match_resolved` — only runs when the rule sets it. Resolves the
     include via `original_include_dirs` (must be unique — duplicate hits
     surface as `Layer5Ambiguous` per-include, exit 3); then enforces
     optional `under` (path-prefix) and `match` (path regex) constraints.
     When layer 5 runs, the action gets `${resolved.path}` /
     `${resolved.dir}` / `${resolved.basename}` placeholders.
- **Inheritance semantics**: runtime AND-combination merges fields; the
  rule-tree invariants ("child's match set ⊆ parent's" + "cross-chain
  disjoint") are enforced at source-scan time by
  `tree::check_chain(match_all(...))`. There is no static lint module —
  the source-level check supersedes it (also covers layer-4 regex).
- **Mode-dependent winner**: under `CheckMode::Full`, the action runs on
  the deepest rule in the matched chain (the leaf), not the first-by-
  declaration. This makes apply behavior independent of rule declaration
  order.
- **Action default**: `{ type = "auto", relative_to = "allowed", form = "quote" }`.
- **`@std.*` built-in constants** (e.g. `@std.cpp.extensions`, `@std.cpp17.system_headers`)
  spread in any string-list field via `@name` syntax.

## Module layout & pipeline data flow

Moved to [docs/architecture.md](docs/architecture.md). Read that doc
for the per-module responsibilities, the six-step pipeline walkthrough
inside `pipeline::run::run`, and the full list of `IncludeOutcome`
variants and exit-code semantics.

## Dev workflow

```sh
cargo check     # fast type-check
cargo test      # unit + integration tests
cargo clippy    # lints
cargo fmt       # format
```

Integration fixtures live under `tests/fixtures/` (small fake libraries).
Add a new fixture for any non-trivial behavior change.

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
- Rule-set / config errors should pinpoint the offending `inclean.toml`
  path and rule name in the message.
- Keep `cli/*` files thin — they parse flags and call into `pipeline::run`.
- The `auto` action requires the resolved file to live under one of the
  matched rule's `allowed_include_dirs`; failure is a hard error and aborts
  the file's apply.

## Pre-1.0 backward-compat policy

Before v1.0.0, **do not introduce any forward-compat or backward-compat
shim code.** `MIN_SUPPORTED_INCLEAN_TOML_VERSION` (in
`src/config/discover.rs`) is the single version gate: bump it whenever
the on-disk schema gets a breaking change, and let
`discover::validate_loaded` hard-reject older configs. Concretely, do
not write:

- schema migration logic ("if version < X, transform like ..."),
- field-rename fallbacks or deprecated-alias support,
- per-version branching in parse/resolve code,
- "old format" probes or auto-upgrades on read,
- any other code whose only job is making old configs work.

The fix for a user with a stale config is to update their `inclean.toml`,
not to maintain compatibility code. Code clarity beats migration ergonomics
in pre-1.0. Revisit this rule when the project reaches v1.0.0.

## Things to avoid

- Don't introduce a `[defaults]` block or any project-level fallback for
  `allowed_include_dirs` / `original_include_dirs`. The deliberate design
  is "rule tree with explicit `base`".
- Don't widen the rule subset invariant — child rules should never match
  more than the parent.
- Don't attempt to formally check regex containment for layer 4. Runtime
  AND-combination is the enforcement; static lint covers layers 1/2/3 only.
- Don't add file-moving, umbrella-header generation, or `extern "C"`
  wrapping. Out of scope for v1.
