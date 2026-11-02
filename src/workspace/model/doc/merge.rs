//! Three-way merge of the shared document.
//!
//! The merge is per space and, inside a space, per tab. A tab changed by only
//! one side takes that side; a deletion against an unchanged object applies;
//! additions on both sides are kept. When both sides changed the exact same
//! tab, the local tab wins and the conflict is reported: the other screen's
//! terminal is then recovered as an orphan, not silently lost.
//!
//! Output order is deterministic: remote/existing order first, local new
//! spaces and tabs appended in local order.

use super::super::{SpaceId, TabId};
use super::schema::{DocSpace, DocTab, SharedDoc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub space: SpaceId,
    /// The tab in conflict; `None` for a space-level conflict.
    pub tab: Option<TabId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeResult {
    pub doc: SharedDoc,
    pub conflicts: Vec<Conflict>,
}

pub fn merge(base: &SharedDoc, local: &SharedDoc, remote: &SharedDoc) -> MergeResult {
    let mut order: Vec<u64> = Vec::new();
    for space in &remote.spaces {
        if !order.contains(&space.id) {
            order.push(space.id);
        }
    }
    for space in &local.spaces {
        if !order.contains(&space.id) {
            order.push(space.id);
        }
    }

    let mut conflicts = Vec::new();
    let mut spaces = Vec::new();
    for id in order {
        let base_space = space(base, id);
        let local_space = space(local, id);
        let remote_space = space(remote, id);
        if let Some(merged) = merge_space(base_space, local_space, remote_space, &mut conflicts) {
            spaces.push(merged);
        }
    }
    MergeResult {
        doc: SharedDoc { spaces },
        conflicts,
    }
}

fn merge_space(
    base: Option<&DocSpace>,
    local: Option<&DocSpace>,
    remote: Option<&DocSpace>,
    conflicts: &mut Vec<Conflict>,
) -> Option<DocSpace> {
    match (local, remote) {
        (None, None) => None,
        (Some(local), None) => {
            if base.is_none() {
                // A space this screen added; the other screen never saw it.
                return Some(local.clone());
            }
            if base == Some(local) {
                // The other screen deleted an unchanged space: the deletion
                // applies.
                return None;
            }
            // The local screen changed it; the other deleted it. Keep local
            // and report it, so its terminals can be recovered if needed.
            conflicts.push(Conflict {
                space: SpaceId(local.id),
                tab: None,
            });
            Some(local.clone())
        }
        (None, Some(remote)) => {
            if base.is_none() {
                // A space the other screen added.
                return Some(remote.clone());
            }
            if base == Some(remote) {
                // This screen deleted an unchanged space: the deletion
                // applies.
                return None;
            }
            // Local deleted, remote changed: the local deletion stands.
            conflicts.push(Conflict {
                space: SpaceId(remote.id),
                tab: None,
            });
            None
        }
        (Some(local), Some(remote)) => {
            if base == Some(local) {
                return Some(remote.clone());
            }
            if base == Some(remote) {
                return Some(local.clone());
            }
            Some(merge_changed_space(base, local, remote, conflicts))
        }
    }
}

fn merge_changed_space(
    base: Option<&DocSpace>,
    local: &DocSpace,
    remote: &DocSpace,
    conflicts: &mut Vec<Conflict>,
) -> DocSpace {
    let space = SpaceId(local.id);
    let name = pick(
        base.map(|space| space.name.as_str()),
        &local.name,
        &remote.name,
    );
    let folder = pick(
        base.map(|space| space.folder.as_str()),
        &local.folder,
        &remote.folder,
    );

    let mut order: Vec<u64> = Vec::new();
    for tab in &remote.tabs {
        if !order.contains(&tab.id) {
            order.push(tab.id);
        }
    }
    for tab in &local.tabs {
        if !order.contains(&tab.id) {
            order.push(tab.id);
        }
    }
    let mut tabs = Vec::new();
    for id in order {
        let base_tab = base.and_then(|space| space.tab(id));
        let local_tab = local.tab(id);
        let remote_tab = remote.tab(id);
        if let Some(tab) = merge_tab(space, base_tab, local_tab, remote_tab, conflicts) {
            tabs.push(tab);
        }
    }
    if tabs.is_empty() {
        // Every local tab was deleted by remote and every remote tab was
        // deleted locally, or the remote side became empty through conflicts.
        // The local screen still has this space; keep its tabs rather than
        // producing an invalid empty space.
        tabs = local.tabs.clone();
        conflicts.push(Conflict {
            space,
            tab: local.tabs.first().map(|tab| TabId(tab.id)),
        });
    }
    DocSpace {
        id: local.id,
        name,
        folder,
        tabs,
    }
}

fn merge_tab(
    space: SpaceId,
    base: Option<&DocTab>,
    local: Option<&DocTab>,
    remote: Option<&DocTab>,
    conflicts: &mut Vec<Conflict>,
) -> Option<DocTab> {
    match (local, remote) {
        (None, None) => None,
        (Some(local), None) => {
            if base.is_none() {
                return Some(local.clone());
            }
            if base == Some(local) {
                // The other screen deleted an unchanged tab.
                return None;
            }
            conflicts.push(Conflict {
                space,
                tab: Some(TabId(local.id)),
            });
            Some(local.clone())
        }
        (None, Some(remote)) => {
            if base.is_none() {
                return Some(remote.clone());
            }
            if base == Some(remote) {
                // This screen deleted an unchanged tab.
                return None;
            }
            conflicts.push(Conflict {
                space,
                tab: Some(TabId(remote.id)),
            });
            None
        }
        (Some(local), Some(remote)) => {
            if base == Some(local) {
                return Some(remote.clone());
            }
            if base == Some(remote) {
                return Some(local.clone());
            }
            if local == remote {
                return Some(local.clone());
            }
            conflicts.push(Conflict {
                space,
                tab: Some(TabId(local.id)),
            });
            Some(local.clone())
        }
    }
}

fn pick(base: Option<&str>, local: &str, remote: &str) -> String {
    if base == Some(local) {
        remote.to_owned()
    } else if base == Some(remote) {
        local.to_owned()
    } else {
        // Both changed the field: local is the deterministic winner.
        local.to_owned()
    }
}

fn space(doc: &SharedDoc, id: u64) -> Option<&DocSpace> {
    doc.spaces.iter().find(|space| space.id == id)
}

#[cfg(test)]
mod tests {
    use super::super::super::Axis;
    use super::super::schema::{DocChild, DocLayout, DocPane};
    use super::*;

    fn pane(id: u64, address: &str) -> DocLayout {
        DocLayout::Leaf(DocPane {
            id,
            address: address.into(),
            name: address.into(),
            labels: Default::default(),
        })
    }

    fn tab(id: u64, name: &str, layout: Option<DocLayout>) -> DocTab {
        DocTab {
            id,
            name: name.into(),
            layout,
        }
    }

    fn space(id: u64, name: &str, tabs: Vec<DocTab>) -> DocSpace {
        DocSpace {
            id,
            name: name.into(),
            folder: String::new(),
            tabs,
        }
    }

    fn doc(spaces: Vec<DocSpace>) -> SharedDoc {
        SharedDoc { spaces }
    }

    fn one_space(tabs: Vec<DocTab>) -> SharedDoc {
        doc(vec![space(1, "space", tabs)])
    }

    #[test]
    fn unrelated_spaces_from_both_screens_are_preserved() {
        let base = one_space(vec![tab(2, "shared", Some(pane(3, "t1")))]);
        let mut local = base.clone();
        local
            .spaces
            .push(space(10, "local", vec![tab(11, "l", Some(pane(12, "t2")))]));
        let mut remote = base.clone();
        remote.spaces.push(space(
            20,
            "remote",
            vec![tab(21, "r", Some(pane(22, "t3")))],
        ));
        let merged = merge(&base, &local, &remote);
        let ids: Vec<u64> = merged.doc.spaces.iter().map(|space| space.id).collect();
        assert_eq!(
            ids,
            vec![1, 20, 10],
            "remote order first, local additions after"
        );
        assert!(merged.conflicts.is_empty());
    }

    #[test]
    fn tabs_changed_on_different_sides_are_both_kept() {
        let base = one_space(vec![
            tab(2, "one", Some(pane(3, "t1"))),
            tab(4, "two", None),
        ]);
        let mut local = base.clone();
        local.spaces[0].tabs[0] = tab(2, "renamed", Some(pane(3, "t1")));
        let mut remote = base.clone();
        remote.spaces[0].tabs[1] = tab(4, "two", Some(pane(5, "t2")));
        let merged = merge(&base, &local, &remote);
        assert!(merged.conflicts.is_empty());
        let tabs = &merged.doc.spaces[0].tabs;
        assert_eq!(tabs[0].name, "renamed");
        assert_eq!(tabs[1].layout.as_ref().unwrap().leaves()[0].address, "t2");
    }

    #[test]
    fn the_same_tab_changed_on_both_screens_keeps_local_and_reports_it() {
        let base = one_space(vec![tab(2, "one", Some(pane(3, "t1")))]);
        let mut local = base.clone();
        local.spaces[0].tabs[0] = tab(2, "local", Some(pane(3, "t1")));
        let mut remote = base.clone();
        remote.spaces[0].tabs[0] = tab(2, "remote", Some(pane(5, "t2")));
        let merged = merge(&base, &local, &remote);
        assert_eq!(merged.doc.spaces[0].tabs[0].name, "local");
        assert_eq!(
            merged.doc.spaces[0].tabs[0]
                .layout
                .as_ref()
                .unwrap()
                .leaves()[0]
                .address,
            "t1"
        );
        assert_eq!(
            merged.conflicts,
            vec![Conflict {
                space: SpaceId(1),
                tab: Some(TabId(2))
            }]
        );
    }

    #[test]
    fn a_one_sided_change_takes_that_side() {
        let base = one_space(vec![tab(2, "one", Some(pane(3, "t1")))]);
        let local = base.clone();
        let mut remote = base.clone();
        remote.spaces[0].tabs[0].name = "remote".into();
        let merged = merge(&base, &local, &remote);
        assert_eq!(merged.doc.spaces[0].tabs[0].name, "remote");
        assert!(merged.conflicts.is_empty());
    }

    #[test]
    fn deleting_an_unchanged_tab_applies_the_deletion() {
        let base = one_space(vec![
            tab(2, "one", Some(pane(3, "t1"))),
            tab(4, "two", None),
        ]);
        let mut local = base.clone();
        local.spaces[0].tabs.remove(1);
        let merged = merge(&base, &local, &base);
        assert_eq!(merged.doc.spaces[0].tabs.len(), 1);
        assert!(merged.conflicts.is_empty());
    }

    #[test]
    fn a_split_layout_survives_the_merge() {
        let layout = DocLayout::Split {
            axis: Axis::Columns,
            children: vec![
                DocChild {
                    size: 2,
                    layout: pane(3, "t1"),
                },
                DocChild {
                    size: 1,
                    layout: pane(4, "t2"),
                },
            ],
        };
        let base = one_space(vec![tab(2, "one", Some(layout))]);
        let mut remote = base.clone();
        remote.spaces[0].tabs[0].name = "remote".into();
        let merged = merge(&base, &base, &remote);
        let layout = merged.doc.spaces[0].tabs[0].layout.as_ref().unwrap();
        assert!(matches!(layout, DocLayout::Split { .. }));
        assert_eq!(layout.leaves().len(), 2);
    }
}
