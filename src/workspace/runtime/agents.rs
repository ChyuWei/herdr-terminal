//! Agent runtime: derives rows from core, writes shared seen/pin labels with
//! bounded retries, drives the modal sidebar and peek, jumps and attention
//! alerts. All decisions are in [`super::super::agents`]; this file performs
//! the IO and keeps the small amount of per-turn bookkeeping.

use super::Screen;
use crate::core::{Error, Result, Terminal};
use crate::session;
use crate::workspace::agents::{
    self, AgentRow, Alert, Jump, SEEN_LABEL, SeenWrite, pin_labels, seen_labels, seen_turn,
    workspace_rows,
};
use crate::workspace::model::PaneId;
use crossterm::execute;
use crossterm::style::Print;
use std::collections::BTreeMap;
use std::io::stdout;
use std::time::{Duration, Instant};

/// How long a failed seen write waits before another bounded attempt.
const SEEN_RETRY: Duration = Duration::from_millis(1000);
/// How many attempts one `(address, turn)` seen write gets in total: the
/// first try plus two retries.
const SEEN_RETRY_ATTEMPTS: u8 = 3;

/// Per-screen agent bookkeeping. Nothing here is persisted.
#[derive(Default)]
pub(super) struct AgentRuntime {
    /// The current sorted rows, for input and drawing.
    pub(super) rows: Vec<AgentRow>,
    /// The displayed word last derived for each address, for transitions.
    words: BTreeMap<String, String>,
    /// Transient seen flags for terminals without a `herdr.seen` label.
    local_seen: BTreeMap<String, bool>,
    /// The last turn a seen write was attempted for, per address.
    attempted: BTreeMap<String, u64>,
    /// Failed seen writes waiting for a bounded retry.
    retries: BTreeMap<String, SeenRetry>,
    /// The first evaluation only records the baseline; it never alerts.
    baseline: bool,
}

#[derive(Clone, Debug)]
struct SeenRetry {
    turn: u64,
    due: Instant,
    left: u8,
}

impl AgentRuntime {
    /// After `missed` or a reconnect: rebuild the baseline from the fresh
    /// snapshot and reset per-turn write bookkeeping. Local seen flags stay
    /// as they were, so an unread agent is not silently marked seen.
    pub(super) fn reset(&mut self) {
        self.words.clear();
        self.attempted.clear();
        self.retries.clear();
        self.baseline = false;
        self.rows.clear();
    }
}

impl Screen<'_> {
    /// Recomputes agent facts, marks visible turns seen, fires transitions and
    /// repairs the sidebar selection. Called once per live loop iteration.
    pub(super) fn refresh_agents(&mut self) -> Result<()> {
        self.refresh_pending_labels()?;
        if self.reconn.is_some() {
            return Ok(());
        }

        let overlay = self.ui.agent_overlay();
        let visible = agents::visible_panes(&self.workspace, self.outer_focused, overlay);

        // The local seen flag only matters where no label exists. Looking sets
        // it; a hidden busy→non-busy transition clears it.
        let mut local_seen = std::mem::take(&mut self.agents.local_seen);
        let panes: Vec<(PaneId, String)> = self
            .workspace
            .panes()
            .map(|pane| (pane.id, pane.address.clone()))
            .collect();
        for (pane, address) in &panes {
            let Some(record) = self.model.terminal(address) else {
                continue;
            };
            if !agents::is_agent(record) || seen_turn(&record.labels).is_some() {
                continue;
            }
            let current = local_seen.get(address).copied().unwrap_or(true);
            let previous = self.agents.words.get(address).map(String::as_str);
            let next = agents::update_local_seen(
                previous,
                record.status.as_ref(),
                visible.contains(pane),
                current,
            );
            local_seen.insert(address.clone(), next);
        }
        self.agents.local_seen = local_seen;

        let mut rows = workspace_rows(&self.workspace, &self.model, &self.agents.local_seen);
        agents::sort_rows(&mut rows);

        let writes = agents::seen_writes(&rows, &visible, &self.agents.attempted);
        for write in writes {
            self.write_seen(&write)?;
            if self.reconn.is_some() {
                return Ok(());
            }
        }

        // Rebuild after the optimistic seen labels so the row shows idle.
        let mut rows = workspace_rows(&self.workspace, &self.model, &self.agents.local_seen);
        agents::sort_rows(&mut rows);
        self.apply_transitions(&rows, &visible);
        self.keep_sidebar_selection(&rows);

        if self
            .ui
            .peek
            .is_some_and(|pane| self.workspace.pane(pane).is_none())
        {
            self.ui.peek = None;
            self.dirty = true;
        }
        self.agents.rows = rows;
        Ok(())
    }

    // --- seen / pin / unread ----------------------------------------------

    fn write_seen(&mut self, write: &SeenWrite) -> Result<()> {
        self.agents
            .attempted
            .insert(write.address.clone(), write.turn);
        self.set_local_label(&write.address, SEEN_LABEL, Some(&write.turn.to_string()));
        let labels = seen_labels(write.turn);
        match self.client.label(&write.address, &labels) {
            Ok(record) => {
                self.agents.retries.remove(&write.address);
                self.apply_labels(&write.address, &record);
            }
            Err(error) if session::is_lost(&error) => self.begin_reconnect(),
            Err(error) => self.seen_failed(&write.address, write.turn, SEEN_RETRY_ATTEMPTS, &error),
        }
        Ok(())
    }

    /// Retries due seen writes. A retry is dropped when the turn moved on, the
    /// label was confirmed by a `labels` event, or the pane disappeared.
    fn refresh_pending_labels(&mut self) -> Result<()> {
        let now = Instant::now();
        let due: Vec<String> = self
            .agents
            .retries
            .iter()
            .filter(|(_, retry)| retry.due <= now)
            .map(|(address, _)| address.clone())
            .collect();
        for address in due {
            let Some(retry) = self.agents.retries.get(&address).cloned() else {
                continue;
            };
            if self.workspace.pane_by_address(&address).is_none() {
                self.agents.retries.remove(&address);
                continue;
            }
            let Some(record) = self.model.terminal(&address) else {
                self.agents.retries.remove(&address);
                continue;
            };
            if record.turn != retry.turn {
                self.agents.retries.remove(&address);
                continue;
            }
            match self.client.label(&address, &seen_labels(retry.turn)) {
                Ok(record) => {
                    self.agents.retries.remove(&address);
                    self.apply_labels(&address, &record);
                }
                Err(error) if session::is_lost(&error) => {
                    self.begin_reconnect();
                    return Ok(());
                }
                Err(error) => self.seen_failed(&address, retry.turn, retry.left, &error),
            }
        }
        Ok(())
    }

    fn seen_failed(&mut self, address: &str, turn: u64, left: u8, error: &Error) {
        if left <= 1 {
            self.agents.retries.remove(address);
            self.ui.message = Some(format!(
                "could not mark {address} seen: {error}; giving up until the next change"
            ));
        } else {
            self.agents.retries.insert(
                address.to_owned(),
                SeenRetry {
                    turn,
                    due: Instant::now() + SEEN_RETRY,
                    left: left - 1,
                },
            );
            self.ui.message = Some(format!("could not mark {address} seen: {error}; retrying"));
        }
        self.dirty = true;
    }

    /// A `labels` event from core is authoritative: a retry whose turn is now
    /// covered by the label no longer needs to run.
    pub(super) fn note_labels(&mut self, address: &str) {
        let seen = self
            .model
            .terminal(address)
            .and_then(|record| seen_turn(&record.labels));
        if let Some(retry) = self.agents.retries.get(address)
            && seen.is_some_and(|seen| seen >= retry.turn)
        {
            self.agents.retries.remove(address);
        }
    }

    /// `p`: toggle the selected (or focused) agent's shared pin label.
    pub(super) fn toggle_pin(&mut self) -> Result<()> {
        let Some(row) = self.action_row() else {
            self.ui.message = Some("no agent selected".into());
            self.dirty = true;
            return Ok(());
        };
        let pinned = !row.pinned;
        self.write_shared_labels(&row.address, pin_labels(pinned))?;
        Ok(())
    }

    /// `u`: mark the selected (or focused) agent unread by lowering its seen
    /// label to `turn - 1`. The only lower write this client makes.
    pub(super) fn mark_unread(&mut self) -> Result<()> {
        let Some(row) = self.action_row() else {
            self.ui.message = Some("no agent selected".into());
            self.dirty = true;
            return Ok(());
        };
        let Some(turn) = agents::unread_value(row.turn) else {
            self.ui.message = Some(format!("{} has no turn to mark unread", row.address));
            self.dirty = true;
            return Ok(());
        };
        let mut labels = BTreeMap::new();
        labels.insert(SEEN_LABEL.to_owned(), turn.to_string());
        self.write_shared_labels(&row.address, labels)?;
        // A lower seen value is intentional; the per-turn automatic guard
        // must not suppress a future raise back to the same turn.
        self.agents.attempted.remove(&row.address);
        Ok(())
    }

    /// A user-requested pin/unread write: optimistic, then core's reply (and
    /// any later `labels` event) is authoritative. A failure restores the
    /// previous labels and reports it.
    fn write_shared_labels(
        &mut self,
        address: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<()> {
        let before = self.current_labels(address);
        for (key, value) in &labels {
            self.set_local_label(address, key, Some(value));
        }
        match self.client.label(address, &labels) {
            Ok(record) => self.apply_labels(address, &record),
            Err(error) if session::is_lost(&error) => self.begin_reconnect(),
            Err(error) => {
                self.apply_label_map(address, &before);
                self.ui.message = Some(format!("could not update {address}: {error}"));
                self.dirty = true;
            }
        }
        Ok(())
    }

    fn current_labels(&self, address: &str) -> BTreeMap<String, String> {
        self.model
            .terminal(address)
            .map(|record| record.labels.clone())
            .or_else(|| {
                self.workspace
                    .pane_by_address(address)
                    .map(|pane| pane.labels.clone())
            })
            .unwrap_or_default()
    }

    fn set_local_label(&mut self, address: &str, key: &str, value: Option<&str>) {
        let mut labels = self.current_labels(address);
        match value {
            Some(value) => {
                labels.insert(key.to_owned(), value.to_owned());
            }
            None => {
                labels.remove(key);
            }
        }
        self.apply_label_map(address, &labels);
    }

    fn apply_labels(&mut self, address: &str, record: &Terminal) {
        self.apply_label_map(address, &record.labels);
    }

    fn apply_label_map(&mut self, address: &str, labels: &BTreeMap<String, String>) {
        self.model.set_labels(address, labels.clone());
        if let Some(pane) = self.workspace.pane_by_address(address) {
            let id = pane.id;
            if self.workspace.set_labels(id, labels.clone()) {
                self.dirty = true;
            }
        }
    }

    // --- transitions and attention ---------------------------------------

    fn apply_transitions(
        &mut self,
        rows: &[AgentRow],
        visible: &std::collections::BTreeSet<PaneId>,
    ) {
        if !self.agents.baseline {
            self.agents.baseline = true;
            self.agents.words = rows
                .iter()
                .map(|row| (row.address.clone(), row.word.clone()))
                .collect();
            return;
        }
        for row in rows {
            let previous = self.agents.words.get(&row.address).map(String::as_str);
            if let Some(alert) =
                agents::transition(previous, &row.word, visible.contains(&row.pane))
            {
                self.notify(row, alert);
            }
        }
        self.agents.words = rows
            .iter()
            .map(|row| (row.address.clone(), row.word.clone()))
            .collect();
    }

    fn notify(&mut self, row: &AgentRow, alert: Alert) {
        let what = match alert {
            Alert::NeedsYou => "needs you",
            Alert::Finished => "finished",
        };
        let title = if row.title.is_empty() {
            row.address.as_str()
        } else {
            row.title.as_str()
        };
        self.ui.message = Some(format!("{title} ({}) {} · {what}", row.address, row.word));
        self.dirty = true;
        // One BEL per transition, inside the alternate screen; no OS
        // notification and no external service.
        if let Err(error) = execute!(stdout(), Print('\u{7}')) {
            let _ = error;
        }
    }

    // --- sidebar ----------------------------------------------------------

    /// The rail's cells right now: the persistent column on a wide terminal,
    /// the overlay drawer while it is summoned on a narrow one.
    pub(super) fn rail_rect(&self) -> ratatui::layout::Rect {
        let bars = super::super::geometry::bars(self.area());
        if bars.rail.width > 0 {
            bars.rail
        } else if self.ui.sidebar.open {
            super::super::sidebar::drawer_rect(self.area())
        } else {
            ratatui::layout::Rect::new(0, 0, 0, 0)
        }
    }

    /// `prefix a`: give the rail keyboard focus, or take it away. On a narrow
    /// terminal the rail is an overlay drawer; it never shrinks the panes.
    pub(super) fn toggle_sidebar(&mut self) {
        if self.ui.sidebar.open {
            self.close_sidebar();
            return;
        }
        self.clear_modes();
        let focused = self
            .workspace
            .focused_pane()
            .filter(|pane| self.agents.rows.iter().any(|row| row.pane == *pane));
        let selected = focused
            .map(super::super::sidebar::Item::Agent)
            .or_else(|| {
                self.agents
                    .rows
                    .first()
                    .map(|row| super::super::sidebar::Item::Agent(row.pane))
            })
            .or_else(|| {
                Some(super::super::sidebar::Item::Space(
                    self.workspace.active_space_id(),
                ))
            });
        self.ui.sidebar.selected = selected;
        self.ui.sidebar.offset = 0;
        self.ui.sidebar.open = true;
        self.keep_sidebar_visible();
        self.dirty = true;
    }

    pub(super) fn close_sidebar(&mut self) {
        if self.ui.sidebar.open {
            self.ui.sidebar.open = false;
            self.dirty = true;
        }
    }

    pub(super) fn sidebar_move(&mut self, delta: isize) {
        let items = super::super::sidebar::items(&self.workspace, &self.agents.rows);
        if items.is_empty() {
            self.ui.sidebar.selected = None;
            return;
        }
        let current = self
            .ui
            .sidebar
            .selected
            .and_then(|item| items.iter().position(|candidate| *candidate == item));
        let index = match (current, delta.signum()) {
            (Some(index), 1) => (index + 1).min(items.len() - 1),
            (Some(index), -1) => index.saturating_sub(1),
            (None, 1) => 0,
            (None, _) => items.len() - 1,
            _ => current.unwrap_or(0),
        };
        self.select_sidebar_item(index);
    }

    pub(super) fn sidebar_edge(&mut self, last: bool) {
        let items = super::super::sidebar::items(&self.workspace, &self.agents.rows);
        if items.is_empty() {
            self.ui.sidebar.selected = None;
            return;
        }
        self.select_sidebar_item(if last { items.len() - 1 } else { 0 });
    }

    fn select_sidebar_item(&mut self, index: usize) {
        let items = super::super::sidebar::items(&self.workspace, &self.agents.rows);
        self.ui.sidebar.selected = items.get(index).copied();
        self.keep_sidebar_visible();
        self.dirty = true;
    }

    fn keep_sidebar_visible(&mut self) {
        self.ui.sidebar.offset = super::super::sidebar::scroll_to_item(
            &self.workspace,
            &self.agents.rows,
            self.rail_rect(),
            self.ui.sidebar.offset,
            self.ui.sidebar.selected,
        );
    }

    /// Keeps the selection by item across rows reordering. A gone agent falls
    /// back to the row that took its index, then to the active space.
    fn keep_sidebar_selection(&mut self, rows: &[AgentRow]) {
        if self.ui.sidebar.selected.is_none() {
            return;
        }
        match self.ui.sidebar.selected {
            Some(super::super::sidebar::Item::Agent(pane)) => {
                let previous_index = self.agents.rows.iter().position(|row| row.pane == pane);
                let kept = agents::selection_after(rows, Some(pane), previous_index);
                self.ui.sidebar.selected =
                    kept.map(super::super::sidebar::Item::Agent).or_else(|| {
                        Some(super::super::sidebar::Item::Space(
                            self.workspace.active_space_id(),
                        ))
                    });
            }
            Some(super::super::sidebar::Item::Space(id)) if self.workspace.space(id).is_some() => {}
            _ => {
                self.ui.sidebar.selected = Some(super::super::sidebar::Item::Space(
                    self.workspace.active_space_id(),
                ));
            }
        }
        self.keep_sidebar_visible();
    }

    /// A click in the sidebar: a space row switches to that space (keeping
    /// its own active tab) and an agent row jumps to the pane, returning
    /// keyboard focus to it. Empty panel cells are consumed without action.
    pub(super) fn sidebar_click(&mut self, x: u16, y: u16) -> bool {
        let rail = self.rail_rect();
        if rail.width == 0 {
            return false;
        }
        let layout = super::super::sidebar::layout(
            &self.workspace,
            &self.agents.rows,
            rail,
            self.ui.sidebar.offset,
        );
        let Some(item) = super::super::sidebar::item_at(&layout, x, y) else {
            return false;
        };
        self.clear_modes();
        self.ui.sidebar.selected = Some(item);
        self.keep_sidebar_visible();
        match item {
            super::super::sidebar::Item::Space(id) => {
                if self.workspace.select_space_id(id) {
                    self.dirty = true;
                }
            }
            super::super::sidebar::Item::Agent(pane) => {
                self.jump_to_agent(pane);
                self.ui.sidebar.open = false;
                self.dirty = true;
            }
        }
        true
    }

    /// The agent row an action applies to: the sidebar selection while it is
    /// focused, otherwise the focused pane when it is an agent.
    fn action_row(&self) -> Option<AgentRow> {
        if self.ui.sidebar.open {
            let super::super::sidebar::Item::Agent(pane) = self.ui.sidebar.selected? else {
                return None;
            };
            return self
                .agents
                .rows
                .iter()
                .find(|row| row.pane == pane)
                .cloned();
        }
        let focused = self.workspace.focused_pane()?;
        self.agents
            .rows
            .iter()
            .find(|row| row.pane == focused)
            .cloned()
    }

    /// Enter in the sidebar: switch to a space or jump to an agent. A jump
    /// returns keyboard focus to the pane.
    pub(super) fn sidebar_enter(&mut self) -> Result<()> {
        match self.ui.sidebar.selected {
            Some(super::super::sidebar::Item::Space(id)) => {
                if self.workspace.select_space_id(id) {
                    self.dirty = true;
                }
                Ok(())
            }
            Some(super::super::sidebar::Item::Agent(pane)) => {
                self.jump_to_agent(pane);
                self.ui.sidebar.open = false;
                Ok(())
            }
            None => {
                self.ui.message = Some("nothing selected in the sidebar".into());
                self.dirty = true;
                Ok(())
            }
        }
    }

    /// Sidebar `i`: peek the selected agent, keeping the sidebar underneath.
    pub(super) fn sidebar_peek(&mut self) -> Result<()> {
        let Some(row) = self.action_row() else {
            self.ui.message = Some("no agent selected".into());
            self.dirty = true;
            return Ok(());
        };
        self.ui.peek = Some(row.pane);
        self.dirty = true;
        Ok(())
    }

    // --- peek and jump ----------------------------------------------------

    /// Prefix `i`: peek the focused agent.
    pub(super) fn peek_focused(&mut self) {
        let Some(pane) = self.workspace.focused_pane() else {
            self.ui.message = Some("no pane is focused".into());
            self.dirty = true;
            return;
        };
        if !self.agents.rows.iter().any(|row| row.pane == pane) {
            self.ui.message = Some("the focused pane is not an agent".into());
            self.dirty = true;
            return;
        }
        self.ui.peek = Some(pane);
        self.dirty = true;
    }

    /// Any key closes peek; an underlying sidebar keeps its selection.
    pub(super) fn close_peek(&mut self) {
        if self.ui.peek.take().is_some() {
            self.dirty = true;
        }
    }

    /// Prefix `g`/`G`: cycle through agents in sidebar order.
    pub(super) fn jump_agent(&mut self, jump: Jump) {
        let current = self.workspace.focused_pane();
        let Some(target) = agents::jump_target(&self.agents.rows, current, jump) else {
            self.ui.message = Some("no agents".into());
            self.dirty = true;
            return;
        };
        self.jump_to_agent(target);
    }

    fn jump_to_agent(&mut self, pane: PaneId) {
        if self.workspace.jump_to(pane) {
            self.ui.sidebar.selected = Some(super::super::sidebar::Item::Agent(pane));
            self.dirty = true;
        }
    }
}
