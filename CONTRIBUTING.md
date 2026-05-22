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
- `schemas/inclean.toml.schema.json` — generated JSON Schema for
  `inclean.toml`, derived from the structs in `src/config/schema.rs`.
  If you touch those structs (rename a field, add/remove a variant,
  change a serde attribute), regenerate the artifact in the same
  commit:
  ```sh
  cargo run -- schema --output schemas/inclean.toml.schema.json
  ```
  CI runs `cargo run -- schema --check schemas/inclean.toml.schema.json` and will fail the PR if the
  committed schema is stale.

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
- **Comments.** Default to none. Add one when the _why_ is non-
  obvious: a hidden constraint, a subtle invariant, a workaround. Do
  not narrate what code already says.
- **No `unsafe`.** There is none today; keep it that way.

## Releasing

Releases are tag-driven. Pushing a `vMAJOR.MINOR.PATCH` tag to GitHub
triggers `.github/workflows/release.yml`, which validates the tag,
runs the test suite, builds binaries for the five supported targets,
and publishes to crates.io, PyPI, and GitHub Releases in one shot.

Cut a release as follows:

0. **Sanity-check the schema artifact.** Normally enforced by CI on
   every PR, but verify locally before tagging:
   ```sh
   cargo run -- schema --check schemas/inclean.toml.schema.json
   ```
   Should exit 0.

   **If this release introduces a breaking change to `inclean.toml`**
   (renamed/removed field, changed semantics, etc.), also bump
   `MIN_SUPPORTED_INCLEAN_TOML_VERSION` in `src/config/discover.rs` to
   `X.Y.Z` and note "breaking: configs must now set `version >= X.Y.Z`"
   prominently in CHANGELOG. inclean is pre-1.0 and does not ship
   migration shims (see [CLAUDE.md](CLAUDE.md)).
1. **Update `CHANGELOG.md`.** Promote the `[Unreleased]` section to
   `[X.Y.Z] — YYYY-MM-DD` (today's date) and re-open an empty
   `[Unreleased]` above it. Update the reference links at the bottom.
2. **Bump `Cargo.toml`** — set `version = "X.Y.Z"`. The `check-tag`
   workflow job compares the tag against this field and refuses to
   publish if they disagree.
3. **Commit and merge to `main`.** Both edits land in the same commit
   (e.g. `chore: release v0.1.2`). The release workflow only fires on
   tags that point at a `main` commit.
4. **Tag and push.** From a clean `main`:
   ```sh
   git tag -a v0.1.2 -m "v0.1.2"
   git push origin v0.1.2
   ```
   The tag must be SemVer with a leading `v`. Pre-release suffixes
   (`v0.2.0-rc.1`) are accepted.

If the workflow fails at the `check-tag` step, delete the remote tag
(`git push origin :v0.1.2`) and the local one (`git tag -d v0.1.2`),
fix the mismatch, and re-tag — do **not** force-push over a published
tag.

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
