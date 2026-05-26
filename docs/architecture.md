# Architecture

A high-level map of the `inclean` source. For the user-facing schema
and behavior, see [configuration.md](configuration.md).

## Shape

```
src/
├── cli/         # clap subcommands, exit-code routing
├── pipeline/    # top-level orchestrator (the only public entry point)
├── config/      # parse, copy resolution, @std.* expansion
├── lex/         # find #include directives in C/C++ source
└── rule/        # four-layer matching engine + action evaluator
```

The dependency direction is one-way: `cli → pipeline → (config, lex, rule)`.
Nothing in `config`/`lex`/`rule` depends on `pipeline` or `cli`.

## Module map

| File | Responsibility |
| --- | --- |
| [src/cli/mod.rs](../src/cli/mod.rs) | clap `Cli`/`Command` parser; routes to per-subcommand handlers; owns `CheckLevel` and the `config` subcommand group. |
| [src/cli/init.rs](../src/cli/init.rs) | `inclean init` / `inclean config new`. Resolves the PATH (existing dir vs nonexistent dir-like vs file-like) and writes the starter `inclean.toml`. |
| [src/cli/check.rs](../src/cli/check.rs) | Calls `pipeline::run`, prints per-outcome report (config-only or full). |
| [src/cli/diff.rs](../src/cli/diff.rs) | `inclean diff` — runs the pipeline and renders unified diff. |
| [src/cli/apply.rs](../src/cli/apply.rs) | `inclean apply` — runs the pipeline and writes rewritten files (refuses on conflicts; skips files with errors). |
| [src/cli/schema.rs](../src/cli/schema.rs) | `inclean schema` / `inclean config schema`. Emits the JSON schema from `schemars`; `--check PATH` diffs against an existing file. |
| [src/config/schema.rs](../src/config/schema.rs) | serde structs (`RawConfig`, `RawRule`, `RawAction`, `RawTrailingComment`, `CommentStyle`, `OutputForm`, `OutputCommentStyle`); pure deserialisation. |
| [src/config/discover.rs](../src/config/discover.rs) | Finds the single `inclean.toml` by walking upward; parses it; runs the two-direction version compatibility check (`CLI_COMPAT_MIN <= cfg.version` and `cfg.min_inclean_version <= CLI_CURRENT`); resolves `[project].root`; errors if any extra `inclean.toml` is present. |
| [src/config/copy.rs](../src/config/copy.rs) | Single-level `copied_from` resolution (transitive — the parent's already-resolved value is taken); `${copied}` placeholder substitution (scalar, splat); defaults; `@std.*` expansion. Produces `ResolvedRule`. |
| [src/config/constants.rs](../src/config/constants.rs) | `@std.*` definitions and `expand_list` / `substitute_in_string` expansion logic. Includes the `_or` suffix for regex alternation. |
| [src/lex/include_line.rs](../src/lex/include_line.rs) | Scans source for `#include` directives, skipping comments, string literals. Returns `Include { form, content, line, argument_range, trailing_range, trailing_comment_style }`. Also exposes `line_table(src)` for the engine's suppression scan. Cross-line `/* */` after the argument is **not** counted as a trailing comment. |
| [src/rule/glob.rs](../src/rule/glob.rs) | Layer 1 + 2 file matcher (`globset` with `literal_separator: true`). |
| [src/rule/engine.rs](../src/rule/engine.rs) | Four-layer matcher (`file_paths` glob, `file_suffixes` literal, `match_forms` set, `include_match` glob). Per-rule suppression-region scan over the file's line table. Returns every matched rule (no chain selection — conflicts are pipeline-side now). |
| [src/rule/action.rs](../src/rule/action.rs) | Evaluates the six action variants (`resolve`, `replace`, `keep`, `remove`, `comment_out`, `error`) and applies `trailing_comment.transform` / `append_if_absent`. Substitutes `${current_file}` and `${original}` (`${copied}` is handled at copy resolution time, not here). Macro-form includes always produce `Outcome::Error`. |
| [src/pipeline/run.rs](../src/pipeline/run.rs) | Top-level orchestrator. Owns `CheckMode`, `Summary`, `FileResult`, `IncludeOutcome`, `Conflict`, `SkippedFile`. Drives `discover → copy::resolve → walk → lex → match_all → action::evaluate → conflict-by-final-text`. Also exports `apply`, `render_diff`, `summary_exit_code`. |

## Pipeline phases

`pipeline::run::run(start_dir, mode: CheckMode) -> Result<Summary>` is
the single entry point used by every subcommand that actually processes
the project.

1. **Load config.** `discover::find_root_config` walks up from
   `start_dir`. `load_root_config` parses the file and runs the
   two-direction version check.
2. **Resolve project root.** `discover::resolve_project_root` joins the
   config's directory with `[project].root` and canonicalises.
   `assert_no_extra_configs` errors if any other `inclean.toml` is
   present under the resolved root.
3. **Resolve copies.** `copy::resolve` walks rules in declaration order,
   resolving each `copied_from` against already-resolved earlier rules.
   Applies defaults; expands `@std.*` constants in string lists and
   strings; substitutes `${copied}` per the asymmetric reset rule.
4. **Early exit for `Config` mode.** Returns an empty `Summary`.
5. **Compile rules.** Compile layer-1/2/4 globs and any
   `suppression_comments_regex` / `trailing_comment.content_regex`
   regexes into a `CompiledRule`.
6. **Walk source files.** `ignore::WalkBuilder` honors `.gitignore` and
   skips `.git/`, `target/`, `node_modules/`. The walker filters to
   files that some compiled rule's `file_paths` glob accepts.
7. **Sort candidates by lexicographic path** so `rayon::par_iter` (which
   preserves input order) yields a deterministic `Summary.files`.
8. **Per-file (parallel)**:
   - Read bytes; detect + strip UTF-8 BOM; decode UTF-8 (skip+warn on
     failure).
   - Lex includes + line table.
   - For each rule, compute its set of off-limits line numbers via
     `engine::compute_suppressed_lines`.
   - For each include: `engine::match_all` returns all rules whose four
     layers pass and whose suppression doesn't cover the include's
     line.
   - For each matched rule, `action::evaluate` produces an `Outcome`.
   - **Conflict resolution** — `IncludeOutcome` is built by:
     - Any `Error` wins → `IncludeOutcome::Error`.
     - Any `EvaluationFailure` wins next → `IncludeOutcome::EvaluationFailure`.
     - All `Keep` (or `Rewrite` resolved to no-op) → `IncludeOutcome::Keep`.
     - All `Rewrite` with identical `(edit_range, new_text)` →
       `IncludeOutcome::Rewritten`.
     - Otherwise → `IncludeOutcome::Conflict { rule_outputs }`.
9. **Apply edits.** Per file, edits are applied in reverse byte order
   so earlier ranges stay valid. Files that contain any
   `Error`/`EvaluationFailure`/`Conflict` outcome are skipped.

## Concurrency

The walker is single-threaded by `ignore::WalkBuilder`'s design.
Per-file work is pure CPU and runs on `rayon::par_iter` against the
pre-sorted candidate list. Output ordering is preserved by rayon's
`collect()` — no separate output thread is needed for in-process
consumers. The channel + heap-cache design described in
[refactor.md §"并行与输出保序"](../refactor.md) is reserved for a future
streaming-progress hook.

## Key data types

All exported from [src/pipeline/run.rs](../src/pipeline/run.rs):

- **`CheckMode { Config, Run }`** — Config skips source scan; Run goes
  full pipeline.
- **`Summary { mode, project_root, files, conflicts, skipped }`**.
- **`FileResult { relpath, original, rewritten, include_results, had_bom }`** —
  `rewritten` is `Some(_)` only when at least one edit applied.
  `had_bom` carries forward to the write-back so `apply` restores it.
- **`IncludeResult { include, outcome }`**.
- **`IncludeOutcome`** variants — see [Conflict resolution](#conflict-resolution-summary) below.
- **`Conflict { file_relpath, include_line, include_text, rule_outputs }`** —
  one per conflicting include, each entry of `rule_outputs` is
  `(rule_name, final_line_text)`.
- **`SkippedFile { relpath, reason }`** — files that couldn't be
  parsed; do not contribute to the exit code.

## Conflict resolution summary

| outcome | trigger | exit code contribution |
| --- | --- | --- |
| `NoMatch` | no rule matched | 0 |
| `Keep { rules }` | all matched rules produced no-op edit | 0 |
| `Rewritten { rules, edit_range, new_text }` | all matched rules produced same rewrite | 0 |
| `Error { rule, message }` | a rule's `action.type = "error"` matched (or trailing transform error) | 2 |
| `EvaluationFailure { rule, message }` | runtime action failure (e.g. `resolve` ambiguity) | 3 |
| `Conflict { rule_outputs }` | matched rules disagreed on final text | 3 |

`summary_exit_code` returns the maximum across all files + conflicts.

## Error model

- **`anyhow::Result`** at every high-level boundary (`pipeline::run::run`,
  every `cli/*::run`, every `config::*` entry point).
- **`thiserror`** is in `Cargo.toml` and reserved for future typed
  errors at internal module boundaries.
- **CLI exit codes** are computed from the final `Summary`, not by
  early-returning from subcommand handlers.

## Tests

- **`tests/run_golden_tests.rs`** — golden-test driver. Each
  `tests/golden_tests/<case>/{input,expected}` is a strict tree-equality
  check after running the library pipeline + apply.
- **`tests/run_fixture_tests.rs`** — fixture-based tests for the CLI
  binary (currently only `init_template`).
- Per-module unit tests live alongside the code (`mod tests { … }`
  inside each `.rs`).
