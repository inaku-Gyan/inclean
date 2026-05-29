//! `inclean config schema [-o|--output PATH] [--check]`
//!
//! - Without `--check`: write the JSON Schema to `-o` PATH or stdout.
//!   If `-o` points at an existing directory, the default filename
//!   `inclean.toml.schema.json` is used.
//! - With `--check`: `-o` is required and must point at an existing
//!   file (or a directory containing `inclean.toml.schema.json`); the
//!   command exits 0 when the file matches the current schema, non-zero
//!   with a printed diff when it drifts. The file is never modified.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use schemars::generate::SchemaSettings;

use crate::config::schema::RawConfig;

const DEFAULT_SCHEMA_FILENAME: &str = "inclean.toml.schema.json";

#[derive(clap::Args, Debug)]
pub struct SchemaArgs {
    /// Output PATH. Without `--check`: where to write the schema (file,
    /// or a directory in which to drop `inclean.toml.schema.json`). With
    /// `--check`: required; the file to compare against.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Compare the schema at `-o` against the current schema. Read-only;
    /// exits 0 on match, non-zero with a diff on mismatch.
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: SchemaArgs) -> Result<u8> {
    let rendered = render()?;

    if args.check {
        let path = args.output.as_deref().ok_or_else(|| {
            anyhow::anyhow!("--check requires -o/--output pointing at the schema file")
        })?;
        return check_against(path, &rendered);
    }

    match args.output {
        Some(path) => {
            let target = resolve_write_path(&path);
            if let Some(parent) = target.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&target, &rendered)
                .with_context(|| format!("writing {}", target.display()))?;
            eprintln!("wrote {}", target.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout()
                .write_all(rendered.as_bytes())
                .context("writing schema to stdout")?;
        }
    }
    Ok(0)
}

fn resolve_write_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join(DEFAULT_SCHEMA_FILENAME)
    } else {
        path.to_path_buf()
    }
}

fn render() -> Result<String> {
    let settings = SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let mut schema = generator.into_root_schema_for::<RawConfig>();

    let obj = schema
        .as_object_mut()
        .expect("root schema is a JSON object");
    obj.insert(
        "$schema".into(),
        "https://json-schema.org/draft/2020-12/schema".into(),
    );
    obj.insert(
        "$id".into(),
        "https://raw.githubusercontent.com/inaku-Gyan/inclean/main/schemas/inclean.toml.schema.json"
            .into(),
    );
    obj.insert("title".into(), "inclean.toml".into());
    obj.insert(
        "description".into(),
        "Configuration schema for inclean. \
         See https://github.com/inaku-Gyan/inclean/blob/main/docs/configuration.md"
            .into(),
    );

    let mut out = serde_json::to_string_pretty(&schema).context("serializing schema to JSON")?;
    out.push('\n');
    Ok(out)
}

fn check_against(path: &Path, rendered: &str) -> Result<u8> {
    let target = resolve_write_path(path);
    if !target.is_file() {
        anyhow::bail!(
            "{} does not exist (with --check it must be an existing file)",
            target.display()
        );
    }
    let on_disk = std::fs::read_to_string(&target)
        .with_context(|| format!("reading {}", target.display()))?;
    if on_disk == rendered {
        return Ok(0);
    }
    use similar::TextDiff;
    let diff = TextDiff::from_lines(&on_disk, rendered);
    eprintln!(
        "error: {} is out of date.\n\
         It does not match the schema generated from the current source.\n\
         Regenerate it with:\n    inclean config schema -o {}\n",
        target.display(),
        target.display(),
    );
    eprint!(
        "{}",
        diff.unified_diff()
            .context_radius(3)
            .header("on-disk", "rendered")
    );
    Ok(2)
}
