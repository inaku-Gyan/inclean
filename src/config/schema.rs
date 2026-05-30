//! Raw serde structures for `inclean.toml`.
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
/// at [`crate::config::copy::resolve`] time substitutes the parent's resolved object.
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

// For schemars: object-valued fields also accept the whole-field
// `"${copied}"` sentinel when `copied_from` is set. Keep this in the JSON
// Schema so editors do not reject valid inheriting configs.
impl<T: JsonSchema> JsonSchema for MaybeCopiedObject<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let object_schema: serde_json::Value = T::json_schema(g).into();
        let copied_schema = serde_json::json!({
            "type": "string",
            "const": "${copied}",
            "description": "Copy this whole object from the resolved parent rule named by copied_from."
        });
        let mut schema = serde_json::Map::new();
        schema.insert(
            "anyOf".into(),
            serde_json::json!([object_schema, copied_schema]),
        );
        schema.into()
    }
}

/// A single `[[rule]]` entry. Raw rules deserialize exactly as written;
/// inheritance from `copied_from`, `@std.*` constants, `${copied}`, and
/// effective defaults are resolved later.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawRule {
    /// Globally unique rule name. Rules are evaluated in declaration order,
    /// and diagnostics refer to this name.
    pub name: String,

    /// Name of a previously declared rule to copy from. The child starts from
    /// the parent's already resolved value, so copy chains are transitive, but
    /// references are forward-only. Omitted top-level fields inherit from the
    /// parent; written top-level fields replace the parent's field. Inside a
    /// written object, omitted inner fields reset to their defaults unless the
    /// child writes `${copied}` for that inner field.
    pub copied_from: Option<String>,

    // ---- Layer 1: file paths (globset globs) -----------------------------
    /// Globset patterns matched against paths relative to `[project].root`.
    /// `*` does not cross `/`; `**` does. Effective default for a rule with no
    /// parent is `["**/*"]`. If a matching pattern contains wildcard
    /// characters, `file_suffixes` must also match; exact literal paths skip
    /// the suffix check.
    pub file_paths: Option<Vec<String>>,

    // ---- Layer 2: file suffixes (literal extensions like ".c") -----------
    /// Literal extensions, including the leading dot, used after a wildcard
    /// `file_paths` match. Effective default for a rule with no parent is
    /// `["@std.c.extensions", "@std.cpp.extensions"]`, expanded to the built-in
    /// C and C++ extension lists.
    pub file_suffixes: Option<Vec<String>>,

    // ---- Off-limits regions inside source files --------------------------
    /// Optional per-rule suppression markers. Regexes are matched against each
    /// line after stripping `//` or same-line `/* */` delimiters when present
    /// and trimming whitespace. The whole field can also be the string
    /// `"${copied}"` to reuse the parent's resolved value verbatim.
    pub suppression_comments_regex: Option<MaybeCopiedObject<RawSuppression>>,

    // ---- Layer 3: include forms ------------------------------------------
    /// Include delimiter forms this rule matches. Effective default for a rule
    /// with no parent is `["quote"]`. `macro` can match `#include FOO`, but
    /// evaluating any action against a macro include is currently an error.
    pub include_forms: Option<Vec<IncludeForm>>,

    // ---- Layer 4: glob on the stripped include argument ------------------
    /// Globset patterns matched against the include argument with quotes or
    /// angle brackets stripped, for example `mylib/foo.h`. `*` does not cross
    /// `/`; `**` does. Effective default for a rule with no parent is `["**"]`.
    pub include_match: Option<Vec<String>>,

    // ---- Layer 5: optional include directory resolution -------------------
    /// Literal directory paths under the project root (relative to it) that
    /// are probed after `include_forms` and `include_match` pass. NOT a glob;
    /// no implicit `/**` suffix; no `.gitignore` semantics. When this list is
    /// empty, no directory-resolution policy is applied.
    pub include_directories: Option<Vec<String>>,
    /// Policy for a matched include when `include_directories` is non-empty
    /// and no directory contains the include argument. Effective default is
    /// `error`. `allow` keeps the rule matched for non-`resolve` actions; it
    /// is rejected with action `resolve`.
    pub include_on_unresolved: Option<IncludeOnUnresolved>,
    /// Policy for a matched include when `include_directories` is non-empty
    /// and more than one directory contains the include argument. Effective
    /// default is `error`; `first` selects the first matching directory in
    /// declaration order.
    pub include_on_ambiguous: Option<IncludeOnAmbiguous>,

    /// Action to run when all match layers pass. If neither this rule nor any
    /// copied ancestor sets an action, the effective action is
    /// `{ type = "keep", output_form = "preserve" }`. The whole field can also
    /// be the string `"${copied}"` to reuse the parent's resolved action.
    pub action: Option<MaybeCopiedObject<RawAction>>,

    /// Optional trailing-comment transform and/or append rule for
    /// `resolve`/`replace`/`keep` actions. The whole field can also be the
    /// string `"${copied}"` to reuse the parent's resolved value verbatim.
    pub trailing_comment: Option<MaybeCopiedObject<RawTrailingComment>>,
}

/// Suppression markers: regex patterns matched line-by-line. `line` suppresses
/// only matching lines. `block_start` suppresses from its matching line until a
/// later `block_end` match, inclusive. If `block_end` is omitted, suppression
/// continues to the end of the file after `block_start` matches.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSuppression {
    /// Regex that starts an off-limits block.
    pub block_start: Option<String>,
    /// Regex that ends an off-limits block.
    pub block_end: Option<String>,
    /// Regex that suppresses a single line.
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

/// Policy when `include_directories` is configured but no directory contains
/// the include argument.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IncludeOnUnresolved {
    /// Report an evaluation failure.
    Error,
    /// Treat this rule as not matched for this include.
    Skip,
    /// Keep this rule matched. Rejected for `resolve` actions.
    Allow,
}

/// Policy when `include_directories` is configured and multiple directories
/// contain the include argument.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IncludeOnAmbiguous {
    /// Report an evaluation failure.
    Error,
    /// Treat this rule as not matched for this include.
    Skip,
    /// Select the first matching include directory in declaration order.
    First,
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

/// The action a rule executes on a matched `#include`. This is a tagged object
/// using `type = "resolve"`, `"replace"`, `"keep"`, `"remove"`,
/// `"comment_out"`, or `"error"`.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawAction {
    /// Rewrite the include using the header path selected by
    /// `include_directories`, relative to `relative_to`. `${current_file}`
    /// means the directory of the file being edited. Default `output_form`
    /// is `preserve`; default `message` is empty.
    Resolve {
        /// Base path for the rewritten include. Use `${current_file}` for the
        /// current source file's directory.
        relative_to: String,
        /// Output delimiter form. Defaults to `preserve`.
        #[serde(default)]
        output_form: Option<OutputForm>,
        /// Optional diagnostic message string. `${current_file}` and
        /// `${original}` placeholders are supported where messages are emitted.
        #[serde(default)]
        message: Option<String>,
    },
    /// Replace the include argument with `with`. Default `output_form` is
    /// `preserve`; default `message` is empty.
    Replace {
        /// Replacement include argument. Supports `${original}` and
        /// `${current_file}` placeholders.
        with: String,
        /// Output delimiter form. Defaults to `preserve`.
        #[serde(default)]
        output_form: Option<OutputForm>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Leave the include's argument alone (the form may still change via
    /// `output_form`). Default `output_form` is `preserve`; default `message`
    /// is empty.
    Keep {
        /// Output delimiter form. Defaults to `preserve`.
        #[serde(default)]
        output_form: Option<OutputForm>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Delete the entire `#include` line. By default no blank line is kept and
    /// a same-line trailing comment is kept on its own line.
    Remove {
        /// Keep the line terminator as a blank line. Defaults to `false`.
        #[serde(default)]
        keep_blank_line: Option<bool>,
        /// Preserve a recognized same-line trailing comment on its own line.
        /// Defaults to `true`.
        #[serde(default)]
        keep_trailing_comment: Option<bool>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Wrap the include line in `//` (default) or `/* */` delimiters. Default
    /// `message` is empty.
    CommentOut {
        /// Comment delimiter style. Defaults to `//`.
        #[serde(default)]
        style: Option<CommentStyle>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Report a user-facing error for the matched include. Exit code 2.
    Error {
        /// Error message. Supports `${original}` and `${current_file}`.
        /// Defaults to an empty string.
        #[serde(default)]
        message: Option<String>,
    },
}

/// Trailing-comment configuration for `resolve`, `replace`, and `keep`
/// actions. Cross-line block comments after an include are not considered
/// trailing comments and are left alone.
#[derive(Debug, Default, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawTrailingComment {
    /// Optional transform to run when an existing same-line trailing comment
    /// matches.
    pub transform: Option<RawTrailingTransform>,
    /// Literal text to append to the include line when there is no
    /// trailing comment after action evaluation. The user writes the full
    /// comment text, including delimiters and leading whitespace. It must not
    /// contain line terminators.
    pub append_if_absent: Option<String>,
}

/// Trailing-comment transform: matches an existing comment, then runs an
/// action over it. `action` is required so a transform cannot silently become
/// a no-op because of an omitted nested action.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawTrailingTransform {
    /// Existing comment styles that can match. Effective default is both
    /// `//` and `/**/`.
    pub match_styles: Option<Vec<CommentStyle>>,
    /// Regex matched against the trimmed trailing-comment body. Effective
    /// default is `.*`.
    pub content_regex: Option<String>,
    /// Action to run when style and regex both match.
    pub action: RawTrailingAction,
}

/// The action a trailing-comment transform runs on its match.
#[derive(Debug, Deserialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawTrailingAction {
    /// Replace the comment body with `with`. Default `output_style` is
    /// `preserve`; default `message` is empty.
    Replace {
        /// Replacement comment body. Supports `${original}` and
        /// `${current_file}` placeholders.
        with: String,
        /// Output comment delimiter style. Defaults to `preserve`.
        #[serde(default)]
        output_style: Option<OutputCommentStyle>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Keep the comment body; only `output_style` may change. Default
    /// `output_style` is `preserve`; default `message` is empty.
    Keep {
        /// Output comment delimiter style. Defaults to `preserve`.
        #[serde(default)]
        output_style: Option<OutputCommentStyle>,
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Remove the trailing comment entirely. Default `message` is empty.
    Remove {
        /// Optional diagnostic message string.
        #[serde(default)]
        message: Option<String>,
    },
    /// Report a user-facing error when the transform matches.
    Error {
        /// Error message. Supports `${original}` for the original comment body
        /// and `${current_file}` for the edited source path.
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
            include_forms = ["quote"]
            include_match = ["**/foo.h"]
            include_directories = ["src/internal"]
            include_on_unresolved = "skip"
            include_on_ambiguous = "first"
            action = { type = "resolve", relative_to = "include" }
            "#,
        )
        .raw;
        let r = &cfg.rules[0];
        assert_eq!(r.file_paths.as_ref().unwrap().len(), 2);
        assert_eq!(r.include_forms.as_ref().unwrap(), &vec![IncludeForm::Quote]);
        assert_eq!(
            r.include_match.as_ref().unwrap(),
            &vec!["**/foo.h".to_string()]
        );
        assert_eq!(r.include_on_unresolved, Some(IncludeOnUnresolved::Skip));
        assert_eq!(r.include_on_ambiguous, Some(IncludeOnAmbiguous::First));
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
    fn old_match_forms_field_is_rejected() {
        let err = parse(
            &format!(
                "{}{}",
                &*crate::utils::testing::config::MIN_PROJECT_BLOCK,
                r#"
                [[rule]]
                name = "base"
                match_forms = ["quote"]
                "#,
            ),
            Path::new("inclean.toml"),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown field `match_forms`"), "{msg}");
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
