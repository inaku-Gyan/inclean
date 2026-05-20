//! Raw serde structures for `inclean.toml`.
//!
//! These types deserialize directly from TOML. Defaults, `@std.*` constant
//! expansion, `extends` resolution, and field merging happen in later passes
//! (`constants::expand`, `inherit::resolve`). Keep this module free of policy.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The top-level shape of a single `inclean.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    #[serde(default)]
    pub project: Option<RawProject>,

    #[serde(default, rename = "rule")]
    pub rules: Vec<RawRule>,
}

/// The `[project]` block. Intentionally minimal: only `root`. All other
/// project-wide values live on rules (with inheritance providing reuse).
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RawProject {
    /// Project root, defaults to the directory of the top-level
    /// `inclean.toml`. Resolved by `discover`.
    pub root: Option<String>,
}

/// A single `[[rule]]` entry, before defaulting / inheritance / constant
/// expansion. `Option<_>` distinguishes "user did not specify" from "user
/// wrote empty".
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RawRule {
    /// Globally unique across the entire project.
    pub name: String,

    /// Name of the parent rule. `None` means this rule is a root of its
    /// inheritance tree (typically the conventional `base`).
    pub extends: Option<String>,

    // ---- Layer 1: paths (gitignore-style globs) ---------------------------
    pub paths: Option<Vec<String>>,

    // ---- Layer 2: extensions ---------------------------------------------
    pub extensions: Option<Vec<String>>,

    // ---- Layer 3: include forms ------------------------------------------
    pub forms: Option<Vec<IncludeForm>>,

    // ---- Layer 4: regex on stripped include content ----------------------
    #[serde(rename = "match")]
    pub match_regex: Option<String>,

    // ---- Layer 5: match resolved physical file (v1 unsupported) ----------
    /// Reserved field. If present, config loading must error with a clear
    /// "not yet supported in v1" message. Stored as a free-form value so we
    /// can detect the presence without binding to a future shape.
    pub match_resolved: Option<toml::Value>,

    // ---- Non-matching configuration --------------------------------------
    pub allowed_include_dirs: Option<Vec<String>>,
    pub original_include_dirs: Option<Vec<String>>,

    pub action: Option<RawAction>,
}

/// The include "form": which quoting style of `#include` a rule applies to.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum IncludeForm {
    /// `#include "foo.h"`
    Quote,
    /// `#include <foo.h>`
    Angle,
    /// `#include MY_HEADER` (macro-defined). v1: matching this form is an
    /// explicit, configurable possibility but execution must error out.
    Macro,
}

/// The action a rule executes on a matched `#include`. Tagged by `type`.
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum RawAction {
    /// Resolve the include via the rule's `original_include_dirs`, then
    /// emit a path relative to the chosen base. Default action if omitted.
    Auto {
        #[serde(default)]
        relative_to: Option<AutoRelativeTo>,
        #[serde(default)]
        form: Option<OutputForm>,
    },
    /// Replace the include text with `to`, supporting `${...}` placeholders.
    Rewrite {
        to: String,
        #[serde(default)]
        form: Option<OutputForm>,
    },
    /// Leave the include unchanged; stop trying further rules.
    Keep,
    /// Abort processing of the file with the user-provided message.
    Error { message: String },
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoRelativeTo {
    /// Path relative to one of the rule's `allowed_include_dirs` (default).
    Allowed,
    /// Path relative to the directory of the file being edited.
    FileDir,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputForm {
    /// `#include "..."`
    Quote,
    /// `#include <...>`
    Angle,
    /// Keep whatever form the original `#include` used.
    Preserve,
}

/// A file that has been loaded into memory, paired with its on-disk path.
/// `discover` produces a sequence of these.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub path: std::path::PathBuf,
    pub raw: RawConfig,
}

/// Parse the contents of a single `inclean.toml` text. Path-aware errors
/// (line + column) are bubbled up via `anyhow`.
pub fn parse(text: &str, source_path: &std::path::Path) -> anyhow::Result<RawConfig> {
    toml::from_str::<RawConfig>(text).map_err(|err| {
        anyhow::anyhow!(
            "failed to parse inclean.toml at {}: {err}",
            source_path.display()
        )
    })
}

/// Helper used by tests and by other passes to look up rules by name across
/// a set of loaded configs. Returns an error on duplicate names because
/// rule names must be globally unique.
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

/// Where a rule was defined, used for error messages and for the resolved
/// rule-name -> rule lookup table.
#[derive(Debug, Clone, Copy)]
pub struct RuleLocator<'a> {
    pub config_path: &'a std::path::Path,
    pub index: usize,
    pub rule: &'a RawRule,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse_str(s: &str) -> RawConfig {
        parse(s, Path::new("test.toml")).expect("parse")
    }

    #[test]
    fn empty_config_is_valid() {
        let cfg = parse_str("");
        assert!(cfg.project.is_none());
        assert!(cfg.rules.is_empty());
    }

    #[test]
    fn project_root_round_trips() {
        let cfg = parse_str(
            r#"
            [project]
            root = "src"
            "#,
        );
        assert_eq!(cfg.project.unwrap().root.unwrap(), "src");
    }

    #[test]
    fn unknown_project_field_is_rejected() {
        let err = parse(
            r#"
            [project]
            root = "."
            sources = ["src/**"]
            "#,
            Path::new("t.toml"),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("sources"), "should mention rejected field: {msg}");
    }

    #[test]
    fn base_rule_minimal() {
        let cfg = parse_str(
            r#"
            [[rule]]
            name = "base"
            "#,
        );
        assert_eq!(cfg.rules.len(), 1);
        assert_eq!(cfg.rules[0].name, "base");
        assert!(cfg.rules[0].extends.is_none());
        assert!(cfg.rules[0].action.is_none());
    }

    #[test]
    fn full_rule_with_inline_action() {
        let cfg = parse_str(
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**", "include/**"]
            extensions = ["@std.all_extensions"]
            forms = ["quote"]
            match = '^([^/]+\.h)$'
            allowed_include_dirs = ["include"]
            original_include_dirs = ["src", "src/internal"]
            action = { type = "rewrite", to = "mylib/internal/${1}", form = "quote" }
            "#,
        );
        let r = &cfg.rules[0];
        assert_eq!(r.paths.as_ref().unwrap().len(), 2);
        assert_eq!(r.forms.as_ref().unwrap(), &vec![IncludeForm::Quote]);
        assert_eq!(r.match_regex.as_deref(), Some(r"^([^/]+\.h)$"));
        match r.action.as_ref().unwrap() {
            RawAction::Rewrite { to, form } => {
                assert_eq!(to, "mylib/internal/${1}");
                assert_eq!(*form, Some(OutputForm::Quote));
            }
            _ => panic!("expected rewrite action"),
        }
    }

    #[test]
    fn auto_action_defaults_are_none_at_parse_time() {
        // Parsing leaves sub-fields as None; defaulting happens later in the
        // pipeline. This makes "user wrote nothing" distinguishable from
        // "user wrote default", which inheritance needs.
        let cfg = parse_str(
            r#"
            [[rule]]
            name = "base"
            action = { type = "auto" }
            "#,
        );
        match cfg.rules[0].action.as_ref().unwrap() {
            RawAction::Auto { relative_to, form } => {
                assert!(relative_to.is_none());
                assert!(form.is_none());
            }
            _ => panic!("expected auto"),
        }
    }

    #[test]
    fn error_action_requires_message() {
        let err = toml::from_str::<RawConfig>(
            r#"
            [[rule]]
            name = "x"
            action = { type = "error" }
            "#,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("message"));
    }

    #[test]
    fn match_resolved_is_preserved_as_raw_value() {
        // It must be detected (so we can error) but is not strictly typed yet.
        let cfg = parse_str(
            r#"
            [[rule]]
            name = "x"
            match_resolved = { kind = "exact" }
            "#,
        );
        assert!(cfg.rules[0].match_resolved.is_some());
    }

    #[test]
    fn duplicate_rule_names_across_configs_are_rejected() {
        let a = LoadedConfig {
            path: "/proj/inclean.toml".into(),
            raw: parse_str(
                r#"
                [[rule]]
                name = "base"
                "#,
            ),
        };
        let b = LoadedConfig {
            path: "/proj/src/inclean.toml".into(),
            raw: parse_str(
                r#"
                [[rule]]
                name = "base"
                "#,
            ),
        };
        let err = index_rules_by_name([&a, &b]).unwrap_err();
        assert!(format!("{err}").contains("duplicate rule name"));
    }

    #[test]
    fn unique_rule_names_across_configs_are_accepted() {
        let a = LoadedConfig {
            path: "/proj/inclean.toml".into(),
            raw: parse_str(
                r#"
                [[rule]]
                name = "base"
                "#,
            ),
        };
        let b = LoadedConfig {
            path: "/proj/src/inclean.toml".into(),
            raw: parse_str(
                r#"
                [[rule]]
                name = "internal"
                extends = "base"
                "#,
            ),
        };
        let map = index_rules_by_name([&a, &b]).unwrap();
        assert!(map.contains_key("base"));
        assert!(map.contains_key("internal"));
    }
}
