# Architecture

A high-level map of the `inclean` source. For the user-facing schema
and behavior, see [configuration.md](configuration.md).

## Shape

```
src/
├── cli/         # clap subcommands, exit-code routing
├── pipeline/    # top-level orchestrator (the only public entry point)
├── config/      # parse, validate, inherit, expand @std.*
├── lex/         # find #include directives in C/C++ source
├── rule/        # the five-layer matching engine + rule-tree invariants
├── index/       # resolve include text against original_include_dirs
└── validate/    # post-action allowed_include_dirs check
```

The dependency direction is one-way: `cli → pipeline → (config, lex,
rule, index, validate)`. Nothing in `config`/`lex`/`rule`/`index`/
`validate` depends on `pipeline` or `cli`.

## Module map

| File                                                      | Responsibility                                                                                                                                                                                                                                                      |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [src/cli/mod.rs](../src/cli/mod.rs)                       | clap `Cli`/`Command` parser; routes to per-subcommand handlers; owns the `CheckLevel` value-enum surfaced as `-l/--level`.                                                                                                                                          |
| [src/cli/init.rs](../src/cli/init.rs)                     | Generates a documented starter `inclean.toml`; refuses to overwrite.                                                                                                                                                                                                |
| [src/cli/check.rs](../src/cli/check.rs)                   | Calls `pipeline::run` in the requested mode, prints per-level reports (config/rules/full).                                                                                                                                                                          |
| [src/cli/diff.rs](../src/cli/diff.rs)                     | Calls `pipeline::run` in Full mode and renders a unified diff via `pipeline::render_diff`.                                                                                                                                                                          |
| [src/cli/apply.rs](../src/cli/apply.rs)                   | Calls `pipeline::run` in Full mode, then writes rewritten files via `pipeline::apply` (refuses on conflicts; skips files with errors).                                                                                                                              |
| [src/cli/explain.rs](../src/cli/explain.rs)               | For a single source file (and optional include), traces every rule's five-layer trial outcome — debugging aid.                                                                                                                                                      |
| [src/config/schema.rs](../src/config/schema.rs)           | serde structs (`RawConfig`, `RawRule`, `RawAction`, `IncludeForm`, `OutputForm`); pure deserialization, no policy.                                                                                                                                                  |
| [src/config/discover.rs](../src/config/discover.rs)       | Walks the project tree from the root, loads every `inclean.toml`, enforces "root config declares `[project].root`; sub-configs must not".                                                                                                                           |
| [src/config/inherit.rs](../src/config/inherit.rs)         | Resolves `extends` chains, merges inherited fields, applies defaults, expands `@std.*` constants, detects cycles. Produces `ResolvedRule`.                                                                                                                          |
| [src/config/constants.rs](../src/config/constants.rs)     | `@std.*` definitions and the `expand_list` / `substitute_in_string` expansion logic, including the `_or` regex-alternation suffix.                                                                                                                                  |
| [src/lex/include_line.rs](../src/lex/include_line.rs)     | Scans source for `#include` directives, skipping comments, string literals, and line continuations. Returns `Include { form, content, line, argument_range, trailing_range }` where `trailing_range` covers everything between the argument and the line-terminating `\n`. Does not evaluate preprocessor conditionals.                                                         |
| [src/rule/glob.rs](../src/rule/glob.rs)                   | Layer 1 (`paths`) + layer 2 (`extensions`) compiled matcher. Uses `globset` with literal `/` (so `*` does not cross separators).                                                                                                                                    |
| [src/rule/engine.rs](../src/rule/engine.rs)               | The five-layer matcher. `find_match` (first-match-wins, kept for tests) and `match_all` (returns `MatchAllOutcome { matched, ambiguities }`, used by `rules` level and above). Each `CandidateMatch` carries its captures and, when layer 5 ran, the resolved path. |
| [src/rule/tree.rs](../src/rule/tree.rs)                   | Rule-tree invariants over the `extends` forest. `check_chain(matched, by_name)` returns either the deepest rule in the matched set (the leaf of a single ancestor chain) or a `ConflictKind` describing a `ChildWiderThanParent` or `CrossChain` violation.         |
| [src/rule/action.rs](../src/rule/action.rs)               | Evaluates `auto` / `rewrite` / `keep` / `error` and substitutes `${...}` placeholders (captures, file paths, resolved-file paths). `auto` requires the resolved file to live under the matched rule's `allowed_include_dirs`. Also applies the rule's `trailing_comment` (regex `match` over the stripped existing body, template `to` for the new body, `form`, `spacing`); the compiled comment regex lives on `CompiledRule.trailing_comment_regex`. When the comment block runs, the engine widens `Outcome::Rewrite.edit_range` past the argument. Idempotency falls out of byte-equality at the end of `finalize_outcome` — when the new bytes equal what's already on disk, the outcome collapses to `Outcome::Keep`.                                       |
| [src/index/header_index.rs](../src/index/header_index.rs) | Resolves `#include` text against a list of search directories (preprocessor-style: `<dir>/<text>`). `resolve_in_dirs_unique` surfaces ambiguity when multiple dirs contain the same file.                                                                           |
| [src/validate/allowed.rs](../src/validate/allowed.rs)     | Post-action check that the (possibly rewritten) include resolves under the matched rule's `allowed_include_dirs`. Empty list = skip (the system-header idiom).                                                                                                      |
| [src/pipeline/run.rs](../src/pipeline/run.rs)             | Top-level orchestrator. Owns `CheckMode`, `Summary`, `FileResult`, `IncludeOutcome`, `Conflict`. Also exports `apply`, `render_diff`, `summary_exit_code`.                                                                                                          |

## Pipeline phases

`pipeline::run::run(project_root, mode: CheckMode) -> Result<Summary>`
is the single entry point used by every subcommand that actually
processes the project.

1. **Load configs.** `config::discover::load_all_configs` walks the
   project tree, parses every `inclean.toml`. `validate_loaded`
   enforces the `[project]` sigil rule.
2. **Resolve inheritance.** `config::inherit::resolve` walks the
   `extends` graph, merges fields, applies defaults, expands
   `@std.*`. Returns `ResolvedRule`s keyed by name.
3. **Early exit for `Config` mode.** Returns an empty `Summary`; no
   source files are opened.
4. **Compile rules.** Compile layer-1/2 globs and layer-4/5 regexes
   into `CompiledRule`s. Build the by-name index for tree lookups.
5. **Walk source files.** `ignore::WalkBuilder` honors `.gitignore`
   and skips `.git/`, `target/`, `node_modules/`. The walker filters
   to files that satisfy _some_ rule's path/extension predicate so
   we don't read files no rule could ever match.
6. **Process per file, in parallel.** Candidate paths are handed to
   `rayon::par_iter`. Each file's task is pure:
   - Lex the file for `#include` directives.
   - For each include, `engine::match_all` produces all candidate
     rules; `tree::check_chain` either picks the leaf or records a
     `Conflict`.
   - In `Full` mode, evaluate the leaf rule's action and run the
     `validate::allowed` check on the post-action text. In `Rules`
     mode, stop after `check_chain`.
   - Returns its own `FileResult` and `Vec<Conflict>`. No shared
     state.
7. **Sort & return.** `Summary.files` and `Summary.conflicts` are
   sorted by path so output is deterministic across runs.

## Concurrency

The walker is single-threaded; everything inside the per-file task
runs on `rayon::par_iter`. The split is intentional: walking is I/O-
bound and serial by `ignore::WalkBuilder`'s design, while per-file
work (lex + match + action eval + validate) is pure CPU on already-
collected paths. Because each task owns its result and returns it,
there is no cross-thread synchronization. Determinism comes from the
final sort, not from execution order.

## Key data types

All exported from [src/pipeline/run.rs](../src/pipeline/run.rs).

- **`CheckMode { Config, Rules, Full }`** — controls how deep the
  pipeline runs.
- **`Summary`** — `{ mode, project_root, files, conflicts }`.
- **`FileResult`** — `{ relpath, original, rewritten, include_results }`.
  `rewritten` is `Some(_)` only when `Full` mode produced edits.
- **`IncludeResult`** — `{ include, outcome, validation_error }`.
- **`IncludeOutcome`** — one variant per per-include result:
  | Variant | When |
  |---|---|
  | `NoMatch` | No rule matched. |
  | `Matched { rule }` | Matched in `Rules` mode (no action evaluation). |
  | `Keep { rule }` | `Full` mode, action was `keep`. |
  | `Rewritten { rule, new_text, edit_range }` | `Full` mode, action rewrote. |
  | `Error { rule, message }` | `Full` mode, action was `error`. Exit 2. |
  | `EvaluationFailure { rule, message }` | Action evaluation failed (e.g. `auto` couldn't resolve under allowed). Exit 3. |
  | `Conflict` | `tree::check_chain` reported a rule-tree violation. Exit 3. |
  | `Layer5Ambiguous` | Layer-5 resolution hit multiple `original_include_dirs`. Exit 3. |
- **`Conflict`** — per-include record of a rule-tree violation:
  `{ file_relpath, include_line, include_text, kind }` where `kind`
  is `ChildWiderThanParent { child, missing_ancestor }` or
  `CrossChain { a, b }`.
- **`summary_exit_code(&summary) -> u8`** — returns `0` (clean), `2`
  (any `action.error`), or `3` (anything from `EvaluationFailure`,
  `Conflict`, `Layer5Ambiguous`, or a validation error).

## Invariants worth knowing

- **Deepest rule wins in `Full` mode.** When multiple rules match an
  include, the action is evaluated on the **leaf** of the matched
  chain — not the first rule by declaration order. The rule-tree
  invariants guarantee the matched set is a single chain, so the leaf
  is well-defined. Apply behavior is therefore independent of
  declaration order.
- **Empty `allowed_include_dirs` skips validation** for that rule.
  This is how rules covering system headers (`forms = ["angle"]`)
  opt out cleanly.
- **Layer 5 is opt-in.** Rules without `match_resolved` skip the
  resolved-file stage entirely. When layer 5 _does_ run and the
  include resolves under more than one `original_include_dirs`,
  the outcome is `Layer5Ambiguous` (exit 3) — the user is expected to
  narrow their `-I` list.
- **Trial order is deepest-config-first, then declaration order.**
  `find_match` (first-match-wins, used in tests) and `match_all` walk
  rules in this order; the rule-tree invariants make this order
  semantically irrelevant in `Full` mode (the leaf is what matters).
- **Macro-form includes are matched but never evaluated.** A rule
  with `forms = ["macro"]` matches macro `#include`s, but action
  evaluation always errors on them in v1. This lets a rule explicitly
  catch and report them.
- **Conflict detection is per-include, not per-file.** A single file
  may have a mix of conflict and rewritten includes.
- **`apply` refuses to write any file when conflicts are present.**
  Per-file errors (`Error`, `EvaluationFailure`, validation failures)
  only skip that one file; cross-cutting conflicts abort all writes.

## Error model

- **`anyhow::Result`** at every high-level boundary
  (`pipeline::run::run`, every `cli/*::run`, every `config::*` entry
  point). Errors are propagated with `.with_context(…)` so the user
  sees a stack: "reading /path/to/file: permission denied".
- **`thiserror`** is in `Cargo.toml` and reserved for future typed
  errors at internal module boundaries; current code uses `anyhow`
  end-to-end.
- **CLI exit codes** are computed from the final `Summary`, not by
  early-returning out of subcommand handlers. See `summary_exit_code`.

## Tests

- **`tests/integration.rs`** — end-to-end tests against fixtures
  under `tests/fixtures/` (currently just `flat-library/`). Each test
  copies the fixture to a tempdir (non-destructive), drives the CLI
  binary (or the library API), and asserts on `Summary` shape or
  diffed output.
- Per-module unit tests live alongside the code (`mod tests { … }`
  inside each `.rs`). Notably, `pipeline::run::tests` contains the
  highest-level integration coverage that doesn't go through the CLI
  binary.
