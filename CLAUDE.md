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
  (`allowed_include_dirs`, `original_include_dirs`, `validate_angle_patterns`)
  lives on rules.
- **Five-layer matching** (each layer has a default if unspecified):
  1. `paths` — gitignore-style file globs
  2. `extensions` — file extension filter (skipped if layer 1 is an exact path)
  3. `forms` — set of `"quote"` / `"angle"` / `"macro"`; `"macro"` always errors in v1
  4. `match` — regex on the stripped include content (no quotes/angles)
  5. `match_resolved` — v1 unsupported; error if configured
- **Inheritance semantics**: runtime AND-combination. The subset invariant
  ("child's match set ⊆ parent's") is enforced by evaluating the merged
  rule. Static lint catches obvious widening on layers 1/2/3; layer 4
  regex is not statically checked.
- **First-match-wins**, with closer (deeper) configs tried first.
- **Action default**: `{ type = "auto", relative_to = "allowed", form = "quote" }`.
- **`@std.*` built-in constants** (e.g. `@std.cpp.extensions`, `@std.cpp17.system_headers`)
  spread in any string-list field via `@name` syntax.

## Module layout (src/)

| Module | Responsibility |
|---|---|
| `cli/` | clap subcommands: `init`, `validate`, `check`, `diff`, `apply`, `explain`. Every command except `explain` takes a positional `[DIR]` (default `.`) pointing at the directory that contains the root `inclean.toml`. `validate` is the config-only verifier (parsing + structural invariants + extends + constants); `check` is the source-level dry-run. |
| `config/schema.rs` | serde structs for TOML deserialization |
| `config/discover.rs` | walk the project tree, find all `inclean.toml`s |
| `config/inherit.rs` | resolve `extends`, merge fields, detect cycles |
| `config/constants.rs` | `@std.*` definitions and list-spread expansion |
| `config/lint.rs` | static lint on layer 1/2/3 widening |
| `lex/include_line.rs` | recognize `#include` directives, skip comments/strings/continuations |
| `rule/glob.rs` | layer 1 + layer 2 glob matching |
| `rule/engine.rs` | five-layer matching loop + first-match-wins |
| `rule/action.rs` | evaluate `auto` / `rewrite` / `keep` / `error` + `${...}` template |
| `index/header_index.rs` | basename / relpath → physical path index from `original_include_dirs` |
| `validate/allowed.rs` | post-rewrite resolvability check against matched rule's `allowed_include_dirs`. Quote includes always; angle includes only when one of the rule's `validate_angle_patterns` regexes matches; macro includes skipped. Empty `allowed_include_dirs` = "this rule does not participate in validation". |
| `pipeline/run.rs` | top-level orchestration |

## Pipeline data flow

`pipeline::run::run(project_root, validate: bool) -> Summary` is the
single entry point. Per `IncludeResult` it carries the action `outcome`
(NoMatch / Keep / Rewritten / Error / EvaluationFailure) and an optional
`validation_error: Option<String>`. `apply` skips any file that has an
error / evaluation failure / validation_error so partial writes never
happen. `summary_exit_code` returns `0`, `2` (action.error), or `3`
(EvaluationFailure or validation failure).

## Dev workflow

```sh
cargo check     # fast type-check
cargo test      # unit + integration tests
cargo clippy    # lints
cargo fmt       # format
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
