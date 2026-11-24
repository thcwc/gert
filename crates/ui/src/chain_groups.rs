//! Grouping a dashboard section's rows by relation chain.
//!
//! A row is built on another when its current patch set's first parent is
//! that row's current patch set ([`ChangeSummary::current_commit`]), or
//! when the cached related changes put that row next below it in the
//! chain ([`ChainLink`], `Syncer::cached_chain_links`): the chain is built
//! on an outdated patch set, or the changes between the two are not rows.
//! A long chain is rarely all in the section (the query lists the most
//! recently updated changes first), so those gaps are common; they are
//! shown as hidden changes, not as separate chains.
//!
//! Each chain is shown as one block, newest descendant first like Gerrit's
//! related changes, where its first row was in the server's order. Rows
//! outside any chain keep their place.

use std::collections::HashMap;

use model::{ChangeNumber, ChangeSummary, RevisionId};
use sync::ChainLink;

/// Where a row sits in a chain of two or more rows, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainPlace {
    /// The top: nothing in the section is built on it.
    Tip,
    Middle,
    /// The bottom: built on nothing in the section.
    Base,
}

/// One row as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// Index into the rows.
    pub index: usize,
    /// `None` for a row alone.
    pub place: Option<ChainPlace>,
    /// Changes of the chain between this row and the one shown below it
    /// that are not rows.
    pub hidden_below: usize,
}

/// The rows in the order to show them, each with its place in its chain.
pub fn group_by_chain(
    rows: &[ChangeSummary],
    links: &HashMap<ChangeNumber, ChainLink>,
) -> Vec<Shown> {
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
    // Each row's parent among the rows, and the changes skipped to get
    // there.
    let (parent, skipped): (Vec<Option<usize>>, Vec<usize>) = rows
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let first = c.current_commit.as_ref().and_then(|k| k.parents.first());
            let direct = first.and_then(|p| by_sha.get(p).copied()).map(|p| (p, 0));
            let linked = || {
                let l = links.get(&c.number)?;
                Some((*by_number.get(&l.parent)?, l.skipped))
            };
            match direct.or_else(linked).filter(|&(p, _)| p != i) {
                Some((p, n)) => (Some(p), n),
                None => (None, 0),
            }
        })
        .unzip();
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
        for (k, &i) in chain.iter().enumerate() {
            let place = match k {
                _ if last == 0 => None,
                0 => Some(ChainPlace::Tip),
                k if k == last => Some(ChainPlace::Base),
                _ => Some(ChainPlace::Middle),
            };
            // Only a parent drawn right below; in a fork it may not be.
            let hidden_below = match (parent[i], chain.get(k + 1)) {
                (Some(p), Some(&below)) if p == below => skipped[i],
                _ => 0,
            };
            out.push(Shown {
                index: i,
                place,
                hidden_below,
            });
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

    fn shown(rows: &[ChangeSummary], groups: &[Shown]) -> Vec<u64> {
        groups.iter().map(|s| rows[s.index].number.0).collect()
    }

    fn link(parent: u64, skipped: usize) -> ChainLink {
        ChainLink {
            parent: ChangeNumber(parent),
            skipped,
        }
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
        let places: Vec<_> = groups.iter().map(|s| s.place).collect();
        assert_eq!(places, [None, Some(Tip), Some(Middle), Some(Base), None]);
    }

    #[test]
    fn rows_without_commits_stay_where_they_are() {
        let rows = [summary(3), row(2, "b", "a"), summary(1), row(4, "a", "m")];
        let groups = group_by_chain(&rows, &HashMap::new());
        assert_eq!(shown(&rows, &groups), [3, 2, 4, 1]);
        assert_eq!(groups[0].place, None);
        assert_eq!(groups[3].place, None);
    }

    #[test]
    fn an_outdated_parent_links_through_the_related_changes() {
        use ChainPlace::*;
        // 2 has moved on to "b2"; 3 is still on its first patch set "b1".
        let rows = [row(3, "c", "b1"), row(2, "b2", "a"), row(1, "a", "m")];
        let alone = group_by_chain(&rows, &HashMap::new());
        assert_eq!(alone[0].place, None);
        let links = HashMap::from([(ChangeNumber(3), link(2, 0))]);
        let groups = group_by_chain(&rows, &links);
        assert_eq!(shown(&rows, &groups), [3, 2, 1]);
        assert_eq!(groups[0].place, Some(Tip));
        assert_eq!(groups[2].place, Some(Base));
        assert!(groups.iter().all(|s| s.hidden_below == 0));
    }

    #[test]
    fn a_long_chain_listed_in_pieces_is_one_block_with_its_gaps() {
        use ChainPlace::*;
        // The chain 1 <- 2 <- ... <- 30, of which the query lists 30, 12,
        // 11 and 3 (most recently updated first, so out of chain order),
        // with 8 between them.
        let rows = [
            row(12, "s12", "s11"),
            row(30, "s30", "s29"),
            row(8, "h", "main"),
            row(3, "s3", "s2"),
            row(11, "s11", "s10"),
        ];
        let links = HashMap::from([
            (ChangeNumber(30), link(12, 17)),
            (ChangeNumber(11), link(3, 7)),
        ]);
        let groups = group_by_chain(&rows, &links);
        assert_eq!(shown(&rows, &groups), [30, 12, 11, 3, 8]);
        let places: Vec<_> = groups.iter().map(|s| s.place).collect();
        assert_eq!(
            places,
            [Some(Tip), Some(Middle), Some(Middle), Some(Base), None]
        );
        let hidden: Vec<_> = groups.iter().map(|s| s.hidden_below).collect();
        assert_eq!(hidden, [17, 0, 7, 0, 0]);
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
