//! Turning a shared document into a workspace: match panes to core terminals,
//! prune panes whose terminal is gone, and recover labeled terminals that the
//! document lost. Pure: the caller supplies terminal addresses and labels.

use super::super::{
    Axis, Child, IdAllocator, Layout, Pane, PaneId, Space, SpaceId, Tab, TabId, Workspace, focus,
    remove_layout,
};
use super::schema::{
    CLIENT_LABEL, CLIENT_NAME, DocLayout, DocPane, PANE_LABEL, SPACE_LABEL, SharedDoc, TAB_LABEL,
    identity_labels,
};
use std::collections::{BTreeMap, BTreeSet};

/// What matching needs to know about one core terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalInfo {
    pub address: String,
    pub labels: BTreeMap<String, String>,
}

impl TerminalInfo {
    pub fn new(address: impl Into<String>, labels: BTreeMap<String, String>) -> TerminalInfo {
        TerminalInfo {
            address: address.into(),
            labels,
        }
    }

    fn is_ours(&self) -> bool {
        self.labels.get(CLIENT_LABEL).map(String::as_str) == Some(CLIENT_NAME)
    }

    fn pane_label(&self) -> Option<&str> {
        self.labels.get(PANE_LABEL).map(String::as_str)
    }
}

/// The result of restoring a document at startup.
pub struct Restored {
    pub workspace: Workspace,
    /// Panes matched to live or finished terminals, in document order.
    pub matched: Vec<(PaneId, String)>,
    /// Panes pruned because no terminal matched.
    pub missing: usize,
    /// The document needs writing (a rediscovered address/label or a prune).
    pub repaired: bool,
    /// Every pane id the document mentions, including pruned ones. A recovered
    /// orphan may never resurrect a pane this screen already rejected.
    pub seen: BTreeSet<PaneId>,
}

/// Builds a workspace from `doc`.
///
/// `ids` carries the screen namespace and every id already used; restored ids
/// keep their values and are marked used. `names` is the caller's current
/// generated-name high-water mark, so a rebuild does not reuse "space 2".
pub fn restore(
    doc: &SharedDoc,
    ids: IdAllocator,
    names: (u32, u32),
    terminals: &[TerminalInfo],
) -> Restored {
    let mut ids = ids;
    let mut seen = BTreeSet::new();
    for space in &doc.spaces {
        ids.mark(space.id);
        for tab in &space.tabs {
            ids.mark(tab.id);
            for pane in tab.leaves() {
                ids.mark(pane.id);
                seen.insert(PaneId(pane.id));
            }
        }
    }
    let next_space = names.0.max(next_name(
        doc.spaces.iter().map(|space| space.name.as_str()),
        "space ",
    ));
    let next_tab = names.1.max(next_name(
        doc.spaces
            .iter()
            .flat_map(|space| space.tabs.iter().map(|tab| tab.name.as_str())),
        "tab ",
    ));

    let by_address: BTreeMap<&str, &TerminalInfo> = terminals
        .iter()
        .filter(|terminal| terminal.is_ours())
        .map(|terminal| (terminal.address.as_str(), terminal))
        .collect();
    let mut by_pane_label: BTreeMap<&str, Vec<&TerminalInfo>> = BTreeMap::new();
    for terminal in terminals.iter().filter(|terminal| terminal.is_ours()) {
        if let Some(label) = terminal.pane_label() {
            by_pane_label.entry(label).or_default().push(terminal);
        }
    }

    let mut panes = BTreeMap::new();
    let mut matched = Vec::new();
    let mut missing = 0usize;
    let mut repaired = false;
    for space in &doc.spaces {
        for tab in &space.tabs {
            for pane in tab.leaves() {
                let Some(info) = match_terminal(pane, &by_address, &by_pane_label) else {
                    missing += 1;
                    continue;
                };
                let mut record = Pane {
                    id: PaneId(pane.id),
                    address: info.address.clone(),
                    name: pane.name.clone(),
                    labels: info.labels.clone(),
                    title: String::new(),
                    status: None,
                    // Workspace-owned for close semantics: `x` ends the
                    // program, `e` detaches and `X` kills. The `Session`
                    // around it is still adopted, so local errors never end a
                    // program this process did not start.
                    owned: true,
                    keep: false,
                    exit: None,
                };
                if record.name == pane.address {
                    record.name = info.address.clone();
                }
                if record.address != pane.address
                    || record.name != pane.name
                    || identity_labels(&record.labels) != identity_labels(&pane.labels)
                {
                    repaired = true;
                }
                matched.push((record.id, record.address.clone()));
                panes.insert(record.id, record);
            }
        }
    }

    let mut spaces = Vec::with_capacity(doc.spaces.len());
    for doc_space in &doc.spaces {
        let mut tabs = Vec::with_capacity(doc_space.tabs.len());
        for doc_tab in &doc_space.tabs {
            let mut layout = doc_tab.layout.as_ref().map(build_layout);
            if let Some(root) = layout.take() {
                let mut root = Some(root);
                for pane in doc_tab.leaves() {
                    if !panes.contains_key(&PaneId(pane.id)) {
                        root = root.and_then(|layout| remove_layout(layout, PaneId(pane.id)));
                    }
                }
                layout = root;
            }
            if layout.is_some() != doc_tab.layout.is_some() {
                repaired = true;
            }
            let mut tab = Tab::new(TabId(doc_tab.id), doc_tab.name.clone());
            tab.layout = layout;
            if let Some(first) = tab.layout.as_ref().and_then(Layout::first_leaf) {
                focus(&mut tab, first);
            }
            tabs.push(tab);
        }
        let active = tabs.first().map(|tab| tab.id).unwrap_or(TabId(0));
        spaces.push(Space {
            id: SpaceId(doc_space.id),
            name: doc_space.name.clone(),
            folder: doc_space.folder.clone(),
            tabs,
            active,
        });
    }

    let workspace = if spaces.is_empty() {
        // Only a merge of two conflicting deletions can get here; keep the
        // workspace invariant with a fresh empty space/tab in this screen's
        // namespace.
        repaired = true;
        let space_id = SpaceId(ids.alloc());
        let tab_id = TabId(ids.alloc());
        let fallback = Space {
            id: space_id,
            name: format!("space {next_space}"),
            folder: String::new(),
            tabs: vec![Tab::new(tab_id, format!("tab {next_tab}"))],
            active: tab_id,
        };
        Workspace {
            spaces: vec![fallback],
            active: space_id,
            panes,
            ids,
            next_space_name: next_space.saturating_add(1),
            next_tab_name: next_tab.saturating_add(1),
        }
    } else {
        let active = spaces[0].id;
        Workspace {
            spaces,
            active,
            panes,
            ids,
            next_space_name: next_space,
            next_tab_name: next_tab,
        }
    };
    if missing > 0 {
        repaired = true;
    }
    Restored {
        workspace,
        matched,
        missing,
        repaired,
        seen,
    }
}

fn match_terminal<'a>(
    pane: &DocPane,
    by_address: &BTreeMap<&str, &'a TerminalInfo>,
    by_pane_label: &BTreeMap<&str, Vec<&'a TerminalInfo>>,
) -> Option<&'a TerminalInfo> {
    let key = pane.id.to_string();
    if let Some(info) = by_address.get(pane.address.as_str())
        && info.pane_label() == Some(key.as_str())
    {
        return Some(info);
    }
    // A terminal rediscovered under a new address is found by the unique pane
    // label; an ambiguous label is not a match.
    let candidates = by_pane_label.get(key.as_str())?;
    (candidates.len() == 1).then_some(candidates[0])
}

fn build_layout(layout: &DocLayout) -> Layout {
    match layout {
        DocLayout::Leaf(pane) => Layout::Leaf(PaneId(pane.id)),
        DocLayout::Split { axis, children } => Layout::Split {
            axis: *axis,
            children: children
                .iter()
                .map(|child| Child {
                    size: child.size,
                    layout: build_layout(&child.layout),
                })
                .collect(),
        },
    }
}

/// Appends labeled terminals that exist in core but not in the workspace.
/// `excluded` holds pane ids this client has seen or deliberately rejected;
/// they are never resurrected. Invalid or inconsistent labels are ignored.
pub fn recover_orphans(
    workspace: &mut Workspace,
    terminals: &[TerminalInfo],
    excluded: &BTreeSet<PaneId>,
) -> Vec<(PaneId, String)> {
    let mut recovered = Vec::new();
    for terminal in terminals.iter().filter(|terminal| terminal.is_ours()) {
        let Some(space_id) = label_id(terminal, SPACE_LABEL) else {
            continue;
        };
        let Some(tab_id) = label_id(terminal, TAB_LABEL) else {
            continue;
        };
        let Some(pane_id) = label_id(terminal, PANE_LABEL) else {
            continue;
        };
        let pane = PaneId(pane_id);
        if excluded.contains(&pane)
            || workspace.panes.contains_key(&pane)
            || workspace.ids.is_used(pane_id)
        {
            continue;
        }

        let (space_index, created_space) = match workspace
            .spaces
            .iter()
            .position(|space| space.id.0 == space_id)
        {
            Some(index) => (index, false),
            None => {
                if workspace.ids.is_used(space_id) {
                    continue;
                }
                let number = workspace.next_space_name;
                workspace.next_space_name += 1;
                workspace.spaces.push(Space {
                    id: SpaceId(space_id),
                    name: format!("recovered space {number}"),
                    folder: String::new(),
                    tabs: Vec::new(),
                    active: TabId(0),
                });
                (workspace.spaces.len() - 1, true)
            }
        };
        let tab = TabId(tab_id);
        let tab_index = match workspace.spaces[space_index]
            .tabs
            .iter()
            .position(|existing| existing.id == tab)
        {
            Some(index) => index,
            None => {
                if workspace.ids.is_used(tab_id) {
                    continue;
                }
                let number = workspace.next_tab_name;
                workspace.next_tab_name += 1;
                workspace.spaces[space_index]
                    .tabs
                    .push(Tab::new(tab, format!("recovered tab {number}")));
                workspace.spaces[space_index].tabs.len() - 1
            }
        };
        if created_space {
            workspace.spaces[space_index].active = tab;
        }
        let record = Pane {
            id: pane,
            address: terminal.address.clone(),
            name: terminal.address.clone(),
            labels: terminal.labels.clone(),
            title: String::new(),
            status: None,
            // Recovered panes are workspace-owned for close semantics too.
            owned: true,
            keep: false,
            exit: None,
        };
        workspace.panes.insert(pane, record);
        let target = &mut workspace.spaces[space_index].tabs[tab_index];
        let leaf = Layout::Leaf(pane);
        target.layout = Some(match target.layout.take() {
            None => leaf,
            Some(root) => Layout::Split {
                axis: Axis::Columns,
                children: vec![Child::new(root), Child::new(leaf)],
            },
        });
        if target.focused.is_none() {
            focus(target, pane);
        }
        workspace.ids.mark(space_id);
        workspace.ids.mark(tab_id);
        workspace.ids.mark(pane_id);
        recovered.push((pane, terminal.address.clone()));
    }
    recovered
}

fn label_id(terminal: &TerminalInfo, label: &str) -> Option<u64> {
    terminal.labels.get(label)?.parse().ok()
}

fn next_name<'a>(names: impl Iterator<Item = &'a str>, prefix: &str) -> u32 {
    let highest = names
        .filter_map(|name| name.strip_prefix(prefix))
        .filter_map(|number| number.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    highest.saturating_add(1)
}

/// The document's pane addresses, for a runtime refetch that wants to ask core
/// about terminals the event model has not seen yet.
pub fn pane_addresses(doc: &SharedDoc) -> Vec<String> {
    let mut addresses = Vec::new();
    for space in &doc.spaces {
        for tab in &space.tabs {
            for pane in tab.leaves() {
                addresses.push(pane.address.clone());
            }
        }
    }
    addresses
}

#[cfg(test)]
mod tests {
    use super::super::schema::{DocSpace, DocTab, PINNED_LABEL, SEEN_LABEL};
    use super::*;

    fn labels(pane: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            (CLIENT_LABEL.to_owned(), CLIENT_NAME.to_owned()),
            (SPACE_LABEL.to_owned(), "1".to_owned()),
            (TAB_LABEL.to_owned(), "2".to_owned()),
            (PANE_LABEL.to_owned(), pane.to_string()),
        ])
    }

    fn doc(address: &str) -> SharedDoc {
        SharedDoc {
            spaces: vec![DocSpace {
                id: 1,
                name: "space".into(),
                folder: String::new(),
                tabs: vec![DocTab {
                    id: 2,
                    name: "tab".into(),
                    layout: Some(DocLayout::Leaf(DocPane {
                        id: 3,
                        address: address.into(),
                        name: address.into(),
                        labels: labels(3),
                    })),
                }],
            }],
        }
    }

    fn info(address: &str, pane: u64) -> TerminalInfo {
        TerminalInfo::new(address, labels(pane))
    }

    #[test]
    fn a_matching_address_and_label_is_restored_and_owned() {
        let restored = restore(
            &doc("t1"),
            IdAllocator::new("default"),
            (1, 1),
            &[info("t1", 3)],
        );
        assert_eq!(restored.missing, 0);
        assert!(!restored.repaired);
        assert_eq!(restored.matched, vec![(PaneId(3), "t1".to_owned())]);
        let pane = restored.workspace.pane(PaneId(3)).expect("pane");
        assert!(
            pane.owned,
            "a restored pane is workspace-owned for close semantics"
        );
        assert_eq!(pane.labels.get(PANE_LABEL), Some(&"3".to_owned()));
        assert_eq!(restored.workspace.focused_pane(), Some(PaneId(3)));
    }

    #[test]
    fn live_attention_labels_never_repair_or_pollute_the_document() {
        let mut live = labels(3);
        live.insert(SEEN_LABEL.into(), "9".into());
        live.insert(PINNED_LABEL.into(), "1".into());
        let restored = restore(
            &doc("t1"),
            IdAllocator::new("default"),
            (1, 1),
            &[TerminalInfo::new("t1", live)],
        );
        assert!(
            !restored.repaired,
            "a seen or pin change is live state, not a document repair"
        );
        assert_eq!(restored.missing, 0);
        let pane = restored.workspace.pane(PaneId(3)).expect("pane");
        assert_eq!(pane.labels.get(SEEN_LABEL), Some(&"9".to_owned()));
        assert_eq!(pane.labels.get(PINNED_LABEL), Some(&"1".to_owned()));
        let captured = restored.workspace.capture_doc().to_value().to_string();
        assert!(!captured.contains(SEEN_LABEL) && !captured.contains(PINNED_LABEL));
    }

    #[test]
    fn a_rediscovered_address_is_found_by_its_unique_pane_label() {
        let restored = restore(
            &doc("t1"),
            IdAllocator::new("default"),
            (1, 1),
            &[info("t9", 3)],
        );
        assert!(restored.repaired, "the new address must be written back");
        assert_eq!(restored.matched, vec![(PaneId(3), "t9".to_owned())]);
        assert_eq!(restored.workspace.pane(PaneId(3)).unwrap().address, "t9");
    }

    #[test]
    fn a_pane_whose_terminal_is_gone_is_pruned_but_its_tab_stays() {
        let mut workspace_doc = doc("gone");
        workspace_doc.spaces[0].tabs.push(DocTab {
            id: 4,
            name: "empty".into(),
            layout: None,
        });
        let restored = restore(&workspace_doc, IdAllocator::new("default"), (1, 1), &[]);
        assert_eq!(restored.missing, 1);
        assert!(restored.repaired);
        assert!(restored.workspace.pane(PaneId(3)).is_none());
        assert!(
            restored
                .workspace
                .tab(SpaceId(1), TabId(2))
                .unwrap()
                .layout
                .is_none()
        );
        assert_eq!(restored.workspace.spaces()[0].tabs.len(), 2);
        assert!(restored.seen.contains(&PaneId(3)));
    }

    #[test]
    fn a_terminal_without_our_client_label_is_never_matched() {
        let mut foreign = info("t1", 3);
        foreign.labels.remove(CLIENT_LABEL);
        let restored = restore(&doc("t1"), IdAllocator::new("default"), (1, 1), &[foreign]);
        assert_eq!(restored.missing, 1);
        assert!(restored.matched.is_empty());
    }

    #[test]
    fn an_ambiguous_pane_label_is_not_a_match() {
        let restored = restore(
            &doc("old"),
            IdAllocator::new("default"),
            (1, 1),
            &[info("t1", 3), info("t2", 3)],
        );
        assert_eq!(restored.missing, 1);
        assert!(restored.matched.is_empty());
    }

    #[test]
    fn a_labeled_orphan_is_recovered_into_its_own_space_and_tab() {
        let mut workspace = Workspace::new();
        let recovered = recover_orphans(&mut workspace, &[info("t7", 12)], &BTreeSet::new());
        assert_eq!(recovered, vec![(PaneId(12), "t7".to_owned())]);
        let location = workspace.location(PaneId(12)).expect("recovered");
        assert_eq!(location, (SpaceId(1), TabId(2)));
        assert_eq!(
            workspace.space(SpaceId(1)).unwrap().name,
            "recovered space 2"
        );
        assert_eq!(workspace.pane(PaneId(12)).unwrap().address, "t7");
    }

    #[test]
    fn an_excluded_or_already_used_pane_is_never_resurrected() {
        let mut workspace = Workspace::new();
        let excluded = BTreeSet::from([PaneId(12)]);
        assert!(recover_orphans(&mut workspace, &[info("t7", 12)], &excluded).is_empty());

        let mut document = doc("gone");
        if let Some(DocLayout::Leaf(pane)) = document.spaces[0].tabs[0].layout.as_mut() {
            pane.id = 12;
        }
        let mut restored = restore(&document, IdAllocator::new("default"), (1, 1), &[]).workspace;
        assert!(restored.ids.is_used(12));
        assert!(
            recover_orphans(&mut restored, &[info("t7", 12)], &BTreeSet::new()).is_empty(),
            "an id the document already used must not be re-added"
        );
    }
}
