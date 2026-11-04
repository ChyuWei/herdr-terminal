//! Agent facts derived from core, without IO.
//!
//! An agent is a terminal in this workspace whose core record says it reported
//! through OSC 7501 (`reported` or a current `status`). Nothing is inferred
//! from titles, argv, process names or screen text. This module owns the
//! derived row word, unseen rules, ordering, visible-set rules, seen-write
//! decisions, attention counts and alert transitions; the runtime performs the
//! `terminal.label` calls and the UI draws the rows.

use crate::core::{Foreground, Status, Terminal};
use crate::model::Model;
pub use crate::workspace::model::{PINNED_LABEL, SEEN_LABEL};
use crate::workspace::model::{PaneId, Workspace};
use std::collections::{BTreeMap, BTreeSet};

// The shared attention labels live in the document schema so capturing a
// layout can always filter them: `herdr.seen` is the last turn the person
// saw (decimal), and `herdr.pinned` marks a pinned agent (present and
// non-empty means pinned).

/// The attention/display group of a row. Lower sorts first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Blocked = 0,
    Done = 1,
    Working = 2,
    Idle = 3,
}

/// One agent row, derived only from a core terminal record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRow {
    pub pane: PaneId,
    pub address: String,
    /// Status title when non-empty, else the core terminal title, else the
    /// pane display name.
    pub title: String,
    /// The pane display name, for the fallback and for show-address modes.
    pub name: String,
    /// The derived row word: blocked, working, done, idle, or the program's
    /// own non-empty word.
    pub word: String,
    pub priority: Priority,
    pub turn: u64,
    pub pinned: bool,
    /// True when the person has seen the current turn.
    pub seen: bool,
    pub seen_turn: Option<u64>,
    pub status: Option<Status>,
    pub foreground: Option<Foreground>,
    pub state: String,
    pub exit: Option<i64>,
    /// Workspace order: (space index, tab index, layout index).
    pub order: (usize, usize, usize),
}

impl AgentRow {
    /// The app name for the row: the status app, else the foreground name.
    pub fn app(&self) -> &str {
        match &self.status {
            Some(status) if !status.app.is_empty() => &status.app,
            _ => self
                .foreground
                .as_ref()
                .map(|foreground| foreground.name.as_str())
                .unwrap_or(""),
        }
    }
}

/// Whether a core record makes its terminal an agent. Only the program's own
/// 7501 report counts; a title, argv or process name never does.
pub fn is_agent(terminal: &Terminal) -> bool {
    terminal.reported || terminal.status.is_some()
}

/// The seen turn stored in a terminal's labels. A missing or unparsable label
/// is `None`; a missing label is not the same as a label of `0`.
pub fn seen_turn(labels: &BTreeMap<String, String>) -> Option<u64> {
    labels.get(SEEN_LABEL)?.parse().ok()
}

/// A pinned agent carries the non-empty shared pin label.
pub fn pinned(labels: &BTreeMap<String, String>) -> bool {
    labels
        .get(PINNED_LABEL)
        .is_some_and(|value| !value.is_empty())
}

/// Whether the current turn is unseen. A present `herdr.seen` label is
/// authoritative over the transient local flag used when no label exists.
pub fn unseen(turn: u64, labels: &BTreeMap<String, String>, local_seen: bool) -> bool {
    match seen_turn(labels) {
        Some(seen) => turn > seen,
        None => !local_seen,
    }
}

/// The one word a row shows. `blocked` and `error` both display as blocked;
/// `done` and a cleared status are done while unseen and idle once seen; any
/// other non-empty word is shown as the program reported it.
pub fn derive_word(status: Option<&Status>, unseen: bool) -> String {
    match status.map(|status| status.state.as_str()) {
        Some("blocked" | "error") => "blocked".to_owned(),
        Some("working") => "working".to_owned(),
        Some("done") => if unseen { "done" } else { "idle" }.to_owned(),
        Some("") | None => if unseen { "done" } else { "idle" }.to_owned(),
        Some(other) => other.to_owned(),
    }
}

/// The ordering group for a displayed word.
pub fn priority(word: &str) -> Priority {
    match word {
        "blocked" => Priority::Blocked,
        "done" => Priority::Done,
        "working" => Priority::Working,
        _ => Priority::Idle,
    }
}

impl AgentRow {
    /// Derives one row, or `None` when the record is not an agent.
    pub fn from_terminal(
        pane: PaneId,
        name: &str,
        order: (usize, usize, usize),
        terminal: &Terminal,
        local_seen: bool,
    ) -> Option<AgentRow> {
        if !is_agent(terminal) {
            return None;
        }
        let status = terminal.status.clone();
        let unseen = unseen(terminal.turn, &terminal.labels, local_seen);
        let word = derive_word(status.as_ref(), unseen);
        let title = match &status {
            Some(status) if !status.title.is_empty() => status.title.clone(),
            _ if !terminal.title.is_empty() => terminal.title.clone(),
            _ => name.to_owned(),
        };
        Some(AgentRow {
            pane,
            address: terminal.address.clone(),
            title,
            name: name.to_owned(),
            priority: priority(&word),
            word,
            turn: terminal.turn,
            pinned: pinned(&terminal.labels),
            seen: !unseen,
            seen_turn: seen_turn(&terminal.labels),
            status,
            foreground: terminal.foreground.clone(),
            state: terminal.state.clone(),
            exit: terminal.exit_code,
            order,
        })
    }
}

/// Every agent in the workspace, in workspace order before sorting. Panes
/// whose core record is missing or never reported are not agents.
pub fn workspace_rows(
    workspace: &Workspace,
    model: &Model,
    local_seen: &BTreeMap<String, bool>,
) -> Vec<AgentRow> {
    let mut rows = Vec::new();
    for (space_index, space) in workspace.spaces().iter().enumerate() {
        for (tab_index, tab) in space.tabs.iter().enumerate() {
            for (layout_index, pane_id) in tab.panes().iter().enumerate() {
                let Some(pane) = workspace.pane(*pane_id) else {
                    continue;
                };
                let Some(terminal) = model.terminal(&pane.address) else {
                    continue;
                };
                let seen = local_seen.get(&pane.address).copied().unwrap_or(true);
                if let Some(row) = AgentRow::from_terminal(
                    *pane_id,
                    &pane.name,
                    (space_index, tab_index, layout_index),
                    terminal,
                    seen,
                ) {
                    rows.push(row);
                }
            }
        }
    }
    rows
}

/// The sidebar order: pinned first, then blocked > done > working > idle, then
/// workspace order, then address as a stable tie-break.
pub fn sort_rows(rows: &mut [AgentRow]) {
    rows.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(a.priority.cmp(&b.priority))
            .then(a.order.cmp(&b.order))
            .then(a.address.cmp(&b.address))
    });
}

/// The transient local seen flag for a terminal with no `herdr.seen` label.
/// Looking sets it; leaving a busy word for a non-busy one while not looking
/// clears it.
pub fn update_local_seen(
    previous_word: Option<&str>,
    status: Option<&Status>,
    visible: bool,
    current: bool,
) -> bool {
    if visible {
        return true;
    }
    let was_busy = matches!(previous_word, Some("working" | "blocked"));
    let now_busy = matches!(
        status.map(|status| status.state.as_str()),
        Some("working" | "blocked" | "error")
    );
    if was_busy && !now_busy {
        false
    } else {
        current
    }
}

/// A word that means the agent needs the person or is actively working.
pub fn busy(word: &str) -> bool {
    matches!(word, "working" | "blocked")
}

/// Whether a person is looking at a pane right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Prompt,
    Palette,
    Search,
    Manager,
    Machines,
    Files,
    Preview,
    Peek(PaneId),
    Sidebar,
}

/// The panes whose output the person is watching. An unfocused window sees
/// none; help, prompt and the other overlays cover every pane, and a focused
/// sidebar counts as covering them because keyboard attention is on the panel;
/// a peek shows only its target; otherwise every pane in the active tab is
/// visible, or only the zoomed pane.
pub fn visible_panes(
    workspace: &Workspace,
    window_focused: bool,
    overlay: Overlay,
) -> BTreeSet<PaneId> {
    let mut visible = BTreeSet::new();
    if !window_focused {
        return visible;
    }
    match overlay {
        Overlay::Help
        | Overlay::Prompt
        | Overlay::Palette
        | Overlay::Search
        | Overlay::Sidebar
        | Overlay::Manager
        | Overlay::Machines
        | Overlay::Files
        | Overlay::Preview => {
            return visible;
        }
        Overlay::Peek(pane) => {
            if workspace.pane(pane).is_some() {
                visible.insert(pane);
            }
            return visible;
        }
        Overlay::None => {}
    }
    let Some(tab) = workspace.active_tab() else {
        return visible;
    };
    if let Some(zoomed) = tab.zoomed {
        if tab.contains(zoomed) {
            visible.insert(zoomed);
        }
        return visible;
    }
    visible.extend(tab.panes());
    visible
}

/// One seen write the runtime should make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenWrite {
    pub address: String,
    pub turn: u64,
}

/// The seen writes for visible agents. Writes only raise, never repeat a
/// `(address, turn)` already attempted, and never repeat a turn already seen.
pub fn seen_writes(
    rows: &[AgentRow],
    visible: &BTreeSet<PaneId>,
    attempted: &BTreeMap<String, u64>,
) -> Vec<SeenWrite> {
    let mut writes = Vec::new();
    for row in rows {
        if !visible.contains(&row.pane) {
            continue;
        }
        let needs = match seen_turn_from_row(row) {
            Some(seen) => seen < row.turn,
            None => true,
        };
        if !needs {
            continue;
        }
        if attempted
            .get(&row.address)
            .is_some_and(|last| *last >= row.turn)
        {
            continue;
        }
        writes.push(SeenWrite {
            address: row.address.clone(),
            turn: row.turn,
        });
    }
    writes
}

fn seen_turn_from_row(row: &AgentRow) -> Option<u64> {
    row.seen_turn
}

/// Mark unread lowers `herdr.seen` to `turn - 1`, and only when there is a
/// turn to mark unread.
pub fn unread_value(turn: u64) -> Option<u64> {
    turn.checked_sub(1)
}

/// The status-bar attention count: blocked always, done only while unseen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    pub blocked: usize,
    pub done: usize,
}

impl Attention {
    pub fn text(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.blocked > 0 {
            parts.push(format!("{} blocked", self.blocked));
        }
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

pub fn attention(rows: &[AgentRow]) -> Attention {
    let mut attention = Attention::default();
    for row in rows {
        match row.word.as_str() {
            "blocked" => attention.blocked += 1,
            "done" => attention.done += 1,
            _ => {}
        }
    }
    attention
}

/// A notification an agent transition calls for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// A newly blocked/error agent needs the person.
    NeedsYou,
    /// A busy agent finished while the person was not looking at it.
    Finished,
}

/// A transition between two derived row words. The runtime never calls this
/// for its first (baseline) pass; `previous` is `None` when an agent appears
/// after that baseline, in which case only a blocked word alerts. A repeated
/// identical word is not a transition; a newly blocked word is shown even
/// while looking.
pub fn transition(previous: Option<&str>, current: &str, visible: bool) -> Option<Alert> {
    if previous == Some(current) {
        return None;
    }
    if current == "blocked" {
        return Some(Alert::NeedsYou);
    }
    if let Some(previous) = previous
        && busy(previous)
        && !busy(current)
        && !visible
    {
        return Some(Alert::Finished);
    }
    None
}

/// A jump direction from the prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Next,
    Previous,
}

/// The next agent to jump to. From an agent, cycle through sidebar order;
/// from a non-agent or no focus, start at the first (`Next`) or last
/// (`Previous`) row.
pub fn jump_target(rows: &[AgentRow], current: Option<PaneId>, jump: Jump) -> Option<PaneId> {
    if rows.is_empty() {
        return None;
    }
    if let Some(current) = current
        && let Some(index) = rows.iter().position(|row| row.pane == current)
    {
        let next = match jump {
            Jump::Next => (index + 1) % rows.len(),
            Jump::Previous => (index + rows.len() - 1) % rows.len(),
        };
        return Some(rows[next].pane);
    }
    match jump {
        Jump::Next => rows.first(),
        Jump::Previous => rows.last(),
    }
    .map(|row| row.pane)
}

/// Keeps a selection by pane id across reordering. When the pane is gone, the
/// row that took its old index (clamped) or the last row is selected.
pub fn selection_after(
    rows: &[AgentRow],
    selected: Option<PaneId>,
    previous_index: Option<usize>,
) -> Option<PaneId> {
    if let Some(selected) = selected
        && rows.iter().any(|row| row.pane == selected)
    {
        return Some(selected);
    }
    if rows.is_empty() {
        return None;
    }
    let index = previous_index
        .unwrap_or(0)
        .min(rows.len().saturating_sub(1));
    Some(rows[index].pane)
}

/// The sidebar scroll offset that keeps `selected` visible in `height` rows.
pub fn scroll_to(offset: usize, selected: Option<usize>, height: usize, len: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    let max = len - height;
    let mut offset = offset.min(max);
    if let Some(selected) = selected {
        if selected < offset {
            offset = selected;
        } else if selected >= offset + height {
            offset = selected + 1 - height;
        }
    }
    offset.min(max)
}

/// The label map for a pin write: `herdr.pinned=1`, or an empty value to
/// remove it.
pub fn pin_labels(pinned: bool) -> BTreeMap<String, String> {
    BTreeMap::from([(
        PINNED_LABEL.to_owned(),
        if pinned { "1" } else { "" }.to_owned(),
    )])
}

/// The label map for a seen write.
pub fn seen_labels(turn: u64) -> BTreeMap<String, String> {
    BTreeMap::from([(SEEN_LABEL.to_owned(), turn.to_string())])
}

#[cfg(test)]
mod tests;
