//! The pure workspace: spaces, tabs, panes and their split layout.
//!
//! This module has no IO and no clock. It owns the stable ids, the layout
//! tree, focus and zoom, the rules for starting and closing panes, the shared
//! document (capture, restore, merge) and the per-screen local view. The screen
//! runtime turns the model's effects into `terminal.run`, `terminal.kill`,
//! attach streams and document writes.
//!
//! # Close, keep and kill
//!
//! Every pane has a terminal address and an `owned` flag (this client ran it)
//! and a `keep` flag (leave the program running when the pane closes):
//!
//! * `x` close: a kept or adopted pane is **detached** (the program keeps
//!   running); an owned, not kept pane is **ended** (`terminal.kill` then
//!   `terminal.rm`, so no untracked program is left behind).
//! * `e` keep: close the pane and detach, leaving the program running. The
//!   screen shows the address so the user knows what still runs.
//! * `X` kill: close and **end** the terminal even if it was kept.
//!
//! A pane whose program already ended keeps its final screen as a **finished
//! pane**: every close key only forgets its record (`terminal.rm`), because
//! there is no program left to detach or kill. The pane stays visible until
//! the person closes it explicitly; nothing silently discards the final
//! screen.
//!
//! The proposed M3 key table used `k`/`K` for keep/kill; those keys are the
//! conventional focus/move-up family, so [`super::keys`] maps keep/kill to
//! `e`/`X` and the help overlay lists them.
//!
//! Closing the last pane of a tab closes the tab; closing the last tab of a
//! space closes the space. The workspace always keeps one space with one
//! (possibly empty) tab, which gives the empty state a place to live.

use super::geometry;
use ratatui::layout::Rect;
use std::collections::BTreeMap;
use std::fmt;

mod doc;
mod ids;
mod view;

pub use doc::{
    CLIENT_LABEL, CLIENT_NAME, Conflict, DOC_NAME, DocChild, DocError, DocLayout, DocPane,
    DocSpace, DocTab, MergeResult, PANE_LABEL, PINNED_LABEL, SEEN_LABEL, SPACE_LABEL, SharedDoc,
    TAB_LABEL, TerminalInfo, identity_labels, is_attention_label, merge, pane_addresses, pane_ids,
    recover_orphans, restore,
};
pub use ids::IdAllocator;
pub use view::{LocalView, TabView};

/// A stable workspace id. Ids are never reused, so a persisted layout cannot
/// point at a different object after a restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub u64);

impl fmt::Display for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a split lays its children out. `Columns` puts them left to right (a
/// vertical divider); `Rows` puts them top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Columns,
    Rows,
}

/// A focus or move direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

/// One child of a split and its relative size (a weight).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub size: u32,
    pub layout: Layout,
}

impl Child {
    pub fn new(layout: Layout) -> Child {
        Child { size: 1, layout }
    }
}

/// The split tree. Leaves are panes; a split has at least two children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Leaf(PaneId),
    Split { axis: Axis, children: Vec<Child> },
}

impl Layout {
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Layout::Leaf(id) => *id == pane,
            Layout::Split { children, .. } => {
                children.iter().any(|child| child.layout.contains(pane))
            }
        }
    }

    pub fn first_leaf(&self) -> Option<PaneId> {
        match self {
            Layout::Leaf(id) => Some(*id),
            Layout::Split { children, .. } => {
                children.iter().find_map(|child| child.layout.first_leaf())
            }
        }
    }

    /// Every pane in tree order (tests and diagnostics).
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut leaves = Vec::new();
        self.collect(&mut leaves);
        leaves
    }

    fn collect(&self, leaves: &mut Vec<PaneId>) {
        match self {
            Layout::Leaf(id) => leaves.push(*id),
            Layout::Split { children, .. } => {
                for child in children {
                    child.layout.collect(leaves);
                }
            }
        }
    }

    /// Adds `new` next to the leaf `near`. In a split of the same axis the new
    /// leaf becomes a sibling; otherwise the leaf becomes a split of the other
    /// axis. False when `near` is not in this tree.
    fn split(&mut self, near: PaneId, axis: Axis, new: PaneId) -> bool {
        match self {
            Layout::Leaf(id) if *id == near => {
                let old = Layout::Leaf(*id);
                *self = Layout::Split {
                    axis,
                    children: vec![Child::new(old), Child::new(Layout::Leaf(new))],
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split {
                axis: split_axis,
                children,
            } => {
                if *split_axis == axis
                    && let Some(at) = children
                        .iter()
                        .position(|child| child.layout == Layout::Leaf(near))
                {
                    children.insert(at + 1, Child::new(Layout::Leaf(new)));
                    return true;
                }
                for child in children {
                    if child.layout.split(near, axis, new) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// A pane: the pure record of one terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub id: PaneId,
    /// The core terminal address (`t7`, or `machine/t7`).
    pub address: String,
    /// The display name shown when core has no title; the address at start.
    pub name: String,
    /// The labels the terminal carries in core, including the pane id label.
    pub labels: BTreeMap<String, String>,
    /// The terminal title reported by core (OSC 0/2).
    pub title: String,
    /// The OSC 7501 status word reported by core, if any.
    pub status: Option<String>,
    /// This client ran the terminal (as opposed to adopting one).
    pub owned: bool,
    /// Leave the program running when this pane closes.
    pub keep: bool,
    /// The program's exit status once it has finished, if it has. A finished
    /// pane keeps its final screen until the person closes it.
    pub exit: Option<i32>,
}

impl Pane {
    pub fn finished(&self) -> bool {
        self.exit.is_some()
    }
}

/// A tab's split layout and its local view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    /// `None` is an empty tab (the empty state).
    pub layout: Option<Layout>,
    pub focused: Option<PaneId>,
    pub zoomed: Option<PaneId>,
    /// Most recently focused pane last.
    pub history: Vec<PaneId>,
}

impl Tab {
    pub fn new(id: TabId, name: String) -> Tab {
        Tab {
            id,
            name,
            layout: None,
            focused: None,
            zoomed: None,
            history: Vec::new(),
        }
    }

    /// Every pane in this tab; empty for an empty tab.
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.as_ref().map(Layout::leaves).unwrap_or_default()
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.layout
            .as_ref()
            .is_some_and(|layout| layout.contains(pane))
    }
}

/// A space: a named group of tabs and the folder a future file UI would use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub folder: String,
    pub tabs: Vec<Tab>,
    pub active: TabId,
}

impl Space {
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }
}

/// A live core terminal being shown in this workspace for the first time. It
/// was not started by this screen, so it is adopted: the record's labels,
/// name, title, status and keep flag are preserved and the pane is not owned.
/// Nothing is run or respawned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptedTerminal {
    pub address: String,
    /// The core display name, else the address.
    pub name: String,
    /// The terminal's core labels, kept verbatim. No label is written here:
    /// another client's labels are never overwritten.
    pub labels: BTreeMap<String, String>,
    /// The terminal title reported by core (OSC 0/2).
    pub title: String,
    /// The OSC 7501 root word reported by core, if any.
    pub status: Option<String>,
    pub keep: bool,
}

/// A request to start a terminal. The runtime runs it, then calls
/// [`Workspace::insert_terminal`]; the reserved ids are used for the labels
/// sent with `terminal.run`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartRequest {
    pub space: SpaceId,
    pub tab: TabId,
    /// The id reserved for the new pane, sent in its label before `run`.
    pub pane: PaneId,
    pub placement: Placement,
    pub new_space: bool,
    pub new_tab: bool,
}

/// Where the new pane goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The tab is empty (or the caller asked for a tab of its own).
    Only,
    /// Split the tab next to the pane `near`.
    Split { axis: Axis, near: PaneId },
}

/// What kind of terminal to request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartKind {
    /// The first pane of an empty workspace.
    Initial,
    /// A new tab in the current space.
    Tab,
    /// A new space with one tab.
    Space,
    /// A split of the focused pane.
    Split(Axis),
}

impl StartRequest {
    /// Labels written on `terminal.run`. They are HerdR Terminal's own
    /// namespace, never the Herdr App's `herdr.space`/`herdr.tab`.
    pub fn labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("herdr.client".to_owned(), "herdr-terminal".to_owned()),
            ("herdr.terminal.space".to_owned(), self.space.0.to_string()),
            ("herdr.terminal.tab".to_owned(), self.tab.0.to_string()),
            ("herdr.terminal.pane".to_owned(), self.pane.0.to_string()),
            // Every terminal this client starts begins seen at turn 0, so a
            // fresh agent never shows an unread turn just for existing.
            ("herdr.seen".to_owned(), "0".to_owned()),
        ])
    }
}

/// What a close request does to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseKind {
    /// `x`: end an owned, not kept terminal; detach a kept or adopted one.
    Close,
    /// `e`: leave the program running.
    Keep,
    /// `X`: end the program and remove its record.
    Kill,
}

/// The literal action the runtime performs for a closing pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Close the attach stream; the program keeps running.
    Detach,
    /// Detach, `terminal.kill`, then `terminal.rm`.
    End,
    /// Forget a finished terminal (`terminal.rm`); its program already ended.
    Forget,
}

pub fn close_action(pane: &Pane, kind: CloseKind) -> CloseAction {
    if pane.finished() {
        // There is no program left to detach or kill: closing a finished pane
        // only forgets the record after its final screen has been shown.
        return CloseAction::Forget;
    }
    match kind {
        CloseKind::Keep => CloseAction::Detach,
        CloseKind::Kill => CloseAction::End,
        CloseKind::Close => {
            if pane.keep || !pane.owned {
                CloseAction::Detach
            } else {
                CloseAction::End
            }
        }
    }
}

/// A pane that a command removed from the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedPane {
    pub pane: PaneId,
    pub address: String,
    pub action: CloseAction,
}

/// The whole workspace.
#[derive(Clone, Debug)]
pub struct Workspace {
    spaces: Vec<Space>,
    active: SpaceId,
    panes: BTreeMap<PaneId, Pane>,
    ids: IdAllocator,
    next_space_name: u32,
    next_tab_name: u32,
}

impl Default for Workspace {
    fn default() -> Workspace {
        Workspace::new()
    }
}

impl Workspace {
    /// A workspace with one empty space and one empty tab, for the default
    /// screen.
    pub fn new() -> Workspace {
        Workspace::for_screen("default")
    }

    /// A workspace with one empty space and one empty tab whose ids belong to
    /// `screen`'s namespace, so two screens on one core cannot collide.
    pub fn for_screen(screen: &str) -> Workspace {
        let mut workspace = Workspace {
            spaces: Vec::new(),
            active: SpaceId(0),
            panes: BTreeMap::new(),
            ids: IdAllocator::new(screen),
            next_space_name: 1,
            next_tab_name: 1,
        };
        workspace.add_empty_space();
        workspace
    }

    /// The allocator, for a rebuild that must keep every id ever used.
    pub fn allocator(&self) -> &IdAllocator {
        &self.ids
    }

    /// The next generated space and tab numbers, so a rebuild can keep them
    /// monotonic.
    pub fn name_counters(&self) -> (u32, u32) {
        (self.next_space_name, self.next_tab_name)
    }

    // --- reading ----------------------------------------------------------

    pub fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    pub fn active_space_id(&self) -> SpaceId {
        self.active
    }

    pub fn active_space(&self) -> &Space {
        self.spaces
            .iter()
            .find(|space| space.id == self.active)
            .unwrap_or_else(|| self.spaces.first().expect("a workspace has a space"))
    }

    pub fn space(&self, id: SpaceId) -> Option<&Space> {
        self.spaces.iter().find(|space| space.id == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        let space = self.active_space();
        space
            .tabs
            .iter()
            .find(|tab| tab.id == space.active)
            .or_else(|| space.tabs.first())
    }

    pub fn active_tab_id(&self) -> Option<TabId> {
        self.active_tab().map(|tab| tab.id)
    }

    /// The focused pane of the active tab, when it still exists in the layout.
    pub fn focused_pane(&self) -> Option<PaneId> {
        let tab = self.active_tab()?;
        let focused = tab.focused?;
        tab.contains(focused).then_some(focused)
    }

    pub fn focused_location(&self) -> Option<(SpaceId, TabId, PaneId)> {
        let space = self.active_space();
        let tab = self.active_tab()?;
        Some((space.id, tab.id, self.focused_pane()?))
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_mut(&id)
    }

    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn pane_by_address(&self, address: &str) -> Option<&Pane> {
        self.panes.values().find(|pane| pane.address == address)
    }

    pub fn pane_id_by_address(&self, address: &str) -> Option<PaneId> {
        self.pane_by_address(address).map(|pane| pane.id)
    }

    /// The space/tab that owns a pane, if it is in a layout.
    pub fn location(&self, pane: PaneId) -> Option<(SpaceId, TabId)> {
        for space in &self.spaces {
            for tab in &space.tabs {
                if tab.contains(pane) {
                    return Some((space.id, tab.id));
                }
            }
        }
        None
    }

    pub fn tab(&self, space: SpaceId, tab: TabId) -> Option<&Tab> {
        self.space(space)?.tab(tab)
    }

    // --- starting terminals ----------------------------------------------

    /// Reserves ids and describes the terminal to start. The model is not
    /// changed until [`Workspace::insert_terminal`] succeeds; a failed
    /// `terminal.run` leaves no empty tab behind.
    pub fn start_request(&mut self, kind: StartKind) -> StartRequest {
        let pane = PaneId(self.ids.alloc());
        match kind {
            StartKind::Initial => {
                let (space, tab) = self.active_ids();
                StartRequest {
                    space,
                    tab,
                    pane,
                    placement: Placement::Only,
                    new_space: false,
                    new_tab: false,
                }
            }
            StartKind::Tab => StartRequest {
                space: self.active_space_id(),
                tab: TabId(self.ids.alloc()),
                pane,
                placement: Placement::Only,
                new_space: false,
                new_tab: true,
            },
            StartKind::Space => StartRequest {
                space: SpaceId(self.ids.alloc()),
                tab: TabId(self.ids.alloc()),
                pane,
                placement: Placement::Only,
                new_space: true,
                new_tab: true,
            },
            StartKind::Split(axis) => {
                let (space, tab) = self.active_ids();
                let placement = self
                    .focused_pane()
                    .map(|near| Placement::Split { axis, near })
                    .unwrap_or(Placement::Only);
                StartRequest {
                    space,
                    tab,
                    pane,
                    placement,
                    new_space: false,
                    new_tab: false,
                }
            }
        }
    }

    /// The size the reserved pane will attach at once
    /// [`Workspace::insert_terminal`] runs, for the layout as it is now at
    /// `viewport`. Insertion is deterministic, so the preview runs on a clone:
    /// a failed `terminal.run` must leave no empty tab behind.
    pub fn pending_attach_size(&self, request: &StartRequest, viewport: Rect) -> (u16, u16) {
        let mut preview = self.clone();
        preview.insert_terminal(request, String::new(), String::new());
        geometry::compute(preview.active_tab(), viewport)
            .pane(request.pane)
            .map(geometry::attach_size)
            .unwrap_or((1, 1))
    }

    /// Reserves ids for showing an existing core terminal. It goes next to the
    /// focused pane of the current tab; an empty tab takes it alone. The
    /// runtime never runs anything for this request: it calls
    /// [`Workspace::insert_adopted`] with the core record.
    pub fn adopt_request(&mut self) -> StartRequest {
        self.start_request(StartKind::Split(Axis::Columns))
    }

    /// Inserts a successfully started terminal. Creates the requested space or
    /// tab when needed, makes it active, focuses the pane and clears zoom.
    /// The returned id is stable for the life of the workspace.
    pub fn insert_terminal(
        &mut self,
        request: &StartRequest,
        address: String,
        name: String,
    ) -> PaneId {
        let pane = Pane {
            id: request.pane,
            address,
            name,
            labels: request.labels(),
            title: String::new(),
            status: None,
            owned: true,
            keep: false,
            exit: None,
        };
        self.insert_pane(request, pane)
    }

    /// Inserts a pane for a terminal that already runs in core. The record's
    /// labels, name, title, status and keep flag are kept; the pane is not
    /// owned, so closing it only detaches. Returns `None` when the address is
    /// empty or already shown, so a terminal never gets a second pane.
    pub fn insert_adopted(
        &mut self,
        request: &StartRequest,
        terminal: AdoptedTerminal,
    ) -> Option<PaneId> {
        if terminal.address.is_empty() || self.pane_by_address(&terminal.address).is_some() {
            return None;
        }
        let name = if terminal.name.is_empty() {
            terminal.address.clone()
        } else {
            terminal.name
        };
        let pane = Pane {
            id: request.pane,
            address: terminal.address,
            name,
            labels: terminal.labels,
            title: terminal.title,
            status: terminal.status,
            owned: false,
            keep: terminal.keep,
            exit: None,
        };
        Some(self.insert_pane(request, pane))
    }

    /// The shared insertion logic: create the requested space/tab, place the
    /// pane in the layout, make it active and focus it. The pane's id is
    /// `request.pane`; the caller has already checked for duplicates.
    fn insert_pane(&mut self, request: &StartRequest, mut pane: Pane) -> PaneId {
        let pane_id = request.pane;
        pane.id = pane_id;
        self.ids.mark(pane_id.0);
        if !self.spaces.iter().any(|space| space.id == request.space) {
            let space_name = self.space_name();
            let tab_name = self.tab_name();
            self.spaces.push(Space {
                id: request.space,
                name: space_name,
                folder: String::new(),
                tabs: vec![Tab::new(request.tab, tab_name)],
                active: request.tab,
            });
        }
        let si = self
            .spaces
            .iter()
            .position(|space| space.id == request.space)
            .unwrap_or(0);
        let ti = match self.spaces[si]
            .tabs
            .iter()
            .position(|tab| tab.id == request.tab)
        {
            Some(ti) => ti,
            None => {
                let name = self.tab_name();
                self.spaces[si].tabs.push(Tab::new(request.tab, name));
                self.spaces[si].tabs.len() - 1
            }
        };
        let tab_id = self.spaces[si].tabs[ti].id;
        self.active = self.spaces[si].id;
        self.spaces[si].active = tab_id;

        self.panes.insert(pane_id, pane);

        let tab = &mut self.spaces[si].tabs[ti];
        let leaf = Layout::Leaf(pane_id);
        tab.layout = Some(match (tab.layout.take(), request.placement) {
            (None, _) => leaf,
            (Some(root), Placement::Only) => Layout::Split {
                axis: Axis::Columns,
                children: vec![Child::new(root), Child::new(leaf)],
            },
            (Some(mut root), Placement::Split { axis, near }) => {
                if root.split(near, axis, pane_id) {
                    root
                } else if let Some(target) = tab.focused.filter(|pane| root.contains(*pane))
                    && root.split(target, axis, pane_id)
                {
                    root
                } else {
                    Layout::Split {
                        axis,
                        children: vec![Child::new(root), Child::new(leaf)],
                    }
                }
            }
        });
        focus(tab, pane_id);
        tab.zoomed = None;
        pane_id
    }

    // --- removing panes and closing tabs/spaces --------------------------

    /// Removes a pane from its layout and the workspace. Closes the tab when
    /// it was its last pane and the space when it was its last tab. Keeps one
    /// empty space/tab, so the workspace is never invalid.
    pub fn remove_pane(&mut self, pane: PaneId) -> bool {
        if self.panes.remove(&pane).is_none() {
            return false;
        }
        let Some((space_id, tab_id)) = self.location(pane) else {
            return true;
        };
        let Some(si) = self.spaces.iter().position(|space| space.id == space_id) else {
            return true;
        };
        let Some(ti) = self.spaces[si].tabs.iter().position(|tab| tab.id == tab_id) else {
            return true;
        };
        {
            let tab = &mut self.spaces[si].tabs[ti];
            if let Some(root) = tab.layout.take() {
                tab.layout = remove_layout(root, pane);
            }
            tab.history.retain(|id| *id != pane);
            if tab.zoomed == Some(pane) {
                tab.zoomed = None;
            }
            if tab.focused == Some(pane) {
                refocus(tab);
            }
        }
        if self.spaces[si].tabs[ti].layout.is_some() {
            return true;
        }

        // The tab is empty: close it.
        let space = &mut self.spaces[si];
        space.tabs.remove(ti);
        if space.tabs.is_empty() {
            self.spaces.remove(si);
            if self.active == space_id {
                self.active = self
                    .spaces
                    .get(si.min(self.spaces.len().saturating_sub(1)))
                    .map(|space| space.id)
                    .unwrap_or(SpaceId(0));
            }
            if self.spaces.is_empty() {
                self.add_empty_space();
            }
        } else if space.active == tab_id || !space.tabs.iter().any(|tab| tab.id == space.active) {
            let next = ti.min(space.tabs.len().saturating_sub(1));
            space.active = space.tabs[next].id;
        }
        true
    }

    // --- focus, move, zoom ------------------------------------------------

    /// Focuses the nearest pane in `direction`, using the full layout even
    /// while zoomed. A successful focus leaves zoom, so the target is visible.
    pub fn focus_direction(&mut self, viewport: Rect, direction: Direction) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let Some(focused) = tab.focused else {
            return false;
        };
        let rects = geometry::compute_without_zoom(Some(tab), viewport);
        let Some(next) = geometry::neighbor(&rects, focused, direction) else {
            return false;
        };
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        focus(tab, next);
        tab.zoomed = None;
        true
    }

    /// Focuses a pane in the active tab (a mouse click). Does not change the
    /// active tab or space.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        if !tab.contains(pane) || tab.focused == Some(pane) {
            return false;
        }
        focus(tab, pane);
        true
    }

    /// Selects a tab of the active space by id (a tab-bar click).
    pub fn select_tab_id(&mut self, id: TabId) -> bool {
        let Some(space) = self.active_space_mut() else {
            return false;
        };
        if space.active == id || !space.tabs.iter().any(|tab| tab.id == id) {
            return false;
        }
        space.active = id;
        true
    }

    /// Moves the split boundary at `path`/`index` by `delta` cells (positive
    /// right/down), keeping both sides at least [`geometry::MIN_PANE`] when
    /// they have room. The new spans become the children's weights, so the
    /// next resize keeps the ratio. Zoom hides boundaries, so a zoomed tab
    /// refuses the resize.
    pub fn resize_gap(&mut self, viewport: Rect, path: &[usize], index: usize, delta: i16) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        if tab.zoomed.is_some() {
            return false;
        }
        let (_, _, workspace) = geometry::bars(viewport);
        let Some(root) = tab.layout.as_ref() else {
            return false;
        };
        let mut node = root;
        let mut area = workspace;
        for &child_index in path {
            let Layout::Split { axis, children } = node else {
                return false;
            };
            let length = match axis {
                Axis::Columns => area.width,
                Axis::Rows => area.height,
            };
            let weights: Vec<u32> = children.iter().map(|child| child.size).collect();
            let (spans, gap) = geometry::distribute(length, &weights);
            let Some(span) = spans.get(child_index) else {
                return false;
            };
            let before: u32 = spans
                .iter()
                .take(child_index)
                .map(|span| u32::from(*span))
                .sum();
            let offset = before + u32::from(gap) * child_index as u32;
            area = match axis {
                Axis::Columns => Rect::new(
                    area.x.saturating_add(offset as u16),
                    area.y,
                    *span,
                    area.height,
                ),
                Axis::Rows => Rect::new(
                    area.x,
                    area.y.saturating_add(offset as u16),
                    area.width,
                    *span,
                ),
            };
            node = &children[child_index].layout;
        }
        let Layout::Split { axis, children } = node else {
            return false;
        };
        if index + 1 >= children.len() {
            return false;
        }
        let length = match axis {
            Axis::Columns => area.width,
            Axis::Rows => area.height,
        };
        let weights: Vec<u32> = children.iter().map(|child| child.size).collect();
        let (spans, _) = geometry::distribute(length, &weights);
        let (Some(&before), Some(&after)) = (spans.get(index), spans.get(index + 1)) else {
            return false;
        };
        let floor = i32::from(geometry::MIN_PANE.min(before).min(after));
        let delta = i32::from(delta).clamp(floor - i32::from(before), i32::from(after) - floor);
        if delta == 0 {
            return false;
        }
        let mut next = spans;
        next[index] = (i32::from(before) + delta) as u16;
        next[index + 1] = (i32::from(after) - delta) as u16;

        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        let Some(Layout::Split { children, .. }) =
            tab.layout.as_mut().and_then(|root| split_at(root, path))
        else {
            return false;
        };
        for (child, span) in children.iter_mut().zip(next.iter()) {
            child.size = u32::from(span.saturating_sub(geometry::MIN_PANE));
        }
        true
    }

    /// Swaps the focused pane with its nearest neighbour in `direction`.
    /// The pane id and the focus stay the same; only the layout positions
    /// move. Only panes in the same tab take part.
    pub fn move_direction(&mut self, viewport: Rect, direction: Direction) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let Some(focused) = tab.focused else {
            return false;
        };
        let rects = geometry::compute_without_zoom(Some(tab), viewport);
        let Some(next) = geometry::neighbor(&rects, focused, direction) else {
            return false;
        };
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        let Some(root) = tab.layout.as_mut() else {
            return false;
        };
        swap_leaves(root, focused, next);
        true
    }

    /// Selects the space and tab holding `pane`, focuses it and clears zoom.
    /// A jump is local: no layout, label or document changes.
    pub fn jump_to(&mut self, pane: PaneId) -> bool {
        let Some((space_id, tab_id)) = self.location(pane) else {
            return false;
        };
        let Some(space_index) = self.spaces.iter().position(|space| space.id == space_id) else {
            return false;
        };
        let Some(tab_index) = self.spaces[space_index]
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
        else {
            return false;
        };
        self.active = space_id;
        self.spaces[space_index].active = tab_id;
        let tab = &mut self.spaces[space_index].tabs[tab_index];
        if !tab.contains(pane) {
            return false;
        }
        focus(tab, pane);
        tab.zoomed = None;
        true
    }

    /// Toggles zoom on the focused pane. Zoom is per tab.
    pub fn toggle_zoom(&mut self) -> bool {
        let Some(tab) = self.active_tab_mut() else {
            return false;
        };
        let Some(focused) = tab.focused else {
            return false;
        };
        tab.zoomed = if tab.zoomed == Some(focused) {
            None
        } else {
            Some(focused)
        };
        true
    }

    // --- tabs and spaces --------------------------------------------------

    pub fn next_tab(&mut self) -> bool {
        self.step_tab(1)
    }

    pub fn prev_tab(&mut self) -> bool {
        self.step_tab(-1)
    }

    fn step_tab(&mut self, step: isize) -> bool {
        let Some(space) = self.active_space_mut() else {
            return false;
        };
        let Some(index) = space.tabs.iter().position(|tab| tab.id == space.active) else {
            return false;
        };
        let len = space.tabs.len() as isize;
        let next = (index as isize + step).rem_euclid(len) as usize;
        let id = space.tabs[next].id;
        if space.active == id {
            return false;
        }
        space.active = id;
        true
    }

    /// Selects the `tab`th tab of the active space (1-based).
    pub fn select_tab(&mut self, tab: u8) -> bool {
        let Some(space) = self.active_space_mut() else {
            return false;
        };
        let Some(index) = usize::from(tab).checked_sub(1) else {
            return false;
        };
        let Some(target) = space.tabs.get(index) else {
            return false;
        };
        let id = target.id;
        if space.active == id {
            return false;
        }
        space.active = id;
        true
    }

    pub fn next_space(&mut self) -> bool {
        self.step_space(1)
    }

    pub fn prev_space(&mut self) -> bool {
        self.step_space(-1)
    }

    fn step_space(&mut self, step: isize) -> bool {
        let Some(index) = self.spaces.iter().position(|space| space.id == self.active) else {
            return false;
        };
        let len = self.spaces.len() as isize;
        let next = (index as isize + step).rem_euclid(len) as usize;
        let id = self.spaces[next].id;
        if self.active == id {
            return false;
        }
        self.active = id;
        true
    }

    // --- naming -----------------------------------------------------------

    pub fn rename_tab(&mut self, space_id: SpaceId, tab_id: TabId, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(space) = self
            .spaces
            .iter_mut()
            .find(|candidate| candidate.id == space_id)
        else {
            return false;
        };
        let Some(tab) = space.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };
        if tab.name == name {
            return false;
        }
        tab.name = name.to_owned();
        true
    }

    pub fn rename_space(&mut self, space: SpaceId, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(space) = self
            .spaces
            .iter_mut()
            .find(|candidate| candidate.id == space)
        else {
            return false;
        };
        if space.name == name {
            return false;
        }
        space.name = name.to_owned();
        true
    }

    // --- display facts from core -----------------------------------------

    pub fn set_title(&mut self, pane: PaneId, title: String) -> bool {
        match self.panes.get_mut(&pane) {
            Some(record) if record.title != title => {
                record.title = title;
                true
            }
            _ => false,
        }
    }

    pub fn set_status(&mut self, pane: PaneId, status: Option<String>) -> bool {
        match self.panes.get_mut(&pane) {
            Some(record) if record.status != status => {
                record.status = status;
                true
            }
            _ => false,
        }
    }

    /// Synchronizes only the live attention labels (`herdr.seen`,
    /// `herdr.pinned`) from a core record. Pane identity labels are this
    /// client's and are never dropped by a partial or older snapshot record.
    pub fn set_attention_labels(
        &mut self,
        pane: PaneId,
        labels: &BTreeMap<String, String>,
    ) -> bool {
        let Some(record) = self.panes.get_mut(&pane) else {
            return false;
        };
        let before = record.labels.clone();
        for key in [SEEN_LABEL, PINNED_LABEL] {
            match labels.get(key) {
                Some(value) => {
                    record.labels.insert(key.to_owned(), value.clone());
                }
                None => {
                    record.labels.remove(key);
                }
            }
        }
        record.labels != before
    }

    /// Replaces a live pane's whole label map (an optimistic shared-label
    /// update or core's `terminal.label` reply).
    pub fn set_labels(&mut self, pane: PaneId, labels: BTreeMap<String, String>) -> bool {
        match self.panes.get_mut(&pane) {
            Some(record) if record.labels != labels => {
                record.labels = labels;
                true
            }
            _ => false,
        }
    }

    /// Marks a pane as a finished view with its exit status. Its final screen
    /// stays drawn until the person closes the pane.
    pub fn mark_finished(&mut self, pane: PaneId, code: i32) -> bool {
        match self.panes.get_mut(&pane) {
            Some(record) if record.exit != Some(code) => {
                record.exit = Some(code);
                true
            }
            _ => false,
        }
    }

    /// A kept terminal that got a fresh shell is no longer a finished view.
    pub fn mark_live(&mut self, pane: PaneId) -> bool {
        match self.panes.get_mut(&pane) {
            Some(record) if record.exit.is_some() => {
                record.exit = None;
                true
            }
            _ => false,
        }
    }

    // --- internals --------------------------------------------------------

    fn active_ids(&self) -> (SpaceId, TabId) {
        let space = self.active_space();
        let tab = space
            .tabs
            .iter()
            .find(|tab| tab.id == space.active)
            .or_else(|| space.tabs.first())
            .expect("a space has a tab");
        (space.id, tab.id)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let active = self.active;
        let space = self.spaces.iter_mut().find(|space| space.id == active)?;
        let tab_id = space.active;
        let index = space
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .or_else(|| (!space.tabs.is_empty()).then_some(0))?;
        space.tabs.get_mut(index)
    }

    fn active_space_mut(&mut self) -> Option<&mut Space> {
        self.spaces.iter_mut().find(|space| space.id == self.active)
    }

    fn space_name(&mut self) -> String {
        let number = self.next_space_name;
        self.next_space_name += 1;
        format!("space {number}")
    }

    fn tab_name(&mut self) -> String {
        let number = self.next_tab_name;
        self.next_tab_name += 1;
        format!("tab {number}")
    }

    fn add_empty_space(&mut self) {
        let space_id = SpaceId(self.ids.alloc());
        let tab_id = TabId(self.ids.alloc());
        let name = self.space_name();
        let tab_name = self.tab_name();
        self.spaces.push(Space {
            id: space_id,
            name,
            folder: String::new(),
            tabs: vec![Tab::new(tab_id, tab_name)],
            active: tab_id,
        });
        self.active = space_id;
    }
}

fn focus(tab: &mut Tab, pane: PaneId) {
    tab.history.retain(|id| *id != pane);
    tab.history.push(pane);
    tab.focused = Some(pane);
}

fn refocus(tab: &mut Tab) {
    let contains = |pane: PaneId| tab.contains(pane);
    let candidate = tab
        .history
        .iter()
        .rev()
        .copied()
        .find(|pane| contains(*pane))
        .or_else(|| tab.layout.as_ref().and_then(Layout::first_leaf));
    match candidate {
        Some(pane) => {
            tab.focused = Some(pane);
            tab.history.retain(|id| *id != pane);
            tab.history.push(pane);
        }
        None => {
            tab.focused = None;
            tab.history.clear();
        }
    }
}

fn remove_layout(layout: Layout, pane: PaneId) -> Option<Layout> {
    match layout {
        Layout::Leaf(id) => (id != pane).then_some(Layout::Leaf(id)),
        Layout::Split { axis, children } => {
            let children: Vec<Child> = children
                .into_iter()
                .filter_map(|child| {
                    remove_layout(child.layout, pane).map(|layout| Child {
                        size: child.size,
                        layout,
                    })
                })
                .collect();
            match children.len() {
                0 => None,
                1 => children.into_iter().next().map(|child| child.layout),
                _ => Some(Layout::Split { axis, children }),
            }
        }
    }
}

fn split_at<'a>(layout: &'a mut Layout, path: &[usize]) -> Option<&'a mut Layout> {
    match path.split_first() {
        Some((&index, rest)) => match layout {
            Layout::Split { children, .. } => split_at(&mut children.get_mut(index)?.layout, rest),
            Layout::Leaf(_) => None,
        },
        None => Some(layout),
    }
}

fn swap_leaves(layout: &mut Layout, a: PaneId, b: PaneId) {
    match layout {
        Layout::Leaf(id) => {
            if *id == a {
                *id = b;
            } else if *id == b {
                *id = a;
            }
        }
        Layout::Split { children, .. } => {
            for child in children {
                swap_leaves(&mut child.layout, a, b);
            }
        }
    }
}

#[cfg(test)]
mod tests;
