//! Commands: the semantic layer between keys and the pure workspace.
//!
//! A command mutates the workspace and returns an [`Effect`] for the runtime:
//! a terminal to start, terminals to detach or end, a prompt to open, or a
//! detach of the whole client. Key bindings live in [`super::keys`], so M6 can
//! add a palette and configurable bindings without touching these semantics.

use super::agents::Jump;
use super::model::{
    Axis, CloseKind, ClosedPane, Direction, SpaceId, StartKind, StartRequest, TabId, Workspace,
    close_action,
};
use ratatui::layout::Rect;

/// What a key (or a future palette) asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    NewTab,
    NewSpace,
    NextTab,
    PrevTab,
    NextSpace,
    PrevSpace,
    SelectTab(u8),
    Split(Axis),
    Focus(Direction),
    Move(Direction),
    Zoom,
    Close(CloseKind),
    RenameTab,
    RenameSpace,
    /// Toggle the persistent agent sidebar's keyboard focus.
    Agents,
    /// Enter resize mode for the active tab's split boundaries.
    Resize,
    /// Open the Core-wide terminal manager.
    Terminals,
    /// Open the machine manager.
    Machines,
    /// Open the Core-backed file viewer.
    Files,
    /// Open the on-demand tab selector for the active space.
    TabSelector,
    /// Peek the focused (or sidebar-selected) agent.
    Peek,
    /// Jump to the next or previous agent in sidebar order.
    JumpAgent(Jump),
    /// Open the command palette.
    Palette,
    /// Search the focused pane's local scrollback.
    Search,
    /// Enter or leave mouse selection mode.
    Select,
    /// Copy the focused pane's current selection.
    Copy,
    Detach,
}

/// The name a rename prompt edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Tab { space: SpaceId, tab: TabId },
    Space(SpaceId),
}

/// What the runtime must do after a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The command did nothing (for example a close with no pane).
    None,
    /// The workspace view changed; redraw and recompute pane sizes.
    Changed,
    /// Run a terminal, insert it, then attach it.
    Start(StartRequest),
    /// Remove these panes' terminals from core (detach or end).
    Close(Vec<ClosedPane>),
    /// Open the one-line rename prompt.
    Rename(RenameTarget),
    /// Open the help overlay.
    Help,
    /// Toggle the agent sidebar focus; the runtime selects a row.
    Agents,
    /// Enter resize mode.
    Resize,
    /// Open the terminal manager; the runtime selects a row.
    Terminals,
    /// Open the machine manager; the runtime lists core's machines.
    Machines,
    /// Open the file overlay for the focused pane's machine and cwd.
    Files,
    /// Open the tab selector; the runtime lists the active space's tabs.
    TabSelector,
    /// Open the peek overlay for the focused or selected agent.
    Peek,
    /// Jump to the next or previous agent.
    JumpAgent(Jump),
    /// Open the command palette.
    Palette,
    /// Open the scrollback search bar for the focused pane.
    Search,
    /// Toggle mouse selection mode.
    Select,
    /// Copy the current local selection.
    Copy,
    /// Close every attach stream and leave the client.
    Detach,
}

impl Workspace {
    /// Applies a command. Pure: no terminal, core or clock is touched.
    pub fn dispatch(&mut self, command: Command, viewport: Rect) -> Effect {
        match command {
            Command::Help => Effect::Help,
            Command::NewTab => Effect::Start(self.start_request(StartKind::Tab)),
            Command::NewSpace => Effect::Start(self.start_request(StartKind::Space)),
            Command::NextTab => changed(self.next_tab()),
            Command::PrevTab => changed(self.prev_tab()),
            Command::NextSpace => changed(self.next_space()),
            Command::PrevSpace => changed(self.prev_space()),
            Command::SelectTab(tab) => changed(self.select_tab(tab)),
            Command::Split(axis) => Effect::Start(self.start_request(StartKind::Split(axis))),
            Command::Focus(direction) => changed(self.focus_direction(viewport, direction)),
            Command::Move(direction) => changed(self.move_direction(viewport, direction)),
            Command::Zoom => changed(self.toggle_zoom()),
            Command::Close(kind) => self.close_focused(kind),
            Command::RenameTab => {
                let space = self.active_space_id();
                match self.active_tab_id() {
                    Some(tab) => Effect::Rename(RenameTarget::Tab { space, tab }),
                    None => Effect::None,
                }
            }
            Command::RenameSpace => Effect::Rename(RenameTarget::Space(self.active_space_id())),
            Command::Agents => Effect::Agents,
            Command::Resize => Effect::Resize,
            Command::Terminals => Effect::Terminals,
            Command::Machines => Effect::Machines,
            Command::Files => Effect::Files,
            Command::TabSelector => Effect::TabSelector,
            Command::Peek => Effect::Peek,
            Command::JumpAgent(jump) => Effect::JumpAgent(jump),
            Command::Palette => Effect::Palette,
            Command::Search => Effect::Search,
            Command::Select => Effect::Select,
            Command::Copy => Effect::Copy,
            Command::Detach => Effect::Detach,
        }
    }

    fn close_focused(&mut self, kind: CloseKind) -> Effect {
        let Some(pane) = self.focused_pane() else {
            return Effect::None;
        };
        let Some(record) = self.pane(pane) else {
            return Effect::None;
        };
        let action = close_action(record, kind);
        let address = record.address.clone();
        self.remove_pane(pane);
        Effect::Close(vec![ClosedPane {
            pane,
            address,
            action,
        }])
    }
}

fn changed(changed: bool) -> Effect {
    if changed {
        Effect::Changed
    } else {
        Effect::None
    }
}

#[cfg(test)]
mod tests;
