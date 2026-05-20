//! Rule-tree invariants checked at `check --no-rewrites` and above.
//!
//! For each `#include` in the project we ask the engine for **every** rule
//! whose five layers match (via [`super::engine::match_all`]). The
//! resulting candidate set *M* must satisfy two properties:
//!
//! 1. **Subset (child ⊆ parent)** — for every `R ∈ M`, every ancestor of
//!    `R` along the `extends` chain must also be in *M*. Otherwise some
//!    child rule has been widened past its parent's match set.
//! 2. **Same chain (cross-chain disjoint)** — any two rules in *M* must
//!    have an ancestor/descendant relationship. Otherwise two unrelated
//!    rules both match the same include, and first-match-wins behavior
//!    depends on declaration order rather than rule semantics.
//!
//! Both conditions reduce to: *M* must be a chain in the `extends` forest.
//! Implementation collects ancestors for each match, asserts inclusion,
//! and then walks the deepest-first ordering to find the first non-chain
//! adjacent pair.

use std::collections::BTreeMap;

use super::engine::CompiledRule;

/// What kind of rule-tree invariant was violated.
#[derive(Debug)]
pub enum ConflictKind<'a> {
    /// A rule matched but one of its ancestors did not — the child has
    /// widened past the parent.
    ChildWiderThanParent {
        child: &'a CompiledRule<'a>,
        missing_ancestor: &'a CompiledRule<'a>,
    },
    /// Two rules matched but neither is an ancestor of the other.
    CrossChain {
        a: &'a CompiledRule<'a>,
        b: &'a CompiledRule<'a>,
    },
}

/// Verify `matched` forms a single chain in the `extends` forest. Returns
/// the deepest rule on success (the one first-match-wins would pick), or
/// the first detected violation.
///
/// `by_name` maps rule name → compiled rule for O(1) ancestor lookup. It
/// must contain every rule referenced by `matched` and their ancestors —
/// in practice it's built once over the full compiled-rule list.
pub fn check_chain<'a>(
    matched: &[&'a CompiledRule<'a>],
    by_name: &BTreeMap<String, &'a CompiledRule<'a>>,
) -> Result<Option<&'a CompiledRule<'a>>, ConflictKind<'a>> {
    if matched.is_empty() {
        return Ok(None);
    }

    let matched_names: std::collections::HashSet<&str> =
        matched.iter().map(|r| r.rule.name.as_str()).collect();

    // (1) Every ancestor of every matched rule must also be matched.
    for r in matched {
        let mut cursor = r.rule.extends.as_deref();
        while let Some(parent_name) = cursor {
            if !matched_names.contains(parent_name) {
                let missing = by_name
                    .get(parent_name)
                    .copied()
                    .expect("parent name resolved at config-load time");
                return Err(ConflictKind::ChildWiderThanParent {
                    child: r,
                    missing_ancestor: missing,
                });
            }
            cursor = by_name
                .get(parent_name)
                .and_then(|cr| cr.rule.extends.as_deref());
        }
    }

    // (2) Order by depth (deepest first); every adjacent pair must be in
    // an ancestor/descendant relationship. Because (1) holds, "depth(B)
    // < depth(A) and A is in chain ⇒ B is an ancestor of A" — checking
    // adjacency is sufficient.
    let mut ordered: Vec<&CompiledRule<'_>> = matched.to_vec();
    ordered.sort_by_key(|r| std::cmp::Reverse(depth_of(r, by_name)));

    for pair in ordered.windows(2) {
        let deep = pair[0];
        let shallow = pair[1];
        if !is_ancestor(shallow, deep, by_name) {
            return Err(ConflictKind::CrossChain {
                a: deep,
                b: shallow,
            });
        }
    }

    Ok(Some(ordered[0]))
}

/// `maybe_ancestor` is an ancestor of `r` along `extends`?
fn is_ancestor<'a>(
    maybe_ancestor: &'a CompiledRule<'a>,
    r: &'a CompiledRule<'a>,
    by_name: &BTreeMap<String, &'a CompiledRule<'a>>,
) -> bool {
    let target = maybe_ancestor.rule.name.as_str();
    let mut cursor = r.rule.extends.as_deref();
    while let Some(parent) = cursor {
        if parent == target {
            return true;
        }
        cursor = by_name
            .get(parent)
            .and_then(|cr| cr.rule.extends.as_deref());
    }
    false
}

fn depth_of<'a>(r: &'a CompiledRule<'a>, by_name: &BTreeMap<String, &'a CompiledRule<'a>>) -> usize {
    let mut d = 0usize;
    let mut cursor = r.rule.extends.as_deref();
    while let Some(parent) = cursor {
        d += 1;
        cursor = by_name
            .get(parent)
            .and_then(|cr| cr.rule.extends.as_deref());
    }
    d
}

/// Build a `name → &CompiledRule` index. Call once per pipeline run.
pub fn index_by_name<'a, 'b>(
    rules: &'a [CompiledRule<'b>],
) -> BTreeMap<String, &'a CompiledRule<'b>> {
    rules
        .iter()
        .map(|r| (r.rule.name.clone(), r))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::{resolve, ResolvedRule};
    use crate::config::schema::{parse, LoadedConfig};
    use std::collections::BTreeMap as Map;
    use std::path::PathBuf;

    fn cfg(body: &str) -> Map<String, ResolvedRule> {
        let configs = vec![LoadedConfig {
            path: PathBuf::from("/proj/inclean.toml"),
            raw: parse(body, &PathBuf::from("/proj/inclean.toml")).unwrap(),
        }];
        resolve(&configs).unwrap()
    }

    fn compile(resolved: &Map<String, ResolvedRule>) -> Vec<CompiledRule<'_>> {
        let root = PathBuf::from("/proj");
        resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect()
    }

    #[test]
    fn empty_matched_is_ok() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "a"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let res = check_chain(&[], &by_name).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn single_rule_is_ok() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "a"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let r = by_name["a"];
        let res = check_chain(&[r], &by_name).unwrap().unwrap();
        assert_eq!(res.rule.name, "a");
    }

    #[test]
    fn parent_and_child_form_a_chain() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "parent"

            [[rule]]
            name = "child"
            extends = "parent"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let matched = vec![by_name["parent"], by_name["child"]];
        let deepest = check_chain(&matched, &by_name).unwrap().unwrap();
        assert_eq!(deepest.rule.name, "child");
    }

    #[test]
    fn child_without_parent_is_child_wider_than_parent() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "parent"

            [[rule]]
            name = "child"
            extends = "parent"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let err = check_chain(&[by_name["child"]], &by_name).unwrap_err();
        match err {
            ConflictKind::ChildWiderThanParent { child, missing_ancestor } => {
                assert_eq!(child.rule.name, "child");
                assert_eq!(missing_ancestor.rule.name, "parent");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn two_unrelated_roots_are_cross_chain() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "a"

            [[rule]]
            name = "b"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let err = check_chain(&[by_name["a"], by_name["b"]], &by_name).unwrap_err();
        match err {
            ConflictKind::CrossChain { .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cousins_are_cross_chain() {
        // root -> child_a, root -> child_b. Matching {child_a, child_b}
        // satisfies "parents present" (root not in M → child wider error first).
        // To isolate cross-chain, match {root, child_a, child_b}: the
        // adjacency check between child_a and child_b will fail.
        let resolved = cfg(
            r#"
            [[rule]]
            name = "root"

            [[rule]]
            name = "child_a"
            extends = "root"

            [[rule]]
            name = "child_b"
            extends = "root"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let err = check_chain(
            &[by_name["root"], by_name["child_a"], by_name["child_b"]],
            &by_name,
        )
        .unwrap_err();
        match err {
            ConflictKind::CrossChain { .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn three_level_chain_is_ok() {
        let resolved = cfg(
            r#"
            [[rule]]
            name = "gp"

            [[rule]]
            name = "p"
            extends = "gp"

            [[rule]]
            name = "c"
            extends = "p"
            "#,
        );
        let compiled = compile(&resolved);
        let by_name = index_by_name(&compiled);
        let deepest = check_chain(&[by_name["gp"], by_name["p"], by_name["c"]], &by_name)
            .unwrap()
            .unwrap();
        assert_eq!(deepest.rule.name, "c");
    }
}
