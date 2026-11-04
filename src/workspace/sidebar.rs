//! The persistent left rail: the spaces above the cross-space agent index.
//!
//! The rail belongs to the client, like the pane frames: it lists this
//! workspace's spaces and the agent rows derived from core. Its geometry is
//! pure so drawing, mouse hit testing and scrolling all agree. Rows have
//! variable height: a space may add a folder line, and every agent takes two
//! lines, so the model counts lines, not items.
//!
//! On a wide terminal the rail takes the left edge; on a narrow one it is
//! hidden and `prefix a` draws the same model as an overlay drawer.

use super::agents::{self, AgentRow};
use super::model::{PaneId, Space, SpaceId, Workspace};
use ratatui::layout::Rect;

/// The narrowest rail that still reads as a rail.
pub const MIN_WIDTH: u16 = 20;
/// The widest the rail may grow on a very wide terminal.
pub const MAX_WIDTH: u16 = 30;
/// Below this viewport width the rail is hidden by default (the drawer still
/// opens on demand).
pub const HIDE_BELOW: u16 = 90;
/// The pinned hint line at the rail's bottom.
pub const FOOTER: &str = "^b a navigate · Enter jump";

/// One selectable rail entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Space(SpaceId),
    Agent(PaneId),
}

/// One drawn line. Only `Space`, `SpaceFolder`, `Agent` and `AgentDetail` are
/// selectable; the rail's other lines are chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    SpacesHeader,
    Space(SpaceId),
    SpaceFolder(SpaceId),
    /// A dim rule between the spaces and the agent index.
    Divider,
    AgentHeader,
    Attention,
    Agent(PaneId),
    AgentDetail(PaneId),
    EmptyAgents,
    Footer,
}

/// One drawn line and its cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub rect: Rect,
}

/// The rail and the lines that fit right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The whole rail, including its right separator; zero when hidden.
    pub rect: Rect,
    /// The text area left of the separator.
    pub content: Rect,
    /// The pinned footer row, when the rail is tall enough to keep it.
    pub footer: Option<Rect>,
    /// The list lines that fit between the header and the footer.
    pub lines: Vec<Line>,
}

/// The rail width for a viewport: hidden below [`HIDE_BELOW`] columns, then a
/// quarter of the width clamped to [`MIN_WIDTH`]..=[`MAX_WIDTH`] (22 at 90,
/// 30 at 120).
pub fn width(viewport: Rect) -> u16 {
    if viewport.width < HIDE_BELOW {
        return 0;
    }
    (viewport.width / 4).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The rail rectangle for an area that already excludes the bottom line.
pub fn rect(viewport: Rect) -> Rect {
    let width = width(viewport);
    if width == 0 {
        return Rect::new(viewport.x, viewport.y, 0, 0);
    }
    Rect::new(viewport.x, viewport.y, width, viewport.height)
}

/// The narrow-mode drawer: the same rail drawn over the panes on demand. It is
/// an overlay, so it never shrinks or resizes them. It ends above the bottom
/// line so its pinned footer is visible instead of being painted over.
pub fn drawer_rect(viewport: Rect) -> Rect {
    let width = (viewport.width / 4)
        .clamp(MIN_WIDTH, MAX_WIDTH)
        .min(viewport.width);
    let height = viewport
        .height
        .saturating_sub(super::geometry::STATUS_HEIGHT);
    if width == 0 || height == 0 {
        return Rect::new(viewport.x, viewport.y, 0, 0);
    }
    Rect::new(viewport.x, viewport.y, width, height)
}

/// The space that holds `pane`, if any. The rail's second line names it; it
/// is looked up from the workspace, never remembered on the row.
pub fn space_of(workspace: &Workspace, pane: PaneId) -> Option<&Space> {
    workspace
        .spaces()
        .iter()
        .find(|space| space.tabs.iter().any(|tab| tab.contains(pane)))
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

/// Every line in the rail's list, in display order, before the optional
/// section divider. The footer is not part of the scrolled list.
fn base_kinds(workspace: &Workspace, agents: &[AgentRow]) -> Vec<LineKind> {
    let mut kinds = Vec::new();
    kinds.push(LineKind::SpacesHeader);
    for space in workspace.spaces() {
        kinds.push(LineKind::Space(space.id));
        if !space.folder.is_empty() {
            kinds.push(LineKind::SpaceFolder(space.id));
        }
    }
    kinds.push(LineKind::AgentHeader);
    if agents::attention(agents).summary().is_some() {
        kinds.push(LineKind::Attention);
    }
    if agents.is_empty() {
        kinds.push(LineKind::EmptyAgents);
    } else {
        for row in agents {
            kinds.push(LineKind::Agent(row.pane));
            kinds.push(LineKind::AgentDetail(row.pane));
        }
    }
    kinds
}

/// Puts the section rule directly before `AGENTS`; it is chrome, never a
/// selectable row.
fn with_divider(mut kinds: Vec<LineKind>) -> Vec<LineKind> {
    if let Some(index) = kinds.iter().position(|kind| *kind == LineKind::AgentHeader) {
        kinds.insert(index, LineKind::Divider);
    }
    kinds
}

/// The list with the divider only when it costs no necessary content: a rail
/// too short for every row drops the rule rather than an agent.
fn kinds(workspace: &Workspace, agents: &[AgentRow], height: usize) -> Vec<LineKind> {
    let base = base_kinds(workspace, agents);
    if base.len() < height {
        with_divider(base)
    } else {
        base
    }
}

/// The number of list lines, footer excluded, without the optional divider.
pub fn line_count(workspace: &Workspace, agents: &[AgentRow]) -> usize {
    base_kinds(workspace, agents).len()
}

/// How many list lines fit in the rail.
fn list_height(content: Rect, footer: bool) -> usize {
    (content.height as usize).saturating_sub(usize::from(footer))
}

/// The visible lines for `offset`, clipped to `rail`. The offset is a line
/// offset and is clamped so the list never scrolls past its last line.
pub fn layout(workspace: &Workspace, agents: &[AgentRow], rail: Rect, offset: usize) -> Layout {
    if rail.width == 0 || rail.height == 0 {
        return Layout::default();
    }
    let content = Rect::new(rail.x, rail.y, rail.width.saturating_sub(1), rail.height);
    // Keep the footer pinned only when at least two list rows remain.
    let has_footer = content.height >= 3;
    let footer = has_footer.then(|| {
        Rect::new(
            content.x,
            content.y.saturating_add(content.height.saturating_sub(1)),
            content.width,
            1,
        )
    });
    let height = list_height(content, has_footer);
    let kinds = kinds(workspace, agents, height);
    let offset = offset.min(kinds.len().saturating_sub(height));
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
        rect: rail,
        content,
        footer,
        lines,
    }
}

/// Where one item starts and how many lines it occupies.
pub fn item_span(workspace: &Workspace, agents: &[AgentRow], item: Item) -> Option<(usize, usize)> {
    match item {
        Item::Space(id) => {
            let mut start = 1;
            for space in workspace.spaces() {
                if space.id == id {
                    return Some((start, 1 + usize::from(!space.folder.is_empty())));
                }
                start += 1 + usize::from(!space.folder.is_empty());
            }
            None
        }
        Item::Agent(pane) => {
            let index = agents.iter().position(|row| row.pane == pane)?;
            let spaces: usize = workspace
                .spaces()
                .iter()
                .map(|space| 1 + usize::from(!space.folder.is_empty()))
                .sum();
            let attention = usize::from(agents::attention(agents).summary().is_some());
            Some((1 + spaces + 1 + attention + 2 * index, 2))
        }
    }
}

/// Where an item starts when the optional section rule is drawn. The rule
/// sits after the spaces, so only agent rows move down by one.
fn item_span_at(
    workspace: &Workspace,
    agents: &[AgentRow],
    item: Item,
    divider: bool,
) -> Option<(usize, usize)> {
    let (start, len) = item_span(workspace, agents, item)?;
    if divider && matches!(item, Item::Agent(_)) {
        Some((start + 1, len))
    } else {
        Some((start, len))
    }
}

/// The offset that keeps `selected`'s whole row visible, clamped to the list.
pub fn scroll_to_item(
    workspace: &Workspace,
    agents: &[AgentRow],
    rail: Rect,
    offset: usize,
    selected: Option<Item>,
) -> usize {
    if rail.width == 0 || rail.height == 0 {
        return 0;
    }
    let content = Rect::new(rail.x, rail.y, rail.width.saturating_sub(1), rail.height);
    let has_footer = content.height >= 3;
    let height = list_height(content, has_footer);
    let base = line_count(workspace, agents);
    let divider = base < height;
    let total = base + usize::from(divider);
    if height == 0 || total <= height {
        return 0;
    }
    let max = total - height;
    let mut offset = offset.min(max);
    if let Some((start, len)) =
        selected.and_then(|item| item_span_at(workspace, agents, item, divider))
    {
        if len >= height {
            offset = start.min(max);
        } else if start < offset {
            offset = start;
        } else if start + len > offset + height {
            offset = (start + len - height).min(max);
        }
    }
    offset.min(max)
}

/// The selectable item under a screen cell, when a line is actually drawn.
/// Either line of a space or agent row selects that row.
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
            LineKind::Space(id) | LineKind::SpaceFolder(id) => Some(Item::Space(id)),
            LineKind::Agent(pane) | LineKind::AgentDetail(pane) => Some(Item::Agent(pane)),
            LineKind::SpacesHeader
            | LineKind::Divider
            | LineKind::AgentHeader
            | LineKind::Attention
            | LineKind::EmptyAgents
            | LineKind::Footer => None,
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
            seen: word != "done",
            seen_turn: Some(1),
            status: None,
            foreground: None,
            state: "live".into(),
            exit: None,
            order: (0, 0, 0),
        }
    }

    #[test]
    fn width_is_hidden_below_ninety_columns_and_clamped_above() {
        assert_eq!(width(Rect::new(0, 0, 89, 24)), 0);
        assert_eq!(width(Rect::new(0, 0, 90, 24)), 22);
        assert_eq!(width(Rect::new(0, 0, 110, 24)), 27);
        assert_eq!(width(Rect::new(0, 0, 120, 24)), 30);
        assert_eq!(width(Rect::new(0, 0, 200, 24)), 30);
        assert_eq!(rect(Rect::new(3, 2, 120, 10)), Rect::new(3, 2, 30, 10));
        assert_eq!(rect(Rect::new(3, 2, 80, 10)), Rect::new(3, 2, 0, 0));
        assert_eq!(
            drawer_rect(Rect::new(0, 0, 80, 10)),
            Rect::new(0, 0, 20, 9),
            "the narrow drawer uses the quarter-width rule and ends above the status row"
        );
        assert_eq!(
            drawer_rect(Rect::new(0, 0, 80, 1)),
            Rect::new(0, 0, 0, 0),
            "no status row leaves no room for a drawer"
        );
    }

    #[test]
    fn the_layout_lists_spaces_with_folders_then_agents_with_two_lines_each() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        add(&mut workspace, StartKind::Space, "t2");
        let first_space = workspace.spaces()[0].id;
        workspace.set_space_folder(first_space, "/work/herdr");
        let mut agents = vec![row(first, "Build", "working", false)];
        agents::sort_rows(&mut agents);
        let viewport = Rect::new(0, 0, 24, 14);
        let panel = layout(&workspace, &agents, viewport, 0);
        assert_eq!(panel.rect, Rect::new(0, 0, 24, 14));
        assert_eq!(panel.content, Rect::new(0, 0, 23, 14));
        let kinds: Vec<LineKind> = panel.lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::SpacesHeader,
                LineKind::Space(first_space),
                LineKind::SpaceFolder(first_space),
                LineKind::Space(workspace.spaces()[1].id),
                LineKind::Divider,
                LineKind::AgentHeader,
                LineKind::Agent(first),
                LineKind::AgentDetail(first),
            ]
        );
        assert_eq!(panel.lines[6].rect, Rect::new(0, 6, 23, 1));
        assert_eq!(line_count(&workspace, &agents), 7);
        assert_eq!(
            item_span(&workspace, &agents, Item::Space(first_space)),
            Some((1, 2))
        );
        assert_eq!(
            item_span(&workspace, &agents, Item::Agent(first)),
            Some((5, 2))
        );
        assert_eq!(
            items(&workspace, &agents),
            vec![
                Item::Space(first_space),
                Item::Space(workspace.spaces()[1].id),
                Item::Agent(first),
            ]
        );
        let footer = panel.footer.expect("a fourteen-row rail keeps its footer");
        assert_eq!(footer.y, 13);
        assert_eq!(footer.height, 1);

        let empty = layout(&workspace, &[], viewport, 0);
        assert_eq!(empty.lines[6].kind, LineKind::EmptyAgents);
    }

    #[test]
    fn the_section_rule_yields_to_necessary_rows_on_a_short_rail() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        let agents = vec![row(first, "Build", "idle", false)];

        let tall = layout(&workspace, &agents, Rect::new(0, 0, 24, 14), 0);
        let divider = tall
            .lines
            .iter()
            .position(|line| line.kind == LineKind::Divider)
            .expect("a tall rail draws the section rule");
        assert_eq!(
            tall.lines[divider + 1].kind,
            LineKind::AgentHeader,
            "the rule sits directly above AGENTS"
        );

        // Six rows: header, space, AGENTS, the agent's two lines, and the
        // footer. Adding a rule would displace the detail line, so it is
        // dropped instead.
        let short = layout(&workspace, &agents, Rect::new(0, 0, 24, 6), 0);
        assert!(
            !short
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Divider),
            "the rule displaced a necessary row: {short:?}"
        );
        assert!(
            short
                .lines
                .iter()
                .any(|line| line.kind == LineKind::AgentDetail(first))
        );
    }

    #[test]
    fn attention_adds_one_summary_line_and_the_offset_is_clamped() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        let mut agents = vec![
            row(first, "Build", "blocked", false),
            row(PaneId(9), "Review", "done", true),
        ];
        agents::sort_rows(&mut agents);
        let panel = layout(&workspace, &agents, Rect::new(0, 0, 24, 6), 0);
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Attention)
        );
        // Header, space, agents header, attention, then the first agent row.
        let total = line_count(&workspace, &agents);
        let clamped = layout(&workspace, &agents, Rect::new(0, 0, 24, 6), 99);
        assert_eq!(
            clamped.lines.last().unwrap().kind,
            LineKind::AgentDetail(first)
        );
        assert_eq!(clamped.lines.len(), 6 - 1, "footer takes one row");
        assert_eq!(clamped.lines[0].rect.y, 0);
        assert_eq!(total, 8);
    }

    #[test]
    fn item_at_maps_both_lines_of_a_row_and_ignores_chrome() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        add(&mut workspace, StartKind::Space, "t2");
        let first_space = workspace.spaces()[0].id;
        workspace.set_space_folder(first_space, "folder");
        let agents = vec![row(first, "Build", "blocked", true)];
        let panel = layout(&workspace, &agents, Rect::new(0, 0, 24, 16), 0);
        assert_eq!(item_at(&panel, 5, 1), Some(Item::Space(first_space)));
        assert_eq!(item_at(&panel, 5, 2), Some(Item::Space(first_space)));
        assert_eq!(
            item_at(&panel, 5, 3),
            Some(Item::Space(workspace.spaces()[1].id))
        );
        assert_eq!(item_at(&panel, 5, 4), None, "the section rule");
        assert_eq!(item_at(&panel, 5, 5), None, "agents header");
        assert_eq!(item_at(&panel, 5, 6), None, "attention summary");
        assert_eq!(item_at(&panel, 5, 7), Some(Item::Agent(first)));
        assert_eq!(item_at(&panel, 5, 8), Some(Item::Agent(first)));
        assert_eq!(item_at(&panel, 23, 7), None, "the separator column");
        assert_eq!(item_at(&panel, 5, 15), None, "the pinned footer");
    }

    #[test]
    fn narrow_viewports_draw_nothing_and_scrolling_keeps_the_row_visible() {
        let mut workspace = Workspace::new();
        let first = add(&mut workspace, StartKind::Initial, "t1");
        add(&mut workspace, StartKind::Space, "t2");
        let second_space = workspace.spaces()[1].id;
        let agents = vec![row(first, "Build", "idle", false)];
        let viewport = Rect::new(0, 0, 0, 0);
        let panel = layout(&workspace, &agents, viewport, 0);
        assert_eq!(panel, Layout::default());
        assert_eq!(item_at(&panel, 0, 0), None);

        let tall = Rect::new(0, 0, 24, 5);
        let offset = scroll_to_item(&workspace, &agents, tall, 0, Some(Item::Agent(first)));
        // Four list rows: the agent's two lines must both be visible, so the
        // offset moves to its first line.
        let visible = layout(&workspace, &agents, tall, offset);
        assert!(
            visible
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Agent(first))
        );
        assert!(
            visible
                .lines
                .iter()
                .any(|line| line.kind == LineKind::AgentDetail(first))
        );
        let offset = scroll_to_item(
            &workspace,
            &agents,
            tall,
            99,
            Some(Item::Space(second_space)),
        );
        assert_eq!(offset, 2, "the offset moves just enough to show the space");
        let visible = layout(&workspace, &agents, tall, offset);
        assert!(
            visible
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Space(second_space))
        );
    }
}
