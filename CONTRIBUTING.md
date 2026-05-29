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
