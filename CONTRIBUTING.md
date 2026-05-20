# Contributing to inclean

Thanks for your interest in inclean. This is a small project — the
process is correspondingly light.

## Toolchain

- **Rust 1.91+** (2021 edition).
- **`rustfmt`** and **`clippy`** — install via
  `rustup component add rustfmt clippy` if your toolchain doesn't
  already have them.

No other system dependencies are required.

## Build and test

```sh
cargo build                # debug build
cargo check                # fast type-check
cargo test                 # unit + integration tests
cargo clippy --all-targets # lints
cargo fmt                  # format

# Run perf benchmarks (10k-file synthetic project, release mode):
cargo test --release --test perf -- --ignored --nocapture
```

All four of `cargo test`, `cargo clippy --all-targets`, and
`cargo fmt --check` should pass cleanly before a change is submitted.

## Repository layout

See [docs/architecture.md](docs/architecture.md) for the
module-by-module map. In short:

- `src/cli/*.rs` — clap subcommand handlers (thin).
- `src/pipeline/run.rs` — the orchestrator that every subcommand
  calls into.
- `src/config/*`, `src/lex/*`, `src/rule/*`, `src/index/*`,
  `src/validate/*` — the matching pipeline's building blocks.
- `tests/integration.rs` + `tests/fixtures/` — end-to-end tests.
- `tests/perf.rs` — `#[ignore]`-gated benchmarks.

## Adding a feature or fixing a bug

1. **Add (or extend) a fixture.** If the change alters observable
   behavior, add a tiny `inclean.toml` + source-file fixture under
   `tests/fixtures/<name>/`. Fixtures should be the smallest possible
   reproduction of the scenario you're testing.
2. **Add an integration test** in `tests/integration.rs` that drives
   the fixture and asserts on the outcome you care about (exit code,
   conflicts, rewritten text).
3. **Run `cargo test` + `cargo clippy --all-targets` + `cargo fmt`**
   clean.
4. **Open a PR** with a brief description of what changed and why.
   Link the fixture/test that demonstrates the new behavior.

## Code conventions

- **Errors.** Use `anyhow::Result` for high-level errors and
  `.with_context(…)` for I/O and parsing boundaries. `thiserror` is
  reserved for future typed errors at internal module boundaries.
- **Error messages.** When a config or rule-set error references a
  specific rule or `inclean.toml` file, the message must include the
  rule name and the source path so the user can locate the problem.
- **CLI is thin.** `src/cli/*.rs` files parse flags and call
  `pipeline::run`. Don't put pipeline logic in CLI handlers.
- **Comments.** Default to none. Add one when the *why* is non-
  obvious: a hidden constraint, a subtle invariant, a workaround. Do
  not narrate what code already says.
- **No `unsafe`.** There is none today; keep it that way.

## Things outside v1 scope

These have been considered and explicitly excluded. Please discuss
in an issue before submitting a PR for any of them:

- A `[defaults]` block or any project-level fallback for
  `allowed_include_dirs` / `original_include_dirs`. The deliberate
  design is "rule tree with explicit `base`".
- Widening the child-subset invariant. Child rules must never match
  more than their parent.
- Formally checking regex containment between layer-4 patterns.
  Runtime AND-combination is the enforcement.
- File-moving, umbrella-header generation, `extern "C"` wrapping,
  or any other source transformation beyond `#include` rewriting.

## Reporting bugs

Open an issue with the smallest possible reproduction:

- the offending `inclean.toml` (or its relevant rules),
- a one-or-two-file source snippet,
- what `inclean check` printed and what you expected.

If the bug is in `cargo test`, please also include the test name and
the output of `cargo test -- --nocapture <test_name>`.
