//! Raw serde structures for `inclean.toml` (v0.3.0 schema).
//!
//! These types deserialize directly from TOML. Defaults, `@std.*` constant
//! expansion, `copied_from` resolution, and `${copied}` placeholder
//! substitution happen in later passes (`constants::expand`, `copy::resolve`).
//! Keep this module free of policy.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, de};

/// Top-level shape of a single `inclean.toml`.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub project: RawProject,

    #[schemars(length(min = 1))]
    #[serde(default, rename = "rule")]
    pub rules: Vec<RawRule>,
}

/// The `[project]` block.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawProject {
    /// Project root, relative to the config file's directory.
    /// Defaults to `"."` (the config's own directory).
    /// Must be a literal path, not a glob.
    #[serde(default = "default_project_root")]
    pub root: String,

    /// CLI version that wrote this config.
    pub version: String,

    /// Minimum CLI version that can parse this config.
    pub min_inclean_version: String,
}

fn default_project_root() -> String {
    ".".to_string()
}

/// Sentinel emitted when a user wrote a top-level object field as the
/// literal string `"${copied}"` (e.g. `action = "${copied}"`). Resolution
/// at [`copy::resolve`] time substitutes the parent's resolved object.
#[derive(Debug, Clone, Copy)]
pub enum MaybeCopiedObject<T> {
    Copied,
    Object(T),
}

impl<'de, T> Deserialize<'de> for MaybeCopiedObject<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use std::fmt;
        struct V<T>(PhantomData<T>);
        impl<'de, T> de::Visitor<'de> for V<T>
        where
            T: Deserialize<'de>,
        {
            type Value = MaybeCopiedObject<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("either the string \"${copied}\" or an object")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == "${copied}" {
                    Ok(MaybeCopiedObject::Copied)
                } else {
                    Err(E::custom(format!(
                        "expected \"${{copied}}\" or an object, got string {v:?}"
                    )))
                }
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let t = T::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(MaybeCopiedObject::Object(t))
            }
        }
        de.deserialize_any(V(PhantomData))
    }
}

// For schemars: emit T's own schema. The `"${copied}"` string sentinel is
// documented in docs/configuration.md and the template; we don't currently
// teach the JSON Schema about it (a future M-G pass can switch to a
// proper oneOf when/if editor tooling needs it).
impl<T: JsonSchema> JsonSchema for MaybeCopiedObject<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(g)
    }
}

/// A single `[[rule]]` entry, before defaulting / copy / constant expansion.
/// `Option<_>` distinguishes "user did not specify" from "user wrote empty".
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawRule {
    /// Globally unique across the config.
    pub name: String,

    /// Name of a previously declared rule whose resolved fields are copied
    /// (transitively) into this one. Top-level fields the child sets are
    /// kept; top-level fields the child omits inherit from the parent's
    /// resolved value. Inner fields of an object the child rewrites default
    /// to null/disabled — use `${copied}` to pull each inner field from the
    /// parent explicitly.
    pub copied_from: Option<String>,

    // ---- Layer 1: file paths (gitignore-style globs) ---------------------
    pub file_paths: Option<Vec<String>>,

    // ---- Layer 2: file suffixes (literal extensions like ".c") -----------
    pub file_suffixes: Option<Vec<String>>,

    // ---- Off-limits regions inside source files --------------------------
    /// The whole field can also be the string `"${copied}"` to reuse the
    /// parent rule's resolved value verbatim (object-context `${copied}`).
    pub suppression_comments_regex: Option<MaybeCopiedObject<RawSuppression>>,

    // ---- Layer 3: include forms ------------------------------------------
    pub match_forms: Option<Vec<IncludeForm>>,

    // ---- Layer 4: glob on the stripped include argument ------------------
    pub include_match: Option<Vec<String>>,

    // ---- Non-matching configuration --------------------------------------
    /// Literal directory paths under the project root (relative to it) that
    /// the `resolve` action probes to locate the include's actual header.
    /// NOT a glob; no implicit `/**` suffix; no `.gitignore` semantics.
    pub include_directories: Option<Vec<String>>,

    /// The whole field can also be the string `"${copied}"` to reuse the
    /// parent rule's resolved action verbatim (object-context `${copied}`).
    pub action: Option<MaybeCopiedObject<RawAction>>,

    /// The whole field can also be the string `"${copied}"` to reuse the
    /// parent rule's resolved value verbatim (object-context `${copied}`).
    pub trailing_comment: Option<MaybeCopiedObject<RawTrailingComment>>,
}

/// Suppression markers: regex patterns matched line-by-line.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSuppression {
    pub block_start: Option<String>,
    pub block_end: Option<String>,
    pub line: Option<String>,
}

/// Which "form" of `#include` a rule applies to.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IncludeForm {
    /// `#include "foo.h"`
    Quote,
    /// `#include <foo.h>`
    Angle,
    /// `#include MY_HEADER` (macro-defined). Matching this form is allowed
    /// in config; in v1 evaluation of an action against a macro #include
    /// always produces an error.
    Macro,
}

/// Output form for the include's delimiters after a rule fires.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputForm {
    /// `#include "..."`
    Quote,
    /// `#include <...>`
    Angle,
    /// Keep whatever form the original `#include` used.
    Preserve,
}

/// Trailing-comment delimiter style.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum CommentStyle {
    /// `// ...`
    #[serde(rename = "//")]
    Line,
    /// `/* ... */`
    #[serde(rename = "/**/")]
    Block,
}

/// Trailing-comment output style with an extra `preserve` variant for
/// `trailing_comment.transform.action.output_style`.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum OutputCommentStyle {
    /// `// ...`
    #[serde(rename = "//")]
    Line,
    /// `/* ... */`
    #[serde(rename = "/**/")]
    Block,
    /// Keep whatever delimiter the original trailing comment used.
    #[serde(rename = "preserve")]
    Preserve,
}

/// The action a rule executes on a matched `#include`. Tagged by `type`.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawAction {
    /// Resolve the include against the rule's `include_directories`, then
    /// rewrite the path to be relative to `relative_to`.
    Resolve {
        relative_to: String,
        #[serde(default)]
        output_form: Option<OutputForm>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Replace the include text with `with`, supporting `${...}` placeholders.
    Replace {
        with: String,
        #[serde(default)]
        output_form: Option<OutputForm>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Leave the include's argument alone (the form may still change via
    /// `output_form`).
    Keep {
        #[serde(default)]
        output_form: Option<OutputForm>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Delete the entire `#include` line.
    Remove {
        #[serde(default)]
        keep_blank_line: Option<bool>,
        #[serde(default)]
        keep_trailing_comment: Option<bool>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Wrap the include line in `//` (default) or `/* */` delimiters.
    CommentOut {
        #[serde(default)]
        style: Option<CommentStyle>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Report a user-facing error for the matched include. Exit code 2.
    Error {
        #[serde(default)]
        message: Option<String>,
    },
}

/// Trailing-comment configuration.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawTrailingComment {
    pub transform: Option<RawTrailingTransform>,
    /// Literal text to append to the include line when there is no
    /// trailing comment after action evaluation. The user writes the full
    /// comment text (including delimiters and leading whitespace).
    pub append_if_absent: Option<String>,
}

/// Trailing-comment transform: matches an existing comment, then runs an
/// action over it. `action` is required — per refactor.md §"Config File"
/// the schema lists it without a `?`, and silent defaulting to `Keep`
/// hides config bugs.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawTrailingTransform {
    pub match_styles: Option<Vec<CommentStyle>>,
    pub content_regex: Option<String>,
    pub action: RawTrailingAction,
}

/// The action a trailing-comment transform runs on its match.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawTrailingAction {
    /// Replace the comment body with `with`.
    Replace {
        with: String,
        #[serde(default)]
        output_style: Option<OutputCommentStyle>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Keep the comment body; only `output_style` may change.
    Keep {
        #[serde(default)]
        output_style: Option<OutputCommentStyle>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Remove the trailing comment entirely.
    Remove {
        #[serde(default)]
        message: Option<String>,
    },
    /// Report a user-facing error when the transform matches.
    Error {
        #[serde(default)]
        message: Option<String>,
    },
}

/// A file that has been loaded into memory, paired with its on-disk path.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub path: std::path::PathBuf,
    pub raw: RawConfig,
}

/// Parse the contents of a single `inclean.toml`. Path-aware errors
/// (line + column) bubble up via `anyhow`.
pub fn parse(text: &str, source_path: &std::path::Path) -> anyhow::Result<RawConfig> {
    toml::from_str::<RawConfig>(text).map_err(|err| {
        anyhow::anyhow!(
            "failed to parse inclean.toml at {}: {err}",
            source_path.display()
        )
    })
}

/// Index rules by name across `configs`. Returns an error on duplicate
/// names — rule names are globally unique.
pub fn index_rules_by_name<'a, I>(configs: I) -> anyhow::Result<BTreeMap<String, RuleLocator<'a>>>
where
    I: IntoIterator<Item = &'a LoadedConfig>,
{
    let mut by_name: BTreeMap<String, RuleLocator<'a>> = BTreeMap::new();
    for cfg in configs {
        for (idx, rule) in cfg.raw.rules.iter().enumerate() {
            let locator = RuleLocator {
                config_path: &cfg.path,
                index: idx,
                rule,
            };
            if let Some(prior) = by_name.insert(rule.name.clone(), locator) {
                anyhow::bail!(
                    "duplicate rule name `{}`: defined at both {} (rule #{}) and {} (rule #{})",
                    rule.name,
                    prior.config_path.display(),
                    prior.index,
                    cfg.path.display(),
                    idx,
                );
            }
        }
    }
    Ok(by_name)
}

/// Where a rule was defined; used for error messages and the by-name
/// lookup table.
#[derive(Debug, Clone, Copy)]
pub struct RuleLocator<'a> {
    pub config_path: &'a std::path::Path,
    pub index: usize,
    pub rule: &'a RawRule,
}

#[cfg(test)]
mod tests {
    use crate::utils::testing::config::load_rules;

    use super::*;
    use std::path::Path;

    #[test]
    fn base_rule_minimal() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            "#,
        )
        .raw;
        assert_eq!(cfg.rules.len(), 1);
        assert_eq!(cfg.rules[0].name, "base");
        assert!(cfg.rules[0].copied_from.is_none());
        assert!(cfg.rules[0].action.is_none());
    }

    /// Unwrap `MaybeCopiedObject::Object(...)`; panics on the sentinel form.
    fn raw_action_of(rule: &RawRule) -> &RawAction {
        match rule.action.as_ref().expect("action missing") {
            MaybeCopiedObject::Object(a) => a,
            MaybeCopiedObject::Copied => panic!("test expects an Object action, got Copied"),
        }
    }
    fn raw_suppression_of(rule: &RawRule) -> &RawSuppression {
        match rule
            .suppression_comments_regex
            .as_ref()
            .expect("suppression missing")
        {
            MaybeCopiedObject::Object(s) => s,
            MaybeCopiedObject::Copied => panic!("test expects an Object suppression, got Copied"),
        }
    }
    fn raw_trailing_of(rule: &RawRule) -> &RawTrailingComment {
        match rule
            .trailing_comment
            .as_ref()
            .expect("trailing_comment missing")
        {
            MaybeCopiedObject::Object(t) => t,
            MaybeCopiedObject::Copied => panic!("test expects an Object trailing, got Copied"),
        }
    }

    #[test]
    fn full_rule_with_resolve_action() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**", "include/**"]
            file_suffixes = ["@std.c.extensions"]
            match_forms = ["quote"]
            include_match = ["**/foo.h"]
            include_directories = ["src/internal"]
            action = { type = "resolve", relative_to = "include" }
            "#,
        )
        .raw;
        let r = &cfg.rules[0];
        assert_eq!(r.file_paths.as_ref().unwrap().len(), 2);
        assert_eq!(r.match_forms.as_ref().unwrap(), &vec![IncludeForm::Quote]);
        assert_eq!(
            r.include_match.as_ref().unwrap(),
            &vec!["**/foo.h".to_string()]
        );
        match raw_action_of(r) {
            RawAction::Resolve {
                relative_to,
                output_form,
                ..
            } => {
                assert_eq!(relative_to, "include");
                assert!(output_form.is_none());
            }
            _ => panic!("expected resolve action"),
        }
    }

    #[test]
    fn copied_from_field_parses() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"

            [[rule]]
            name = "child"
            copied_from = "base"
            "#,
        )
        .raw;
        assert_eq!(cfg.rules[1].copied_from.as_deref(), Some("base"));
    }

    #[test]
    fn all_six_action_variants_parse() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r1"
            action = { type = "resolve", relative_to = "${current_file}" }

            [[rule]]
            name = "r2"
            action = { type = "replace", with = "x" }

            [[rule]]
            name = "r3"
            action = { type = "keep" }

            [[rule]]
            name = "r4"
            action = { type = "remove" }

            [[rule]]
            name = "r5"
            action = { type = "comment_out" }

            [[rule]]
            name = "r6"
            action = { type = "error", message = "no" }
            "#,
        )
        .raw;
        assert!(matches!(
            raw_action_of(&cfg.rules[0]),
            RawAction::Resolve { .. }
        ));
        assert!(matches!(
            raw_action_of(&cfg.rules[1]),
            RawAction::Replace { .. }
        ));
        assert!(matches!(
            raw_action_of(&cfg.rules[2]),
            RawAction::Keep { .. }
        ));
        assert!(matches!(
            raw_action_of(&cfg.rules[3]),
            RawAction::Remove { .. }
        ));
        assert!(matches!(
            raw_action_of(&cfg.rules[4]),
            RawAction::CommentOut { .. }
        ));
        assert!(matches!(
            raw_action_of(&cfg.rules[5]),
            RawAction::Error { .. }
        ));
    }

    #[test]
    fn comment_out_style_renders_as_slashes() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r"
            action = { type = "comment_out", style = "/**/" }
            "#,
        )
        .raw;
        match raw_action_of(&cfg.rules[0]) {
            RawAction::CommentOut { style, .. } => {
                assert_eq!(*style, Some(CommentStyle::Block));
            }
            _ => panic!("expected comment_out"),
        }
    }

    #[test]
    fn suppression_block_round_trips() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r"
            suppression_comments_regex = {
                block_start = "^USER CODE BEGIN.*$",
                block_end = "^USER CODE END.*$",
                line = "^inclean: skip$",
            }
            "#,
        )
        .raw;
        let s = raw_suppression_of(&cfg.rules[0]);
        assert_eq!(s.block_start.as_deref(), Some("^USER CODE BEGIN.*$"));
        assert_eq!(s.block_end.as_deref(), Some("^USER CODE END.*$"));
        assert_eq!(s.line.as_deref(), Some("^inclean: skip$"));
    }

    #[test]
    fn trailing_comment_transform_parses() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r"
            trailing_comment = {
                transform = {
                    match_styles = ["//"],
                    content_regex = "^TODO.*$",
                    action = { type = "replace", with = "FIXED" },
                },
                append_if_absent = " // IWYU pragma: export",
            }
            "#,
        )
        .raw;
        let tc = raw_trailing_of(&cfg.rules[0]);
        let t = tc.transform.as_ref().unwrap();
        assert_eq!(t.match_styles.as_ref().unwrap(), &vec![CommentStyle::Line]);
        assert_eq!(t.content_regex.as_deref(), Some("^TODO.*$"));
        assert!(matches!(t.action, RawTrailingAction::Replace { .. }));
        assert_eq!(
            tc.append_if_absent.as_deref(),
            Some(" // IWYU pragma: export")
        );
    }

    #[test]
    fn unknown_action_field_is_rejected() {
        let err = parse(
            r#"
            [[rule]]
            name = "r"
            action = { type = "resolve", relative_to = ".", bogus = 1 }
            "#,
            Path::new("t.toml"),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("bogus"));
    }

    #[test]
    fn object_context_copied_sentinel_for_action() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "c"
            copied_from = "p"
            action = "${copied}"
            "#,
        )
        .raw;
        assert!(matches!(
            cfg.rules[0].action.as_ref().unwrap(),
            MaybeCopiedObject::Copied
        ));
    }

    #[test]
    fn object_context_copied_sentinel_for_suppression_and_trailing() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "c"
            copied_from = "p"
            suppression_comments_regex = "${copied}"
            trailing_comment = "${copied}"
            "#,
        )
        .raw;
        assert!(matches!(
            cfg.rules[0].suppression_comments_regex.as_ref().unwrap(),
            MaybeCopiedObject::Copied
        ));
        assert!(matches!(
            cfg.rules[0].trailing_comment.as_ref().unwrap(),
            MaybeCopiedObject::Copied
        ));
    }

    #[test]
    fn non_copied_string_for_object_field_is_rejected() {
        let err = parse(
            r#"
            [[rule]]
            name = "c"
            action = "bogus"
            "#,
            Path::new("t.toml"),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("${copied}"));
    }

    #[test]
    fn trailing_transform_missing_action_is_rejected() {
        let err = parse(
            r#"
            [[rule]]
            name = "r"
            trailing_comment = {
                transform = { content_regex = "^TODO.*$" },
            }
            "#,
            Path::new("t.toml"),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("action"), "got: {msg}");
    }
}
