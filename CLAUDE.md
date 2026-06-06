# CLAUDE.md - agent notes

## Project Shape

`inclean` is a Rust CLI that normalizes C/C++ `#include` paths from one
`inclean.toml`.

Keep module boundaries clear:

- `cli`: clap args and human-readable output only.
- `config`: parse raw TOML, compatibility checks, `copied_from` resolution.
- `lex`: find includes and macro definitions.
- `rule`: match and evaluate one rule.
- `pipeline`: orchestrate scans, collapse all matching rules, aggregate reports,
  apply/diff edits.

The main runtime entry point is `pipeline::run::run()` in
`src/pipeline/run.rs`.

## Config And Compatibility

- Config version fields are required: `[project].version` and
  `[project].min_inclean_version`.
- Current CLI/config version comes from `Cargo.toml`.
- Compatibility constants live in `src/profile.rs`:
  `MIN_COMPAT_CFG_VERSION` and `MIN_COMPAT_CLI_VERSION`.
- Before v1.0, do not add migration shims or old-format aliases. Breaking config
  changes should bump the relevant compatibility constant and reject stale
  configs.
- If config shape or schema descriptions change, update
  `schemas/inclean.toml.schema.json` with:

```sh
cargo run --locked -- config schema -o schemas/inclean.toml.schema.json
```

## Testing

Use focused checks while editing, then run the full suite before committing
release-facing changes:

```sh
cargo fmt
cargo run --locked -- config schema --check -o schemas/inclean.toml.schema.json
cargo test --all-features --locked
```

Normal tests cover the current CLI. Historical config compatibility is checked
by `.github/scripts/check-config-compat.py` and reruns compatible golden and
pipeline fixtures using `MIN_COMPAT_CLI_VERSION`.

## Release Checklist

Release tags drive `.github/workflows/release.yml`, which builds binaries and
publishes to GitHub Releases, PyPI, and crates.io.

For a release such as `v0.4.0`:

1. Set `Cargo.toml` package `version = "0.4.0"`.
2. Let `Cargo.lock` update the `inclean` package version.
3. Update docs/examples that hard-code the release tag or generated config
   version, especially:
   - `README.md`
   - `docs/README.zh-CN.md`
   - `docs/configuration.md`
   - `docs/configuration.zh-CN.md`
4. Keep `MIN_COMPAT_CLI_VERSION` unchanged unless generated configs now need a
   newer CLI to parse or execute them.
5. Run:

```sh
rg 'old-version-or-prerelease-string'
cargo run --locked -- config schema --check -o schemas/inclean.toml.schema.json
cargo test --all-features --locked
```

6. Commit the release prep.
7. Tag the commit with `vX.Y.Z`; the tag version must match `Cargo.toml`.
8. Push the tag. If the workflow fails before publication, delete the local and
   remote tag, fix, and tag again. Do not force-push over a published tag.

Do not run `cargo package` just to check networking; CI handles publish-time
packaging.

## Avoid

- Reintroducing deprecated config fields such as `extends`,
  `allowed_include_dirs`, `match_resolved`, or `original_include_dirs`.
- Turning matching back into “pick one best rule”; current behavior evaluates
  all matches and compares final text.
- Normalizing source encodings or line endings while rewriting; preserve BOM and
  existing line endings.
