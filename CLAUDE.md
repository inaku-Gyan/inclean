# CLAUDE.md - repo notes for coding agents

## Project

`inclean` is a Rust CLI for normalizing C/C++ `#include` paths so consumers can
use a smaller, cleaner `-I` list. It reads one `inclean.toml`, scans source
files, matches include directives against rules, then reports, diffs, or applies
rewrites.

Crate version: `Cargo.toml`. Config/CLI compatibility constants:
`src/profile.rs`.

## Main Pipeline

The real entry point is `pipeline::run::run()` in `src/pipeline/run.rs`.
CLI modules should stay thin and call it.

Flow:

1. `discover::{find_root_config, load_root_config, resolve_project_root}` loads
   config and resolves `[project].root`.
2. `copy::resolve()` converts raw rules into declaration-ordered
   `ResolvedRule`s.
3. `compile_rules()` builds `CompiledRule`s.
4. `source_files()` walks the project root, applies CLI path filters, skips
   `inclean.toml`, prefilters with `any_rule_eligible()`, sorts, then uses rayon.
5. `process_file_outer()` reads bytes, preserves UTF-8 BOM state, and skips
   non-UTF-8 files.
6. `process_file()` scans `Include`s, computes suppression, runs
   `engine::match_all()`, evaluates every match with `action::evaluate()`, and
   calls `collapse_outcomes()`.
7. `run()` aggregates `Summary`, `FileResult`, `Conflict`, `UnfixableDetail`,
   skipped files, and warnings.

Key structs/enums:

- `schema`: `RawConfig`, `RawProject`, `RawRule`
- `copy`: `ResolvedRule`, `ResolvedAction`, `ResolvedSuppression`
- `engine`: `CompiledRule`, `CandidateMatch`, `MatchAllOutcome`
- `include_line`: `Include`, `ScanReport`
- `action`: `Outcome`
- `pipeline::run`: `Summary`, `FileResult`, `IncludeResult`, `IncludeOutcome`,
  `Conflict`, `UnfixableDetail`

Conflict handling is final-text based: every matched rule is evaluated. If all
rules produce the same final `(edit_range, new_text)` or the same kept text,
there is no conflict. Any divergence is `IncludeOutcome::Conflict`.

## Config Model

- One root `inclean.toml`; `[project].root` is config-relative and defaults to
  `"."`.
- Required version fields: `version`, `min_inclean_version`. Gate:
  `MIN_COMPAT_CFG_VERSION <= version` and `min_inclean_version <= CLI_VERSION`.
- Rule names are globally unique. `copied_from` references only earlier rules
  and is transitive.
- Omitted top-level child fields inherit. Written object fields reset inner
  fields unless `${copied}` is used.
- Match layers: `file_paths`, `file_suffixes`, `match_forms`, `include_match`.
  Macro includes may match, but action evaluation returns an error.
- Actions: `resolve`, `replace`, `keep`, `remove`, `comment_out`, `error`.
  The whole field can also be `action = "skip"` to opt out of action
  conflict detection. Default without `copied_from`: `skip`.
- `trailing_comment` only applies to `resolve`, `replace`, `keep`, and
  `action = "skip"`. The whole field can be `trailing_comment = "skip"`
  to opt out of trailing-comment conflict detection. Default without
  `copied_from`: `skip`.

## Commands

Useful checks:

```sh
cargo fmt --check
cargo clippy
cargo test
cargo build
```

After config-shape changes, regenerate the checked-in schema:

```sh
cargo run -- config schema -o schemas/inclean.toml.schema.json
```

Golden tests: `tests/golden_tests/<case>/{input,expected}`. Fixture tests:
`tests/fixture_tests`.

## Conventions

- Respect module boundaries: `cli` parses/prints, `pipeline` orchestrates,
  `rule` matches/evaluates, `config` parses/resolves, `lex` finds includes.
- Use `anyhow::Result` for high-level fallible APIs. Include config path and
  rule name in config errors where possible.
- Preserve BOM and line endings. Do not normalize source text while rewriting.
- Keep output deterministic; candidate files are sorted before parallel work.
- Config semantic changes need `src/profile.rs`, schema, tests, and README
  updates as needed.

## Pre-1.0 Compatibility Policy

Do not add migration shims before v1.0. Breaking schema changes should bump
`MIN_COMPAT_CFG_VERSION` / `MIN_COMPAT_CLI_VERSION` as appropriate and hard
reject stale configs. Avoid field aliases, old-format probes, auto-upgrades, or
per-version parsing branches.

## Avoid

- Reintroducing `extends`, `allowed_include_dirs`, `match_resolved`, or
  `original_include_dirs`.
- Adding file-moving, umbrella-header generation, or `extern "C"` wrapping.
- Turning rule matching back into "pick one best rule"; the current model is
  "evaluate all matches, then compare final text."
