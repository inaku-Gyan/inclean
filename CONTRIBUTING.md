# Contributing to inclean

## Checks before pushing

```sh
# format & lint
cargo fmt
cargo lint

# Run all tests:
cargo test

# debug or release build
cargo build
cargo build --release

# Build and install to $PATH:
cargo install --path .
```

## Config compatibility tests

Normal `cargo test` runs the current CLI against all golden and pipeline
fixtures. Historical config compatibility is checked separately by
`.github/scripts/check-config-compat.py`.

The compatibility runner installs the oldest CLI declared by
`MIN_COMPAT_CLI_VERSION` and reruns the `tests/golden_tests` and
`tests/pipeline_cases` fixtures whose own
`[project].min_inclean_version` allows that CLI. Golden cases are
validated by `inclean apply` plus tree comparison; pipeline cases check
the CLI exit code from `case.toml` and, when `apply = true`, compare the
resulting tree. Rust API-only `summary` / `unfixable` assertions remain
covered by the normal current-CLI test harness.

Run it locally with Python 3.11+:

```sh
python3 .github/scripts/check-config-compat.py

# Or use uv:
uv run -s --no-project .github/scripts/check-config-compat.py
```

Local runs use `tempdir/config-compat-test/...` and install the old CLI
there only. They prefer `cargo binstall` when available and fall back to
`cargo install --root ...`, so they do not write to `~/.cargo/bin`.
For manual debugging, set `INCLEAN_COMPAT_BIN=/path/to/inclean` to use a
specific binary. In CI, the job sets `INCLEAN_COMPAT_SOURCE=github-release`
and downloads the Linux x86_64 binary from GitHub Releases without a
fallback.

## Changing the config file policy

If you make changes to `inclean.toml` configuration policy
(adding/removing fields or modifying semantics),
update the compatible versions in `src/profile.rs`.

If the changes affect the JSON schema, regenerate the schema artifact:

```sh
cargo gen-schema
# or equivalently:
cargo run -- config schema -o schemas/inclean.toml.schema.json
```

## Releasing

Releases are tag-driven. Pushing a SemVer tag to GitHub
triggers `.github/workflows/release.yml`, which validates the tag,
runs the test suite, builds binaries for all supported targets,
and publishes to crates.io, PyPI, and GitHub Releases in one shot.

Cut a release as follows:

1. **Bump `Cargo.toml`** — set `version = "X.Y.Z"`.
   The version on PyPI is automatically synced from `Cargo.toml`, so no need to update `pyproject.toml`.
2. **Tag the Git commit** with the same SemVer as in `Cargo.toml` but with a leading `v`.
3. **Push the tag to GitHub** to trigger the release workflow automatically.

If the release workflow fails,
delete the remote and local Git tag,
fix the mismatch, and re-tag
— do NOT force-push over a published tag.
