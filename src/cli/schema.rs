//! `inclean schema` — emit the JSON Schema for `inclean.toml`.
//!
//! Default: print to stdout.
//! `--output <PATH>`: write to a file (overwrites).
//! `--check`: regenerate and diff against `schemas/inclean.schema.json`,
//! exit 2 on drift. Used by CI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use schemars::generate::SchemaSettings;

use crate::config::schema::RawConfig;

const COMMITTED_SCHEMA_PATH: &str = "schemas/inclean.schema.json";

#[derive(clap::Args, Debug)]
pub struct SchemaArgs {
    /// Write the schema to this path instead of stdout.
    #[arg(short, long, conflicts_with = "check")]
    pub output: Option<PathBuf>,

    /// Regenerate the schema and diff against schemas/inclean.schema.json.
    /// Exit 2 if out of date. Intended for CI; runs from the repo root.
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: SchemaArgs) -> Result<u8> {
    let rendered = render()?;

    if args.check {
        return check_against_committed(&rendered);
    }

    match args.output {
        Some(path) => {
            std::fs::write(&path, &rendered)
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
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
        "https://raw.githubusercontent.com/inaku-Gyan/inclean/main/schemas/inclean.schema.json"
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

fn check_against_committed(rendered: &str) -> Result<u8> {
    let path = Path::new(COMMITTED_SCHEMA_PATH);
    let on_disk =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if on_disk == rendered {
        return Ok(0);
    }
    eprintln!(
        "error: {} is out of date.\n\
         The committed schema does not match the schema generated from the current source.\n\
         Regenerate it with:\n    cargo run -- schema --output {}\n",
        path.display(),
        path.display(),
    );
    Ok(2)
}
