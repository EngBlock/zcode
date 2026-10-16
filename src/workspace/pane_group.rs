//! The editor area's split tree: panes side by side or stacked, splits in
//! splits. File ▸ New ▸ Editor Pane on Right / Below split the focused pane;
//! Close Editor Pane removes it and folds a split left with one pane back
//! into its parent.

use serde::{Deserialize, Serialize};

use super::pane::PaneId;

/// How a split lays out its members: `Horizontal` side by side (Editor Pane
/// on Right), `Vertical` stacked (Editor Pane Below).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    Pane(PaneId),
    /// Two or more members along `axis`; never directly inside a split of
    /// the same axis (that would be one split).
    Split { axis: SplitAxis, members: Vec<Member> },
}

impl Member {
    /// What a split is known by on screen and in the session: its panes, so
    /// a split that changes starts from equal sizes, not another split's.
    pub fn split_key(members: &[Member]) -> String {
        let panes: Vec<String> = members
            .iter()
            .filter_map(|m| match m {
                Member::Pane(id) => Some(id.0.to_string()),
                Member::Split { .. } => None,
            })
            .collect();
        format!("panes-{}-{}", members.len(), panes.join("-"))
    }

    /// The keys of every split under (and including) this member.
    pub fn split_keys(&self, out: &mut Vec<String>) {
        if let Member::Split { members, .. } = self {
            out.push(Self::split_key(members));
            members.iter().for_each(|m| m.split_keys(out));
        }
    }

    fn panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Member::Pane(id) => out.push(*id),
            Member::Split { members, .. } => members.iter().for_each(|m| m.panes(out)),
        }
    }

    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Member::Pane(id) => *id == pane,
            Member::Split { members, .. } => members.iter().any(|m| m.contains(pane)),
        }
    }

    /// Splits `at`: `new` goes after it along `axis`, as a sibling when the
    /// split around `at` already runs along `axis`.
    fn split(&mut self, at: PaneId, new: PaneId, axis: SplitAxis) -> bool {
        match self {
            Member::Pane(id) if *id == at => {
                *self = Member::Split { axis, members: vec![Member::Pane(at), Member::Pane(new)] };
                true
            }
            Member::Pane(_) => false,
            Member::Split { axis: own, members } => {
                if *own == axis
                    && let Some(ix) = members.iter().position(|m| *m == Member::Pane(at))
                {
                    members.insert(ix + 1, Member::Pane(new));
                    return true;
                }
                members.iter_mut().any(|m| m.split(at, new, axis))
            }
        }
    }

    /// Removes `pane` from the splits under this member.
    fn remove(&mut self, pane: PaneId) -> bool {
        let Member::Split { axis, members } = self else { return false };
        if let Some(ix) = members.iter().position(|m| *m == Member::Pane(pane)) {
            members.remove(ix);
        } else if !members.iter_mut().any(|m| m.remove(pane)) {
            return false;
        }
        // A member left alone takes the split's place; a split of the same
        // axis merges into this one.
        let own = *axis;
        let flattened: Vec<Member> = std::mem::take(members)
            .into_iter()
            .flat_map(|m| match m {
                Member::Split { axis, members } if axis == own => members,
                m => vec![m],
            })
            .collect();
        *self = match flattened.len() {
            1 => flattened.into_iter().next().unwrap(),
            _ => Member::Split { axis: own, members: flattened },
        };
        true
    }
}

/// The panes of the editor area and how they are split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneGroup {
    pub root: Member,
}

impl PaneGroup {
    pub fn new(pane: PaneId) -> Self {
        Self { root: Member::Pane(pane) }
    }

    /// Every pane, left to right and top to bottom: the order of Move Focus
    /// to Next Editor Pane.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = vec![];
        self.root.panes(&mut out);
        out
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.contains(pane)
    }

    /// Puts `new` after `at` (to its right, or below it).
    pub fn split(&mut self, at: PaneId, new: PaneId, axis: SplitAxis) -> bool {
        !self.contains(new) && self.root.split(at, new, axis)
    }

    /// Removes a pane; the last one stays.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        self.root.remove(pane)
    }

    /// The pane after (or before) `pane`, wrapping around.
    pub fn next(&self, pane: PaneId, forward: bool) -> PaneId {
        let panes = self.panes();
        let Some(ix) = panes.iter().position(|p| *p == pane) else { return pane };
        let len = panes.len();
        panes[if forward { (ix + 1) % len } else { (ix + len - 1) % len }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: u64) -> Member {
        Member::Pane(PaneId(id))
    }

    #[test]
    fn splitting_in_the_same_direction_adds_a_sibling() {
        let mut group = PaneGroup::new(PaneId(1));
        assert!(group.split(PaneId(1), PaneId(2), SplitAxis::Horizontal));
        assert!(group.split(PaneId(1), PaneId(3), SplitAxis::Horizontal));
        assert_eq!(group.root, Member::Split { axis: SplitAxis::Horizontal, members: vec![p(1), p(3), p(2)] });
        assert_eq!(group.panes(), [PaneId(1), PaneId(3), PaneId(2)]);
        assert!(!group.split(PaneId(9), PaneId(4), SplitAxis::Vertical), "no such pane");
        assert!(!group.split(PaneId(1), PaneId(2), SplitAxis::Vertical), "already there");
    }

    #[test]
    fn splitting_across_nests_a_split() {
        let mut group = PaneGroup::new(PaneId(1));
        group.split(PaneId(1), PaneId(2), SplitAxis::Horizontal);
        group.split(PaneId(2), PaneId(3), SplitAxis::Vertical);
        assert_eq!(
            group.root,
            Member::Split {
                axis: SplitAxis::Horizontal,
                members: vec![p(1), Member::Split { axis: SplitAxis::Vertical, members: vec![p(2), p(3)] }],
            }
        );
        assert_eq!(group.next(PaneId(3), true), PaneId(1));
        assert_eq!(group.next(PaneId(1), false), PaneId(3));
        assert_eq!(group.next(PaneId(1), true), PaneId(2));
    }

    #[test]
    fn removing_folds_single_members_and_merges_same_axis_splits() {
        let mut group = PaneGroup::new(PaneId(1));
        group.split(PaneId(1), PaneId(2), SplitAxis::Horizontal);
        group.split(PaneId(2), PaneId(3), SplitAxis::Vertical);
        group.split(PaneId(3), PaneId(4), SplitAxis::Horizontal);
        // 1 | (2 / (3 | 4)): removing 2 leaves 3 | 4 inside the outer row.
        assert!(group.remove(PaneId(2)));
        assert_eq!(group.root, Member::Split { axis: SplitAxis::Horizontal, members: vec![p(1), p(3), p(4)] });
        assert!(group.remove(PaneId(3)) && group.remove(PaneId(4)));
        assert_eq!(group.root, p(1));
        assert!(!group.remove(PaneId(1)), "the last pane stays");
        assert!(!group.remove(PaneId(7)));
    }
}
