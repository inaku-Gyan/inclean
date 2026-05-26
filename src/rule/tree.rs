//! ⚠️ M1 transitional stub.
//!
//! Rule-tree invariants disappear in v0.3 — conflicts are now detected by
//! comparing the final text each matched rule would produce (M5). This
//! file is kept around solely to satisfy `use crate::rule::tree::{...}`
//! imports during the transition. It will be deleted in M4.

use std::collections::BTreeMap;

use super::engine::CompiledRule;

#[derive(Debug)]
pub enum ConflictKind<'a> {
    ChildWiderThanParent {
        child: &'a CompiledRule<'a>,
        missing_ancestor: &'a CompiledRule<'a>,
    },
    CrossChain {
        a: &'a CompiledRule<'a>,
        b: &'a CompiledRule<'a>,
    },
}

/// During the transition this always reports "single chain, deepest =
/// first matched". M5 introduces the new conflict-by-final-text criterion.
pub fn check_chain<'a>(
    matched: &[&'a CompiledRule<'a>],
    _by_name: &BTreeMap<String, &'a CompiledRule<'a>>,
) -> Result<Option<&'a CompiledRule<'a>>, ConflictKind<'a>> {
    Ok(matched.first().copied())
}

pub fn index_by_name<'a, 'b>(
    rules: &'a [CompiledRule<'b>],
) -> BTreeMap<String, &'a CompiledRule<'b>> {
    rules.iter().map(|r| (r.rule.name.clone(), r)).collect()
}
