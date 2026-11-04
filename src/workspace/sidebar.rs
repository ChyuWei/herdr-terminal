//! The persistent left sidebar: its width, its lines and its hit testing.
//!
//! The sidebar belongs to the client, like the tab bar: it lists this
//! workspace's spaces and the agent rows derived from core. Its geometry is
//! pure so drawing, mouse hit testing and scrolling all agree.

use super::agents::AgentRow;
use super::model::{PaneId, SpaceId, Workspace};
use ratatui::layout::Rect;

/// The narrowest panel that still reads as a sidebar.
pub const MIN_WIDTH: u16 = 18;
/// The widest the panel may grow on a very wide terminal.
pub const MAX_WIDTH: u16 = 28;
/// Below this viewport width the sidebar is hidden entirely.
pub const HIDE_BELOW: u16 = 70;

/// One selectable sidebar entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Space(SpaceId),
    Agent(PaneId),
}

/// One drawn line. Section headers and the empty-agents note are not
/// selectable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    SpacesHeader,
    AgentHeader,
    Space(SpaceId),
    Agent(PaneId),
    EmptyAgents,
}

/// One drawn line and its cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub rect: Rect,
}

/// The panel and the lines that fit right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The whole panel, including its right separator; zero when hidden.
    pub rect: Rect,
    /// The text area left of the separator.
    pub content: Rect,
    pub lines: Vec<Line>,
}

/// The panel width for a viewport: hidden below [`HIDE_BELOW`] columns, then a
/// quarter of the width clamped to [`MIN_WIDTH`]..=[`MAX_WIDTH`] (24 columns
/// at 96).
pub fn width(viewport: Rect) -> u16 {
    if viewport.width < HIDE_BELOW {
        return 0;
    }
    (viewport.width / 4).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The full-height panel rectangle; zero-sized when hidden.
pub fn rect(viewport: Rect) -> Rect {
    let width = width(viewport);
    if width == 0 {
        return Rect::new(viewport.x, viewport.y, 0, 0);
    }
    Rect::new(viewport.x, viewport.y, width, viewport.height)
}

/// How many lines fit in the panel.
pub fn height(viewport: Rect) -> usize {
    rect(viewport).height as usize
}

/// Every selectable item, in display order: the spaces, then the agents.
pub fn items(workspace: &Workspace, agents: &[AgentRow]) -> Vec<Item> {
    workspace
        .spaces()
        .iter()
        .map(|space| Item::Space(space.id))
        .chain(agents.iter().map(|row| Item::Agent(row.pane)))
        .collect()
}

/// The line index of one item, matching [`layout`].
pub fn line_index(workspace: &Workspace, agents: &[AgentRow], item: Item) -> Option<usize> {
    match item {
        Item::Space(id) => workspace
            .spaces()
            .iter()
            .position(|space| space.id == id)
            .map(|index| 1 + index),
        Item::Agent(pane) => agents
            .iter()
            .position(|row| row.pane == pane)
            .map(|index| 1 + workspace.spaces().len() + 1 + index),
    }
}

/// The number of drawn lines, including headers.
pub fn line_count(workspace: &Workspace, agents: &[AgentRow]) -> usize {
    let empty = usize::from(agents.is_empty());
    1 + workspace.spaces().len() + 1 + agents.len() + empty
}

/// The visible lines for `offset`, already clipped to the panel. The offset is
/// clamped so the list never scrolls past its last line.
pub fn layout(workspace: &Workspace, agents: &[AgentRow], viewport: Rect, offset: usize) -> Layout {
    let rect = rect(viewport);
    if rect.width == 0 || rect.height == 0 {
        return Layout::default();
    }
    let content = Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), rect.height);
    let height = rect.height as usize;
    let total = line_count(workspace, agents);
    let offset = offset.min(total.saturating_sub(height));
    let mut kinds = Vec::with_capacity(total);
    kinds.push(LineKind::SpacesHeader);
    kinds.extend(
        workspace
            .spaces()
            .iter()
            .map(|space| LineKind::Space(space.id)),
    );
    kinds.push(LineKind::AgentHeader);
    if agents.is_empty() {
        kinds.push(LineKind::EmptyAgents);
    } else {
        kinds.extend(agents.iter().map(|row| LineKind::Agent(row.pane)));
    }
    let lines = kinds
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, kind)| Line {
            kind,
            rect: Rect::new(
                content.x,
                content.y.saturating_add((index - offset) as u16),
                content.width,
                1,
            ),
        })
        .collect();
    Layout {
        rect,
        content,
        lines,
    }
}

/// The selectable item under a screen cell, when a line is actually drawn.
pub fn item_at(layout: &Layout, x: u16, y: u16) -> Option<Item> {
    layout.lines.iter().find_map(|line| {
        let rect = line.rect;
        let inside = x >= rect.x
            && x < rect.x.saturating_add(rect.width)
            && y >= rect.y
            && y < rect.y.saturating_add(rect.height);
        if !inside {
            return None;
        }
        match line.kind {
            LineKind::Space(id) => Some(Item::Space(id)),
            LineKind::Agent(pane) => Some(Item::Agent(pane)),
            LineKind::SpacesHeader | LineKind::AgentHeader | LineKind::EmptyAgents => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::agents::Priority;
    use crate::workspace::model::StartKind;

    fn add(workspace: &mut Workspace, kind: StartKind, name: &str) -> PaneId {
        let request = workspace.start_request(kind);
        workspace.insert_terminal(&request, name.to_owned(), name.to_owned())
    }

    fn row(pane: PaneId, title: &str, word: &str, pinned: bool) -> AgentRow {
        AgentRow {
            pane,
            address: format!("t{}", pane.0),
            title: title.to_owned(),
            name: format!("t{}", pane.0),
            word: word.to_owned(),
            priority: Priority::Idle,
            turn: 1,
            pinned,
            seen: true,
            seen_turn: Some(1),
            status: None,
            foreground: None,
            state: "live".into(),
            exit: None,
            order: (0, 0, 0),
        }
    }

    #[test]
    fn width_is_hidden_below_seventy_columns_and_clamped_above() {
        assert_eq!(width(Rect::new(0, 0, 69, 24)), 0);
        assert_eq!(width(Rect::new(0, 0, 70, 24)), 18);
        assert_eq!(width(Rect::new(0, 0, 80, 24)), 20);
        assert_eq!(width(Rect::new(0, 0, 96, 24)), 24);
        assert_eq!(width(Rect::new(0, 0, 200, 24)), 28);
        assert_eq!(rect(Rect::new(3, 2, 80, 10)), Rect::new(3, 2, 20, 10));
        assert_eq!(rect(Rect::new(3, 2, 60, 10)), Rect::new(3, 2, 0, 0));
    }

    #[test]
    fn the_layout_lists_spaces_then_agents_and_clips_to_the_panel() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        add(&mut workspace, StartKind::Space, "t2");
        let agents = vec![row(first, "Build", "working", false)];
        let viewport = Rect::new(0, 0, 80, 6);
        let panel = layout(&workspace, &agents, viewport, 0);
        assert_eq!(panel.rect, Rect::new(0, 0, 20, 6));
        assert_eq!(panel.content, Rect::new(0, 0, 19, 6));
        let kinds: Vec<LineKind> = panel.lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::SpacesHeader,
                LineKind::Space(workspace.spaces()[0].id),
                LineKind::Space(workspace.spaces()[1].id),
                LineKind::AgentHeader,
                LineKind::Agent(first),
            ]
        );
        assert_eq!(panel.lines[4].rect, Rect::new(0, 4, 19, 1));
        assert_eq!(
            line_count(&workspace, &agents),
            5,
            "space header + two spaces + agent header + one agent"
        );
        assert_eq!(
            line_index(&workspace, &agents, Item::Space(workspace.spaces()[1].id)),
            Some(2)
        );
        assert_eq!(line_index(&workspace, &agents, Item::Agent(first)), Some(4));
        assert_eq!(
            line_index(&workspace, &agents, Item::Agent(PaneId(99))),
            None
        );
        assert_eq!(
            items(&workspace, &agents),
            vec![
                Item::Space(workspace.spaces()[0].id),
                Item::Space(workspace.spaces()[1].id),
                Item::Agent(first),
            ]
        );

        let empty = layout(&workspace, &[], viewport, 0);
        assert_eq!(empty.lines[4].kind, LineKind::EmptyAgents);
    }

    #[test]
    fn empty_agents_get_one_note_line_and_the_offset_is_clamped() {
        let mut workspace = Workspace::new();
        add(&mut workspace, StartKind::Initial, "t1");
        let viewport = Rect::new(0, 0, 80, 3);
        let panel = layout(&workspace, &[], viewport, 99);
        // Four lines (header, space, agent header, note) in a three-line panel:
        // the offset clamps to one so the last three lines are shown.
        let kinds: Vec<LineKind> = panel.lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Space(workspace.spaces()[0].id),
                LineKind::AgentHeader,
                LineKind::EmptyAgents
            ]
        );
        assert_eq!(panel.lines[0].rect.y, 0);
    }

    #[test]
    fn item_at_maps_drawn_rows_and_ignores_headers() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        add(&mut workspace, StartKind::Space, "t2");
        let agents = vec![row(first, "Build", "blocked", true)];
        let viewport = Rect::new(0, 0, 80, 12);
        let panel = layout(&workspace, &agents, viewport, 0);
        let spaces = workspace.spaces();
        assert_eq!(item_at(&panel, 5, 1), Some(Item::Space(spaces[0].id)));
        assert_eq!(item_at(&panel, 5, 2), Some(Item::Space(spaces[1].id)));
        assert_eq!(item_at(&panel, 5, 3), None, "agents header");
        assert_eq!(item_at(&panel, 5, 4), Some(Item::Agent(first)));
        assert_eq!(item_at(&panel, 19, 4), None, "the separator column");
        assert_eq!(item_at(&panel, 5, 9), None, "below the last line");
    }

    #[test]
    fn narrow_viewports_draw_nothing_and_hit_nothing() {
        let mut workspace = Workspace::new();
        let pane = add(&mut workspace, StartKind::Initial, "t1");
        let agents = vec![row(pane, "Build", "idle", false)];
        let viewport = Rect::new(0, 0, 69, 12);
        let panel = layout(&workspace, &agents, viewport, 0);
        assert_eq!(panel, Layout::default());
        assert_eq!(item_at(&panel, 0, 0), None);
        assert_eq!(height(viewport), 0);
    }
}
