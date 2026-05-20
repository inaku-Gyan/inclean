//! Static lint: detect child rules whose layer 1 / 2 / 3 widens the
//! parent's. The runtime engine already enforces the subset invariant via
//! AND-combination, so widening here is at worst a misleading config — the
//! rule will silently match a narrower set than the user expects. We
//! surface that with warnings.
//!
//! - **Layer 2 (extensions)**: strict set containment.
//! - **Layer 3 (forms)**: strict set containment.
//! - **Layer 1 (paths)**: path-component-prefix heuristic. We compute each
//!   glob's "static prefix" (the leading run of components with no glob
//!   meta-chars), and a child glob is considered covered if some parent
//!   glob's static prefix is a component-wise prefix of the child's. Catches
//!   the common mistake of `parent = ["src/**"]` + `child = ["**"]` or
//!   adding a brand-new top-level path to a child. False positives are
//!   possible (we return them as warnings, not errors) but in practice this
//!   shape matches the way users write `paths`.
//! - **Layer 4 (match)**: regex containment is undecidable in practice; no
//!   static check.

use std::collections::{BTreeMap, HashSet};

use super::inherit::ResolvedRule;
use super::schema::IncludeForm;

/// A single widening finding. Carries enough context to produce a useful
/// human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub child: String,
    pub parent: String,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    /// Layer 1 — at least one child path glob is not covered by any parent
    /// glob (under the static-prefix heuristic).
    PathsWiden { uncovered: Vec<String> },
    /// Layer 2 — child specifies extensions absent from parent's set.
    ExtensionsWiden { extra: Vec<String> },
    /// Layer 3 — child specifies forms absent from parent's set.
    FormsWiden { extra: Vec<IncludeForm> },
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            WarningKind::PathsWiden { uncovered } => write!(
                f,
                "rule `{}` extends `{}` but adds path glob(s) {:?} not covered by the parent's paths",
                self.child, self.parent, uncovered
            ),
            WarningKind::ExtensionsWiden { extra } => write!(
                f,
                "rule `{}` extends `{}` but adds extensions {:?} not in the parent's set",
                self.child, self.parent, extra
            ),
            WarningKind::FormsWiden { extra } => write!(
                f,
                "rule `{}` extends `{}` but adds forms {:?} not in the parent's set",
                self.child, self.parent, extra
            ),
        }
    }
}

/// Scan all resolved rules and return widening warnings.
pub fn check(rules: &BTreeMap<String, ResolvedRule>) -> Vec<Warning> {
    let mut out = Vec::new();
    for rule in rules.values() {
        let Some(parent_name) = rule.extends.as_deref() else {
            continue;
        };
        let Some(parent) = rules.get(parent_name) else {
            // Missing parent is caught earlier by inherit::resolve.
            continue;
        };

        // ---- Layer 1: paths ----
        let mut uncovered = Vec::new();
        for child_glob in &rule.paths {
            if !parent
                .paths
                .iter()
                .any(|p| static_prefix_covers(p, child_glob))
            {
                uncovered.push(child_glob.clone());
            }
        }
        if !uncovered.is_empty() {
            out.push(Warning {
                child: rule.name.clone(),
                parent: parent.name.clone(),
                kind: WarningKind::PathsWiden { uncovered },
            });
        }

        // ---- Layer 2: extensions ----
        let parent_exts: HashSet<&str> = parent.extensions.iter().map(String::as_str).collect();
        let extra_exts: Vec<String> = rule
            .extensions
            .iter()
            .filter(|e| !parent_exts.contains(e.as_str()))
            .cloned()
            .collect();
        if !extra_exts.is_empty() {
            out.push(Warning {
                child: rule.name.clone(),
                parent: parent.name.clone(),
                kind: WarningKind::ExtensionsWiden { extra: extra_exts },
            });
        }

        // ---- Layer 3: forms ----
        let parent_forms: HashSet<IncludeForm> = parent.forms.iter().copied().collect();
        let extra_forms: Vec<IncludeForm> = rule
            .forms
            .iter()
            .copied()
            .filter(|f| !parent_forms.contains(f))
            .collect();
        if !extra_forms.is_empty() {
            out.push(Warning {
                child: rule.name.clone(),
                parent: parent.name.clone(),
                kind: WarningKind::FormsWiden { extra: extra_forms },
            });
        }
    }
    out
}

/// Return true if `parent_glob`'s static prefix (component-wise, up to the
/// first wildcard component) is a prefix of `child_glob`'s static prefix.
fn static_prefix_covers(parent_glob: &str, child_glob: &str) -> bool {
    let parent_prefix = static_prefix_components(parent_glob);
    let child_prefix = static_prefix_components(child_glob);
    if parent_prefix.len() > child_prefix.len() {
        return false;
    }
    parent_prefix
        .iter()
        .zip(child_prefix.iter())
        .all(|(p, c)| p == c)
}

/// Components of `glob` up to the first one containing glob meta-characters.
/// Leading slashes and empty components are kept (an empty result means the
/// glob is wildcarded at the very first component, e.g. `**`).
fn static_prefix_components(glob: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for part in glob.split('/') {
        if part.contains(['*', '?', '[', '{', '!']) {
            break;
        }
        out.push(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::{resolve, ResolvedRule};
    use crate::config::schema::{parse, IncludeForm, LoadedConfig};
    use std::path::PathBuf;

    fn load(body: &str) -> Vec<LoadedConfig> {
        vec![LoadedConfig {
            path: PathBuf::from("/p/inclean.toml"),
            raw: parse(body, &PathBuf::from("/p/inclean.toml")).unwrap(),
        }]
    }

    fn resolved(body: &str) -> BTreeMap<String, ResolvedRule> {
        resolve(&load(body)).unwrap()
    }

    #[test]
    fn empty_rule_set_yields_no_warnings() {
        let map = BTreeMap::new();
        assert!(check(&map).is_empty());
    }

    #[test]
    fn root_rule_without_parent_yields_no_warnings() {
        let map = resolved(r#"
            [[rule]]
            name = "base"
            extensions = [".c", ".h"]
        "#);
        assert!(check(&map).is_empty());
    }

    #[test]
    fn child_with_same_or_narrower_fields_is_fine() {
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            extensions = [".c", ".cpp", ".h"]
            forms = ["quote", "angle"]

            [[rule]]
            name = "child"
            extends = "base"
            paths = ["src/foo/**"]
            extensions = [".c", ".h"]
            forms = ["quote"]
            "#,
        );
        assert!(check(&map).is_empty(), "got: {:?}", check(&map));
    }

    #[test]
    fn child_extending_extensions_triggers_warning() {
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            extensions = [".c", ".h"]

            [[rule]]
            name = "child"
            extends = "base"
            extensions = [".c", ".cpp", ".h"]
            "#,
        );
        let warnings = check(&map);
        assert_eq!(warnings.len(), 1);
        match &warnings[0].kind {
            WarningKind::ExtensionsWiden { extra } => assert_eq!(extra, &vec![".cpp".to_string()]),
            _ => panic!("expected ExtensionsWiden"),
        }
    }

    #[test]
    fn child_extending_forms_triggers_warning() {
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            forms = ["quote"]

            [[rule]]
            name = "child"
            extends = "base"
            forms = ["quote", "angle"]
            "#,
        );
        let warnings = check(&map);
        assert_eq!(warnings.len(), 1);
        match &warnings[0].kind {
            WarningKind::FormsWiden { extra } => assert_eq!(extra, &vec![IncludeForm::Angle]),
            _ => panic!("expected FormsWiden"),
        }
    }

    #[test]
    fn child_adding_top_level_path_triggers_warning() {
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]

            [[rule]]
            name = "child"
            extends = "base"
            paths = ["src/**", "lib/**"]
            "#,
        );
        let warnings = check(&map);
        assert_eq!(warnings.len(), 1);
        match &warnings[0].kind {
            WarningKind::PathsWiden { uncovered } => {
                assert_eq!(uncovered, &vec!["lib/**".to_string()])
            }
            _ => panic!("expected PathsWiden"),
        }
    }

    #[test]
    fn parent_with_universal_glob_never_warns_on_paths() {
        // Parent's "**" has empty static prefix → covers everything.
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            paths = ["**"]

            [[rule]]
            name = "child"
            extends = "base"
            paths = ["src/foo/**", "lib/bar.c"]
            "#,
        );
        assert!(check(&map).is_empty(), "got: {:?}", check(&map));
    }

    #[test]
    fn static_prefix_components_truncates_at_wildcards() {
        assert_eq!(static_prefix_components("src/**"), vec!["src"]);
        assert_eq!(static_prefix_components("src/foo/**"), vec!["src", "foo"]);
        assert_eq!(static_prefix_components("**"), Vec::<&str>::new());
        assert_eq!(static_prefix_components("src/*/foo"), vec!["src"]);
        assert_eq!(
            static_prefix_components("src/foo.c"),
            vec!["src", "foo.c"]
        );
    }

    #[test]
    fn exact_path_widening_is_detected() {
        let map = resolved(
            r#"
            [[rule]]
            name = "base"
            paths = ["src/foo.c"]

            [[rule]]
            name = "child"
            extends = "base"
            paths = ["src/bar.c"]
            "#,
        );
        let warnings = check(&map);
        assert_eq!(warnings.len(), 1);
        assert!(matches!(warnings[0].kind, WarningKind::PathsWiden { .. }));
    }
}
