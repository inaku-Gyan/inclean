# CLAUDE.md — Notes for Claude Code working in this repo

## What this project is

inclean is a Rust CLI that rewrites `#include` directives in C/C++ source
trees so the library can be consumed with a clean, minimal `-I` list. The
canonical use case: take an old library whose internal source uses
`#include "bar.h"` (resolving via `-Isrc/internal`) and rewrite every
such line so the consumer only needs `-Ipath/to/lib/include`.

## Design source of truth

The full design lives in `/home/inaku/.claude/plans/c-c-inclean-iterative-tome.md`.
Read it before making non-trivial changes. Key design choices that drive
the code shape:

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

## Module layout (src/)

| Module | Responsibility |
|---|---|
| `cli/` | clap subcommands: `init`, `check`, `diff`, `apply`, `explain`. Every command except `explain` takes a positional `[DIR]` (default `.`) pointing at the directory that contains the root `inclean.toml`. `check` is three-level via `-l/--level config|rules|full` (default `full`). `diff` and `apply` always run full mode. |
| `config/schema.rs` | serde structs for TOML deserialization |
| `config/discover.rs` | walk the project tree, find all `inclean.toml`s |
| `config/inherit.rs` | resolve `extends`, merge fields, detect cycles |
| `config/constants.rs` | `@std.*` definitions and list-spread expansion |
| `lex/include_line.rs` | recognize `#include` directives, skip comments/strings/continuations |
| `rule/glob.rs` | layer 1 + layer 2 glob matching |
| `rule/engine.rs` | five-layer matching: `find_match` (first-match-wins, kept for tests) and `match_all` (returns `MatchAllOutcome { matched, ambiguities }`; used by mode `Rules` and above). Each `CandidateMatch` carries its captures and, when layer 5 ran, the project-root-relative resolved path. |
| `rule/tree.rs` | rule-tree invariants over the `extends` forest. `check_chain(matched, by_name)` either returns the deepest rule in `matched` (the leaf of a single ancestor chain) or a `ConflictKind` describing the violation |
| `rule/action.rs` | evaluate `auto` / `rewrite` / `keep` / `error` + `${...}` template |
| `index/header_index.rs` | basename / relpath → physical path index from `original_include_dirs` |
| `validate/allowed.rs` | post-rewrite resolvability check against matched rule's `allowed_include_dirs`. Quote and angle includes both validated (a rule's `forms` decides which forms it claims); macro skipped. Empty `allowed_include_dirs` = "this rule does not participate in validation" (the idiom for allow-listing e.g. system headers). |
| `pipeline/run.rs` | top-level orchestration |

## Pipeline data flow

`pipeline::run::run(project_root, mode: CheckMode) -> Summary` is the
single entry point with three modes:

* `CheckMode::Config` — load and validate configs (no source files
  opened). Returns an empty `Summary`.
* `CheckMode::Rules` — `Config` + scan source. For each `#include`,
  `engine::match_all` produces every candidate rule and `tree::check_chain`
  asserts the rule-tree invariants. Conflicts land in `Summary.conflicts`.
  No action evaluation, no `allowed_include_dirs` validation.
* `CheckMode::Full` — `Rules` + evaluate the **deepest** rule in the
  matched chain's action (NOT the first-match-by-declaration; the chain
  invariants make this well-defined and order-independent) + run
  `allowed_include_dirs` validation.

`IncludeOutcome` variants: `NoMatch`, `Matched` (Rules mode), `Keep`,
`Rewritten`, `Error`, `EvaluationFailure`, `Conflict`, `Layer5Ambiguous`.
`apply` refuses to write anything when `Summary.conflicts` is non-empty,
and skips any file with an `Error` / `EvaluationFailure` / `Conflict` /
`Layer5Ambiguous` outcome or a non-None `validation_error`.
`summary_exit_code` returns `0`, `2` (action.error), or `3` (any of:
EvaluationFailure, validation failure, rule-tree conflict, layer-5
ambiguity).

Per-file processing runs through `rayon::par_iter` after the (serial)
walker enumerates candidate paths. Each file's task is pure — it returns
its own `FileResult` and `Vec<Conflict>` — so no cross-thread sync is
needed. The final `Summary.files` and `Summary.conflicts` are sorted by
path so output is deterministic across runs.

## Dev workflow

```sh
cargo check     # fast type-check
cargo test      # unit + integration tests
cargo clippy    # lints
cargo fmt       # format

# Run perf benchmarks (11k-file synthetic project, release mode):
cargo test --release --test perf -- --ignored --nocapture
```

Integration fixtures live under `tests/fixtures/` (small fake libraries).
Add a new fixture for any non-trivial behavior change.

## Conventions

- Use `anyhow::Result` for high-level error returns; `thiserror` for typed
  errors at module boundaries.
- Rule-set / config errors should pinpoint the offending `inclean.toml`
  path and rule name in the message.
- Keep `cli/*` files thin — they parse flags and call into `pipeline::run`.
- The `auto` action requires the resolved file to live under one of the
  matched rule's `allowed_include_dirs`; failure is a hard error and aborts
  the file's apply.

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
