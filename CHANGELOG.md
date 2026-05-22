# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Breaking:** `[project].root` is now the actual project root path,
  interpreted relative to the directory containing `inclean.toml` and
  defaulting to `"."` if omitted. The resolved path must exist and be
  a directory. All paths in rules (`paths`, `allowed_include_dirs`,
  `original_include_dirs`, `match_resolved.under`) are now relative to
  the **resolved** project root, not to the config file's directory.
  This makes "config above project root" layouts (e.g. `inclean.toml`
  at a repo top with `root = "lib"`) a supported pattern.
- **Breaking:** sub-configs are no longer supported. Exactly one
  `inclean.toml` may exist anywhere under the project root; extras
  surface as a hard error with a message naming each offending file.
  Consolidate any existing per-directory configs into the root config.

## [0.1.1] — 2026-05-22

### Changed

- **Breaking:** `trailing_comment` schema redesigned to mirror include
  action handling. The four `policy` values (`prepend` / `append` /
  `replace` / `fill_if_absent`) and the `text` field are gone; the new
  shape is `{ match, to, form, spacing }` where `match` is a regex
  over the stripped existing comment body, `to` is the new comment
  body template (with new `${comment.N}` / `${comment.text}`
  placeholders), `form` switches between `"line"` / `"block"` /
  `"preserve"`, and `spacing` controls the gutter before the
  delimiter. `to = ""` removes the trailing comment entirely. See
  `docs/configuration.md` for the migration table and the idempotent
  prepend / append idioms in the Rust `regex` dialect.

## [0.1.0] — 2026-05-21

Initial release. Feature-complete for v1.

### Added

- Hierarchical `inclean.toml` configuration: one root config plus
  optional sub-directory configs.
- Pure rule tree with single inheritance via `extends`; globally unique
  rule names; AND-combined field merge at match time.
- Five-layer matching engine: `paths`, `extensions`, `forms`, `match`
  (layer-4 regex on stripped include content), and `match_resolved`
  (layer-5 constraint on the resolved physical file with ambiguity
  detection against `original_include_dirs`).
- `@std.*` built-in constants for C/C++ extensions and standard
  headers (C89–C23, C++98–C++23), with list-spread and `_or`
  regex-alternation forms.
- Four action types: `auto` (resolve + relativize against
  `allowed_include_dirs` or the source file's directory), `rewrite`,
  `keep`, `error`. `${...}` placeholder substitution including
  `${resolved.*}` for layer-5 matches.
- Rule-tree invariant enforcement at source-scan time:
  `child ⊆ parent` and cross-chain disjointness, reported per-include.
- Post-action `allowed_include_dirs` validation; empty list is the
  explicit opt-out for system-header rules.
- Five CLI subcommands: `init`, `check` (three-level via
  `-l/--level config|rules|full`), `diff`, `apply`, `explain`.
- Per-file processing parallelized with `rayon`; deterministic output
  via post-sort.
- Distinct exit codes: `0` clean, `2` user-defined `action.error`,
  `3` for any of rule-tree conflict, layer-5 ambiguity, action
  evaluation failure, or `allowed_include_dirs` validation failure.

[Unreleased]: https://github.com/inaku-Gyan/inclean/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/inaku-Gyan/inclean/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/inaku-Gyan/inclean/releases/tag/v0.1.0
