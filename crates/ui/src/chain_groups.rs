//! Grouping a dashboard section's rows by relation chain.
//!
//! A row is built on another when its current patch set's first parent is
//! that row's current patch set ([`ChangeSummary::current_commit`]), or,
//! for a chain built on an outdated patch set, when the row's cached
//! related changes say so (the `outdated` links,
//! `Syncer::cached_chain_parents`). Only rows of the section take part: a
//! chain the query lists only part of shows that part.
//!
//! Each chain is shown as one block, newest descendant first like Gerrit's
//! related changes, where its first row was in the server's order. Rows
//! outside any chain keep their place.

use std::collections::HashMap;

use model::{ChangeNumber, ChangeSummary, RevisionId};

/// Where a row sits in a chain of two or more rows, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainPlace {
    /// The top: nothing in the section is built on it.
    Tip,
    Middle,
    /// The bottom: built on nothing in the section.
    Base,
}

/// Indices into `rows` in the order to show them, each with its place in
/// its chain (`None` for a row alone).
pub fn group_by_chain(
    rows: &[ChangeSummary],
    outdated: &HashMap<ChangeNumber, ChangeNumber>,
) -> Vec<(usize, Option<ChainPlace>)> {
    let by_sha: HashMap<&RevisionId, usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.current_commit.as_ref().map(|k| (&k.sha, i)))
        .collect();
    let by_number: HashMap<ChangeNumber, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, c)| (c.number, i))
        .collect();
    let parent: Vec<Option<usize>> = rows
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let first = c.current_commit.as_ref()?.parents.first();
            first
                .and_then(|p| by_sha.get(p).copied())
                .or_else(|| {
                    outdated
                        .get(&c.number)
                        .and_then(|n| by_number.get(n).copied())
                })
                .filter(|&p| p != i)
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = *p {
            children[p].push(i);
        }
    }

    // Each chain from its base, children in server order; a row on a
    // cycle (only possible with inconsistent links) starts a chain of its
    // own when no base reached it.
    let mut seen = vec![false; rows.len()];
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let bases = (0..rows.len()).filter(|&i| parent[i].is_none());
    for start in bases.chain(0..rows.len()) {
        if seen[start] {
            continue;
        }
        let mut chain = Vec::new();
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            chain.push(i);
            stack.extend(children[i].iter().rev());
        }
        chain.reverse();
        chains.push(chain);
    }
    chains.sort_by_key(|c| c.iter().min().copied());

    let mut out = Vec::with_capacity(rows.len());
    for chain in chains {
        let last = chain.len() - 1;
        for (k, i) in chain.into_iter().enumerate() {
            let place = match k {
                _ if last == 0 => None,
                0 => Some(ChainPlace::Tip),
                k if k == last => Some(ChainPlace::Base),
                _ => Some(ChainPlace::Middle),
            };
            out.push((i, place));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use model::CurrentCommit;

    use super::*;
    use crate::testing::summary;

    /// Change `n` at commit `sha` on `parent`.
    fn row(n: u64, sha: &str, parent: &str) -> ChangeSummary {
        ChangeSummary {
            current_commit: Some(CurrentCommit {
                sha: sha.into(),
                parents: vec![parent.into()],
            }),
            ..summary(n)
        }
    }

    fn shown(rows: &[ChangeSummary], groups: &[(usize, Option<ChainPlace>)]) -> Vec<u64> {
        groups.iter().map(|(i, _)| rows[*i].number.0).collect()
    }

    #[test]
    fn a_chain_is_one_block_tip_first_where_its_first_row_was() {
        use ChainPlace::*;
        // Server order (most recently updated first): 9 alone, then the
        // base of the chain 1 <- 2 <- 3, 8 alone, then the rest.
        let rows = [
            row(9, "i", "main"),
            row(1, "a", "main"),
            row(8, "h", "main"),
            row(3, "c", "b"),
            row(2, "b", "a"),
        ];
        let groups = group_by_chain(&rows, &HashMap::new());
        assert_eq!(shown(&rows, &groups), [9, 3, 2, 1, 8]);
        let places: Vec<_> = groups.iter().map(|(_, p)| *p).collect();
        assert_eq!(places, [None, Some(Tip), Some(Middle), Some(Base), None]);
    }

    #[test]
    fn rows_without_commits_stay_where_they_are() {
        let rows = [summary(3), row(2, "b", "a"), summary(1), row(4, "a", "m")];
        let groups = group_by_chain(&rows, &HashMap::new());
        assert_eq!(shown(&rows, &groups), [3, 2, 4, 1]);
        assert_eq!(groups[0].1, None);
        assert_eq!(groups[3].1, None);
    }

    #[test]
    fn an_outdated_parent_links_through_the_related_changes() {
        use ChainPlace::*;
        // 2 has moved on to "b2"; 3 is still on its first patch set "b1".
        let rows = [row(3, "c", "b1"), row(2, "b2", "a"), row(1, "a", "m")];
        let alone = group_by_chain(&rows, &HashMap::new());
        assert_eq!(alone[0].1, None);
        let outdated = HashMap::from([(ChangeNumber(3), ChangeNumber(2))]);
        let groups = group_by_chain(&rows, &outdated);
        assert_eq!(shown(&rows, &groups), [3, 2, 1]);
        assert_eq!(groups[0].1, Some(Tip));
        assert_eq!(groups[2].1, Some(Base));
    }

    #[test]
    fn a_fork_keeps_both_branches_in_the_block() {
        // 2 and 3 are both built on 1.
        let rows = [row(3, "c", "a"), row(2, "b", "a"), row(1, "a", "m")];
        let groups = group_by_chain(&rows, &HashMap::new());
        assert_eq!(shown(&rows, &groups), [2, 3, 1]);
    }

    #[test]
    fn inconsistent_links_do_not_loop() {
        let rows = [row(1, "a", "b"), row(2, "b", "a")];
        let groups = group_by_chain(&rows, &HashMap::new());
        let mut numbers = shown(&rows, &groups);
        numbers.sort_unstable();
        assert_eq!(numbers, [1, 2]);
    }
}
