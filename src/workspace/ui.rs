//! Composing the workspace into a ratatui buffer.
//!
//! Pure presentation: the runtime supplies one VT snapshot per visible pane.
//! The tab bar sits on the first row, the status bar on the last, and panes
//! get the area between. Borders use the pane rectangle; the terminal grid
//! uses its inner rectangle. No core, VT or terminal is touched here.

use super::agents::{self, AgentRow};
use super::file::{DiffTag, Pane};
use super::geometry::{self, Rects};
mod machine_ui;

pub use machine_ui::machine_list_height;

use super::machines::{self, MachineRow};
use super::manager::{self, ManagerRow};
use super::model::{PaneId, Workspace};
use super::palette;
use super::state::{Connection, PreviewState, UiState};
use crate::theme::Theme;
use crate::vt::Screen as VtScreen;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use std::collections::BTreeMap;

/// The help text. It is also the golden-tested documented key map.
pub const HELP: &[&str] = &[
    "prefix Ctrl-b      ? help          q detach   any key closes help",
    "c new tab          C new space",
    "n / p tab          N / P space     1..9 select tab",
    "v split left/right s split top/bottom",
    "h/j/k/l focus      H/J/K/L move",
    "z zoom             x close",
    "e keep + close     X kill + close",
    ", rename tab       . rename space",
    "a agents           i peek agent    g/G next/prev agent",
    "t terminal manager (all live terminals in core)",
    "manager: j/k move  g/G first/last  Enter show/hide",
    "/ filter  p preview (read-only)  Esc close manager",
    "m machine manager · Enter connect · a add · r rename · d remove",
    "machines: n new terminal there · / filter · Esc close · add takes target only",
    "o file viewer       e edit in $VISUAL/$EDITOR, diff before write",
    "file: Enter read · e edit · w write · q discard · Esc close",
    "sidebar: j/k move  g/G first/last  Enter jump",
    "p pin / unpin  u mark unread  i peek  Esc/a close  any key closes peek",
    "Ctrl-] detach      prefix twice sends Ctrl-b",
    ": palette          f search scrollback",
    "[ select mode      y copy selection",
    "selection: drag or h/j/k/l, y copy, Esc cancel",
    "other keys leave selection; paste is ignored while selecting",
    "Shift-PgUp/PgDn scroll history      Shift-Home/End top/bottom",
    "drag a split gap to resize          wheel scrolls local history",
    "mouse: click focuses pane/tab       drag selects; Shift-drag local",
];

/// Where the cursor belongs after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Render {
    /// The focused pane and its terminal grid, when it is visible.
    pub focused: Option<(PaneId, Rect)>,
    /// The rename prompt, palette or search query cursor, when one is open.
    pub prompt_cursor: Option<(u16, u16)>,
}

/// Viewport-relative selection highlight spans `(x, y, width)` per pane.
pub type SelectionSpans = BTreeMap<PaneId, Vec<(u16, u16, u16)>>;

/// Everything a frame needs beyond the workspace and UI state: local VT
/// snapshots, agent rows, the theme, local selection highlights and the
/// scroll-position hint. Grouping them keeps the drawing signatures stable as
/// the daily-driver surface grows.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub screens: &'a BTreeMap<PaneId, VtScreen>,
    pub rows: &'a [AgentRow],
    pub theme: &'a Theme,
    pub selections: &'a SelectionSpans,
    pub scroll_hint: Option<&'a str>,
    /// Every live terminal in core, while the manager is open.
    pub manager: &'a [ManagerRow],
    /// Core's machines, while the machine manager is open.
    pub machines: &'a [MachineRow],
    /// The read-only observer preview's local VT snapshot.
    pub preview: Option<&'a VtScreen>,
    /// The file overlay's viewer or pending diff.
    pub files: Option<&'a Pane>,
}

impl Default for View<'_> {
    fn default() -> View<'static> {
        static SCREENS: std::sync::OnceLock<BTreeMap<PaneId, VtScreen>> =
            std::sync::OnceLock::new();
        static ROWS: [AgentRow; 0] = [];
        static MANAGER_ROWS: [ManagerRow; 0] = [];
        static MACHINE_ROWS: [MachineRow; 0] = [];
        static SELECTIONS: std::sync::OnceLock<SelectionSpans> = std::sync::OnceLock::new();
        static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        View {
            screens: SCREENS.get_or_init(BTreeMap::new),
            rows: &ROWS,
            theme: THEME.get_or_init(Theme::default),
            selections: SELECTIONS.get_or_init(BTreeMap::new),
            scroll_hint: None,
            manager: &MANAGER_ROWS,
            machines: &MACHINE_ROWS,
            preview: None,
            files: None,
        }
    }
}

/// Paints a frame with default settings, for callers that only need the
/// workspace and the terminal snapshots.
pub fn render(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &Workspace,
    ui: &UiState,
    screens: &BTreeMap<PaneId, VtScreen>,
) -> Render {
    let view = View {
        screens,
        ..View::default()
    };
    render_with(buffer, area, workspace, ui, &view)
}

/// Paints a frame.
pub fn render_with(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &Workspace,
    ui: &UiState,
    view: &View,
) -> Render {
    let theme = view.theme;
    let rects = geometry::compute(workspace.active_tab(), area);
    draw_panes(
        buffer,
        workspace,
        &rects,
        view.screens,
        view.selections,
        theme,
    );
    draw_tab_bar(buffer, workspace, rects.tab_bar, theme);
    let mut rendered = Render {
        focused: workspace.focused_pane().and_then(|id| {
            rects
                .pane(id)
                .and_then(geometry::inner)
                .map(|inner| (id, inner))
        }),
        prompt_cursor: None,
    };
    if rects.panes.is_empty() && area.height > 0 {
        draw_empty(buffer, rects.workspace);
    }
    rendered.prompt_cursor = draw_status(buffer, workspace, ui, rects.status, theme, view);
    if ui.search.is_some() {
        // The search bar owns the status row; the program's cursor would
        // suggest the keys are still typing into it.
        rendered.focused = None;
    }
    if ui.sidebar.open {
        draw_sidebar(buffer, area, ui, view.rows, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.peek.is_some() {
        draw_peek(buffer, area, ui, view.screens, view.rows, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.manager.open {
        rendered.prompt_cursor = draw_manager(buffer, area, ui, view.manager, theme);
        rendered.focused = None;
    }
    if ui.machines.open {
        rendered.prompt_cursor = machine_ui::draw_machines(buffer, area, ui, view.machines, theme);
        rendered.focused = None;
    }
    if ui.files.open {
        rendered.prompt_cursor = draw_file(buffer, area, ui, view.files, theme);
        rendered.focused = None;
    }
    if let Some(preview) = &ui.preview {
        draw_preview(buffer, area, preview, view.preview, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.palette.is_some() {
        rendered.prompt_cursor = draw_palette(buffer, area, ui, theme);
        rendered.focused = None;
    }
    if ui.help {
        draw_help(buffer, area, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    rendered
}

/// The rectangle the agent sidebar occupies. Shared with the runtime so its
/// scroll offset uses the same visible height as the drawing.
pub fn sidebar_rect(area: Rect) -> Rect {
    if area.width < 8 || area.height < 5 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = area.width.saturating_sub(4).min(88);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    )
}

/// How many agent rows fit inside the sidebar.
pub fn sidebar_inner_height(area: Rect) -> usize {
    sidebar_rect(area).height.saturating_sub(2) as usize
}

/// The terminal manager's overlay rectangle: a wide, tall panel. Each row is
/// fitted to the list width (`manager::row_line`), so a narrow terminal keeps
/// the marker, status word and address while secondary facts are dropped.
pub fn manager_rect(area: Rect) -> Rect {
    if area.width < 8 || area.height < 5 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    )
}

/// How many inventory rows fit under the manager's filter/count line.
pub fn manager_list_height(area: Rect) -> usize {
    manager_rect(area)
        .height
        .saturating_sub(2)
        .saturating_sub(1) as usize
}

/// The preview overlay's rectangle, shared by drawing and the observer's
/// initial attach size.
pub fn preview_rect(area: Rect) -> Rect {
    if area.width < 8 || area.height < 5 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = area.width.saturating_sub(4).min(96);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    )
}

/// The preview's terminal grid: inside the border, below the facts line.
pub fn preview_grid(area: Rect) -> Rect {
    let rect = preview_rect(area);
    let inner = Block::new().borders(Borders::ALL).inner(rect);
    if inner.width == 0 || inner.height <= 1 {
        return Rect::new(inner.x, inner.y, 0, 0);
    }
    Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    )
}

/// The observer attach size for the preview.
pub fn preview_grid_size(area: Rect) -> (u16, u16) {
    let grid = preview_grid(area);
    (grid.width, grid.height)
}

fn draw_tab_bar(buffer: &mut Buffer, workspace: &Workspace, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let space = workspace.active_space();
    let mut spans = vec![Span::styled(
        format!(" {} ", space.name),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    for (index, tab) in space.tabs.iter().enumerate() {
        let style = if tab.id == space.active {
            theme.tab_active
        } else {
            theme.tab_inactive
        };
        spans.push(Span::styled(format!(" {} {} ", index + 1, tab.name), style));
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn draw_panes(
    buffer: &mut Buffer,
    workspace: &Workspace,
    rects: &Rects,
    screens: &BTreeMap<PaneId, VtScreen>,
    selections: &BTreeMap<PaneId, Vec<(u16, u16, u16)>>,
    theme: &Theme,
) {
    let focused = workspace.focused_pane();
    for (pane_id, rect) in &rects.panes {
        let Some(pane) = workspace.pane(*pane_id) else {
            continue;
        };
        let is_focused = focused == Some(*pane_id);
        let border = if is_focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.border)
        };
        let mut label = machines::qualified_name(
            &pane.address,
            &if pane.title.is_empty() || pane.title == pane.name {
                pane.name.clone()
            } else {
                format!("{}: {}", pane.name, pane.title)
            },
        );
        if let Some(code) = pane.exit {
            label.push_str(&format!(" [exited {code}]"));
        }
        Block::new()
            .borders(Borders::ALL)
            .border_style(border)
            .title(Line::from(Span::styled(format!(" {label} "), border)))
            .render(*rect, buffer);
        let Some(inner) = geometry::inner(*rect) else {
            continue;
        };
        if let Some(screen) = screens.get(pane_id) {
            crate::screen::view::draw_buffer(buffer, inner, screen);
        }
        if let Some(spans) = selections.get(pane_id) {
            draw_selection(buffer, inner, spans, theme);
        }
    }
}

/// Patches the selection color over the terminal cells the local selection
/// covers.
fn draw_selection(buffer: &mut Buffer, inner: Rect, spans: &[(u16, u16, u16)], theme: &Theme) {
    let style = Style::default().bg(theme.selection);
    for &(x, y, width) in spans {
        if y >= inner.height {
            continue;
        }
        let end = x.saturating_add(width).min(inner.width);
        for column in x..end {
            let at = (inner.x.saturating_add(column), inner.y.saturating_add(y));
            if let Some(cell) = buffer.cell_mut(at) {
                cell.set_style(style);
            }
        }
    }
}

fn draw_empty(buffer: &mut Buffer, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = [
        Line::from(Span::styled(
            "no terminals in this tab",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("prefix v: new pane · prefix c: new tab · prefix ?: help"),
    ];
    let height = (area.height as usize).min(lines.len());
    let start = area.height / 2 - (height as u16).saturating_sub(1) / 2;
    let at = Rect::new(
        area.x,
        area.y.saturating_add(start.min(area.height - 1)),
        area.width,
        height as u16,
    );
    Paragraph::new(lines[..height].to_vec())
        .alignment(Alignment::Center)
        .render(at, buffer);
}

/// Draws the status bar (or the rename prompt or search bar in its place) and
/// returns the field cursor.
fn draw_status(
    buffer: &mut Buffer,
    workspace: &Workspace,
    ui: &UiState,
    area: Rect,
    theme: &Theme,
    view: &View<'_>,
) -> Option<(u16, u16)> {
    let rows = view.rows;
    let scroll_hint = view.scroll_hint;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    if let Some(search) = &ui.search {
        return draw_search(buffer, area, search, theme);
    }
    buffer.set_style(area, theme.status);
    if let Some(prompt) = &ui.prompt {
        let text = prompt.text();
        let line = Line::from(vec![
            Span::styled(prompt.label(), theme.status),
            Span::styled(text, theme.status.add_modifier(Modifier::BOLD)),
        ]);
        Paragraph::new(line).render(area, buffer);
        let before: String = prompt.text[..prompt.cursor].iter().collect();
        let x = area
            .x
            .saturating_add(Span::raw(prompt.label()).width() as u16)
            .saturating_add(Span::raw(before.as_str()).width() as u16)
            .min(area.x.saturating_add(area.width.saturating_sub(1)));
        return Some((x, area.y));
    }

    let space = workspace.active_space();
    let tab = workspace.active_tab();
    let tab_name = tab.map(|tab| tab.name.as_str()).unwrap_or("tab");
    let focused = workspace.focused_pane().and_then(|id| workspace.pane(id));
    let pane_name = focused
        .map(|pane| machines::qualified_name(&pane.address, &pane.name))
        .unwrap_or_else(|| "empty".to_owned());
    let row = focused.and_then(|pane| rows.iter().find(|row| row.pane == pane.id));
    let mut left = format!(" {} / {} / {}", space.name, tab_name, pane_name);
    if let Some(message) = &ui.message {
        left.push_str(" · ");
        left.push_str(message);
    } else if let Some(row) = row {
        if !row.title.is_empty() {
            left.push_str(" · ");
            left.push_str(&row.title);
        }
        left.push_str(" [");
        left.push_str(&row.word);
        left.push(']');
        left.push_str(&format!(" · turn {}", row.turn));
        if row.pinned {
            left.push_str(" · pinned");
        }
        if !row.seen {
            left.push_str(" · unseen");
        }
        if !row.app().is_empty() {
            left.push_str(" · ");
            left.push_str(row.app());
        }
        if let Some(progress) = row.status.as_ref().and_then(|status| status.progress) {
            left.push_str(&format!(" {progress}%"));
        }
    } else if let Some(pane) = focused {
        if !pane.title.is_empty() {
            left.push_str(" · ");
            left.push_str(&pane.title);
        }
        if let Some(status) = &pane.status {
            left.push_str(" [");
            left.push_str(status);
            left.push(']');
        }
    }
    let mut right = match ui.connection {
        Connection::Reconnecting { attempt: 0 } => "reconnecting…".to_owned(),
        Connection::Reconnecting { attempt } => format!("reconnecting (attempt {attempt})"),
        Connection::Live if ui.prefix => "prefix ^b…".to_owned(),
        Connection::Live if ui.preview.is_some() => {
            "preview · read-only · any key closes".to_owned()
        }
        Connection::Live if ui.manager.open => {
            "terminals · Enter show/hide · p preview · Esc close".to_owned()
        }
        Connection::Live if ui.machines.open => {
            "machines · Enter connect · a add · n terminal · Esc close".to_owned()
        }
        Connection::Live if ui.files.open => file_hint(ui, view.files).to_owned(),
        Connection::Live if ui.peek.is_some() => "peek · any key closes".to_owned(),
        Connection::Live if ui.sidebar.open => "agents · Esc closes".to_owned(),
        Connection::Live => "^b ? help".to_owned(),
    };
    if let Some(hint) = scroll_hint
        && matches!(ui.connection, Connection::Live)
    {
        right = format!("{hint} · {right}");
    }
    if let Some(attention) = agents::attention(rows).text() {
        right = format!("{attention} · {right}");
    }
    let width = area.width as usize;
    let left_width = Span::raw(left.as_str()).width();
    let right_width = Span::raw(right.as_str()).width();
    let spacer = width.saturating_sub(left_width + right_width).max(1);
    let line = Line::from(vec![
        Span::styled(left, theme.status),
        Span::raw(" ".repeat(spacer)),
        Span::styled(right, theme.status.add_modifier(Modifier::BOLD)),
    ]);
    Paragraph::new(line)
        .style(theme.status)
        .render(area, buffer);
    None
}

/// The search bar drawn in the status row: the literal query, the match
/// counts and the cursor.
fn draw_search(
    buffer: &mut Buffer,
    area: Rect,
    search: &super::state::SearchState,
    theme: &Theme,
) -> Option<(u16, u16)> {
    buffer.set_style(area, theme.status);
    let query = search.line.text();
    let label = " search: ";
    let left = format!("{label}{query}");
    let right = if query.is_empty() {
        "type to search scrollback · Esc close".to_owned()
    } else if !search.complete {
        "searching…".to_owned()
    } else if search.total == 0 {
        "no matches · Esc close".to_owned()
    } else {
        let position = search.selected.map(|index| index + 1).unwrap_or(0);
        format!(
            "{position}/{} matches · Enter next · Up prev · Esc close",
            search.total
        )
    };
    let width = area.width as usize;
    let left_width = Span::raw(left.as_str()).width();
    let right_width = Span::raw(right.as_str()).width();
    let spacer = width.saturating_sub(left_width + right_width).max(1);
    let line = Line::from(vec![
        Span::styled(left, theme.status.add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(spacer)),
        Span::styled(right, theme.status),
    ]);
    Paragraph::new(line)
        .style(theme.status)
        .render(area, buffer);
    let before: String = search.line.text[..search.line.cursor].iter().collect();
    let x = area
        .x
        .saturating_add(Span::raw(label).width() as u16)
        .saturating_add(Span::raw(before.as_str()).width() as u16)
        .min(area.x.saturating_add(area.width.saturating_sub(1)));
    Some((x, area.y))
}

/// The command palette overlay: a query line, the filtered commands and their
/// configured prefix key hints. It reuses the command table; it never writes
/// the layout or the shared document itself.
fn draw_palette(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let state = ui.palette.as_ref()?;
    if area.width < 16 || area.height < 6 {
        return None;
    }
    let width = area.width.saturating_sub(4).min(72);
    let height = area.height.saturating_sub(2).min(18);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    );
    Clear.render(rect, buffer);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay))
        .title(Line::from(Span::styled(
            " palette · Enter run · Esc close ",
            Style::default()
                .fg(theme.overlay)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width < 4 || inner.height < 2 {
        return None;
    }
    let query = state.line.text();
    let before = state.line.before();
    let head = format!("› {query}");
    Paragraph::new(Line::from(Span::styled(
        head,
        Style::default().add_modifier(Modifier::BOLD),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let rows = palette::filter(&palette::entries(&ui.keymap), &query);
    let list = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    if list.height == 0 {
        return None;
    }
    if rows.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "no matching commands",
            Style::default()
                .fg(theme.highlight)
                .add_modifier(Modifier::DIM),
        )))
        .render(list, buffer);
    } else {
        let selected = state.selected.min(rows.len() - 1);
        let visible = (list.height as usize).min(rows.len());
        // Keep the selected row visible in a short palette.
        let offset = selected.saturating_add(1).saturating_sub(visible);
        let lines: Vec<Line> = rows
            .iter()
            .skip(offset)
            .take(visible)
            .enumerate()
            .map(|(index, entry)| {
                let is_selected = offset + index == selected;
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default()
                };
                let hint = match entry.hint {
                    Some(key) => format!(" ^b {key} "),
                    None => String::new(),
                };
                let pad = (list.width as usize).saturating_sub(
                    Span::raw(entry.label).width() + Span::raw(hint.as_str()).width(),
                );
                Line::from(Span::styled(
                    format!(" {}{}{}", entry.label, " ".repeat(pad), hint),
                    style,
                ))
            })
            .collect();
        Paragraph::new(lines).render(list, buffer);
    }
    let x = inner
        .x
        .saturating_add(2)
        .saturating_add(Span::raw(before.as_str()).width() as u16)
        .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
    Some((x, inner.y))
}

fn draw_sidebar(buffer: &mut Buffer, area: Rect, ui: &UiState, rows: &[AgentRow], theme: &Theme) {
    let rect = sidebar_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    Clear.render(rect, buffer);
    let attention = agents::attention(rows);
    let mut title = String::from(" agents ");
    if let Some(text) = attention.text() {
        title.push_str("· ");
        title.push_str(&text);
        title.push(' ');
    }
    title.push_str("· j/k move · Enter jump · i peek · p pin · u unread · Esc close ");
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay))
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.overlay)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if rows.is_empty() {
        Paragraph::new(vec![
            Line::from("no agents yet"),
            Line::from(Span::styled(
                "an agent is a terminal that reports through OSC 7501",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ])
        .render(inner, buffer);
        return;
    }
    let selected = ui.sidebar.selected;
    let visible = (inner.height as usize).min(rows.len());
    let offset = ui.sidebar.offset.min(rows.len().saturating_sub(visible));
    let lines: Vec<Line> = rows
        .iter()
        .skip(offset)
        .take(visible)
        .map(|row| {
            let style = if Some(row.pane) == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(Span::styled(agent_line(row), style))
        })
        .collect();
    Paragraph::new(lines).render(inner, buffer);
}

/// One sidebar row: pin marker, word, title, address, app, progress, kind,
/// message, turn and unseen state — all from core's record.
fn agent_line(row: &AgentRow) -> String {
    let pin = if row.pinned { "*" } else { " " };
    let mut text = format!(
        "{pin} {:<8} {} · {} · turn {}",
        row.word, row.title, row.address, row.turn
    );
    if !row.seen {
        text.push_str(" · unseen");
    }
    if !row.app().is_empty() {
        text.push_str(" · ");
        text.push_str(row.app());
    }
    if let Some(status) = &row.status {
        if let Some(kind) = status.kind.as_deref().filter(|kind| !kind.is_empty()) {
            text.push_str(" · ");
            text.push_str(kind);
        }
        if let Some(progress) = status.progress {
            text.push_str(&format!(" · {progress}%"));
        }
        if !status.msg.is_empty() {
            text.push_str(" · ");
            text.push_str(&status.msg);
        }
    }
    text
}

fn draw_peek(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    screens: &BTreeMap<PaneId, VtScreen>,
    rows: &[AgentRow],
    theme: &Theme,
) {
    let Some(pane) = ui.peek else {
        return;
    };
    let Some(row) = rows.iter().find(|row| row.pane == pane) else {
        return;
    };
    let width = area.width.saturating_sub(4).min(96);
    let height = area.height.saturating_sub(2);
    if width < 8 || height < 5 {
        return;
    }
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    );
    Clear.render(rect, buffer);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.peek))
        .title(Line::from(Span::styled(
            " peek · any key closes ",
            Style::default().fg(theme.peek).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut facts = format!(
        "{} · {} · {} · turn {}",
        row.title, row.address, row.word, row.turn
    );
    if row.pinned {
        facts.push_str(" · pinned");
    }
    if !row.seen {
        facts.push_str(" · unseen");
    }
    if !row.app().is_empty() {
        facts.push_str(" · ");
        facts.push_str(row.app());
    }
    if let Some(progress) = row.status.as_ref().and_then(|status| status.progress) {
        facts.push_str(&format!(" · {progress}%"));
    }
    if let Some(kind) = row
        .status
        .as_ref()
        .and_then(|status| status.kind.as_deref())
        .filter(|kind| !kind.is_empty())
    {
        facts.push_str(" · ");
        facts.push_str(kind);
    }
    if let Some(msg) = row
        .status
        .as_ref()
        .map(|status| status.msg.as_str())
        .filter(|msg| !msg.is_empty())
    {
        facts.push_str(" · ");
        facts.push_str(msg);
    }
    Paragraph::new(Line::from(Span::styled(
        facts,
        Style::default().fg(theme.peek).add_modifier(Modifier::BOLD),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    if inner.height <= 1 {
        return;
    }
    // Core's foreground facts, when the program reported them.
    let header_rows = if let Some(foreground) = row.foreground.as_ref().filter(|foreground| {
        !foreground.name.is_empty() || !foreground.argv.is_empty() || !foreground.cwd.is_empty()
    }) {
        let mut line = String::from("fg ");
        line.push_str(&foreground.name);
        if !foreground.argv.is_empty() {
            line.push_str(" · ");
            line.push_str(&foreground.argv.join(" "));
        }
        if !foreground.cwd.is_empty() {
            line.push_str(" · ");
            line.push_str(&foreground.cwd);
        }
        Paragraph::new(Line::from(Span::styled(
            line,
            Style::default().add_modifier(Modifier::DIM),
        )))
        .render(Rect::new(inner.x, inner.y + 1, inner.width, 1), buffer);
        2
    } else {
        1
    };
    if inner.height <= header_rows {
        return;
    }
    let grid = Rect::new(
        inner.x,
        inner.y + header_rows,
        inner.width,
        inner.height - header_rows,
    );
    match screens.get(&pane) {
        Some(screen) => crate::screen::view::draw_buffer(buffer, grid, screen),
        None => Paragraph::new(Span::styled(
            "terminal not attached yet",
            Style::default().add_modifier(Modifier::DIM),
        ))
        .render(grid, buffer),
    }
}

/// The Core-wide terminal manager: a modal list of every live terminal with
/// its marker, word, age and address, a filter/count line and the local
/// selection. It draws only the rows the runtime derived from core; it never
/// reads a screen or infers a status.
fn draw_manager(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    rows: &[ManagerRow],
    theme: &Theme,
) -> Option<(u16, u16)> {
    if !ui.manager.open {
        return None;
    }
    let rect = manager_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let query = ui.manager.filter.text();
    let filtered = manager::filter(rows, &query);
    let total = rows.len();
    let shown = rows.iter().filter(|row| row.shown).count();
    Clear.render(rect, buffer);
    let title = format!(
        " terminal manager · {shown} shown · {total} live · \
         Enter show/hide · p preview · / filter · Esc close "
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay))
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.overlay)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let (head, label) = if ui.manager.filtering {
        (
            format!(" filter: {query} · {}/{} match", filtered.len(), total),
            " filter: ",
        )
    } else if query.is_empty() {
        (format!(" {total} live · * = shown here · / filter"), "")
    } else {
        (
            format!(
                " filter: {query} · {}/{} match · / edit",
                filtered.len(),
                total
            ),
            "",
        )
    };
    Paragraph::new(Line::from(Span::styled(
        head,
        Style::default().fg(theme.highlight),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);

    if inner.height > 1 {
        let list = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        if total == 0 {
            Paragraph::new(Line::from(Span::styled(
                " no live terminals in core",
                Style::default().add_modifier(Modifier::DIM),
            )))
            .render(list, buffer);
        } else if filtered.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                " no terminals match the filter",
                Style::default().add_modifier(Modifier::DIM),
            )))
            .render(list, buffer);
        } else {
            let selected = ui
                .manager
                .selected
                .as_deref()
                .and_then(|address| filtered.iter().position(|row| row.address == address))
                .unwrap_or(0);
            let visible = (list.height as usize).min(filtered.len());
            let mut offset = ui
                .manager
                .offset
                .min(filtered.len().saturating_sub(visible));
            if selected < offset {
                offset = selected;
            } else if selected >= offset + visible {
                offset = selected + 1 - visible;
            }
            let lines: Vec<Line> = filtered
                .iter()
                .skip(offset)
                .take(visible)
                .enumerate()
                .map(|(index, row)| {
                    let style = if offset + index == selected {
                        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(
                        manager::row_line(row, list.width as usize),
                        style,
                    ))
                })
                .collect();
            Paragraph::new(lines).render(list, buffer);
        }
    }
    if !ui.manager.filtering {
        return None;
    }
    let before = ui.manager.filter.before();
    let x = inner
        .x
        .saturating_add(Span::raw(label).width() as u16)
        .saturating_add(Span::raw(before.as_str()).width() as u16)
        .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
    Some((x, inner.y))
}

/// The file overlay's rectangle. The viewer, diff and prompt share it.
pub fn file_rect(area: Rect) -> Rect {
    if area.width < 8 || area.height < 5 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    )
}

/// Rows available to file text inside the overlay, below the facts line.
pub fn file_body_height(area: Rect) -> usize {
    file_rect(area).height.saturating_sub(2).saturating_sub(1) as usize
}

/// The status-bar hint for the file overlay.
fn file_hint(ui: &UiState, pane: Option<&Pane>) -> &'static str {
    if ui.files.prompt.is_some() {
        return "files · Enter read · Esc close";
    }
    match pane {
        Some(pane) if pane.conflict => "changed on disk · o overwrite · q discard",
        Some(pane) if pane.diff.is_some() => "diff · w write · q discard · Esc close",
        Some(pane) if pane.viewer.is_some() => "file · e edit · Esc close",
        _ => "files · Enter read · Esc close",
    }
}

/// The modal file overlay: the path prompt, the read-only viewer, or the
/// pending editor diff. It never writes the workspace document and never
/// contains terminal input.
fn draw_file(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    pane: Option<&Pane>,
    theme: &Theme,
) -> Option<(u16, u16)> {
    if !ui.files.open {
        return None;
    }
    let rect = file_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    Clear.render(rect, buffer);
    let title = if ui.files.prompt.is_some() {
        " file · Enter read · Esc close "
    } else if pane.is_some_and(|pane| pane.conflict) {
        " file diff · changed on disk · o overwrite · q discard "
    } else if pane.is_some_and(|pane| pane.diff.is_some()) {
        " file diff · w write · q discard · Esc close "
    } else {
        " file · e edit · Esc close "
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay))
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.overlay)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let mut rows: Vec<Line> = Vec::new();
    if let Some(line) = &ui.files.prompt {
        let context = &ui.files.context;
        let mut facts = context.machine_label().to_owned();
        if let Some(cwd) = context.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
            facts.push_str(" · cwd ");
            facts.push_str(cwd);
        }
        rows.push(Line::from(Span::styled(
            facts,
            Style::default().add_modifier(Modifier::DIM),
        )));
        let label = " path: ";
        let text = line.text();
        rows.push(Line::from(vec![
            Span::styled(label, Style::default().fg(theme.highlight)),
            Span::styled(text, Style::default().add_modifier(Modifier::BOLD)),
        ]));
        let before = line.before();
        let cursor = (
            inner
                .x
                .saturating_add(Span::raw(label).width() as u16)
                .saturating_add(Span::raw(before.as_str()).width() as u16)
                .min(inner.x.saturating_add(inner.width.saturating_sub(1))),
            inner.y + 1,
        );
        if let Some(error) = &ui.files.error {
            rows.push(Line::from(Span::styled(
                error.clone(),
                Style::default().fg(Color::Red),
            )));
        }
        rows.push(Line::from(Span::styled(
            "a path, not a directory; local or a machine's path",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(rows).render(inner, buffer);
        return Some(cursor);
    }

    let Some(pane) = pane else {
        Paragraph::new(Line::from(Span::styled(
            "no file open",
            Style::default().add_modifier(Modifier::DIM),
        )))
        .render(inner, buffer);
        return None;
    };

    let mut facts = pane.title();
    if let Some(diff) = &pane.diff {
        facts.push_str(" · ");
        facts.push_str(&diff.summary());
        if diff.truncated {
            facts.push_str(" (large change shown whole)");
        }
    } else if let Some(viewer) = &pane.viewer {
        facts.push_str(" · ");
        facts.push_str(&viewer.range_label());
        facts.push_str(" · e edit");
    }
    if pane.conflict {
        facts.push_str(" · the file changed since it was read");
    }
    rows.push(Line::from(Span::styled(
        facts,
        Style::default()
            .fg(theme.highlight)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(error) = &ui.files.error {
        rows.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    if pane.diff.is_some() {
        for row in pane.visible_diff(inner.height.saturating_sub(rows.len() as u16) as usize) {
            let (mark, number, style) = match row.tag {
                DiffTag::Same => (
                    " ",
                    row.new.or(row.old).unwrap_or(0),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                DiffTag::Add => ("+", row.new.unwrap_or(0), Style::default().fg(Color::Green)),
                DiffTag::Remove => ("-", row.old.unwrap_or(0), Style::default().fg(Color::Red)),
            };
            rows.push(Line::from(Span::styled(
                format!("{mark} {number:>5} {}", row.text),
                style,
            )));
        }
    } else if let Some(viewer) = &pane.viewer {
        let height = inner.height.saturating_sub(rows.len() as u16) as usize;
        if viewer.window.is_empty() {
            rows.push(Line::from(Span::styled(
                "empty file",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for (number, text) in viewer.visible(height) {
                rows.push(Line::from(format!("  {number:>5} {text}")));
            }
        }
    }
    Paragraph::new(rows).render(inner, buffer);
    None
}

/// The read-only observer preview: a border and title, a facts line, and the
/// local VT fed by the observer stream. It never shows a caret and never
/// offers input.
fn draw_preview(
    buffer: &mut Buffer,
    area: Rect,
    preview: &PreviewState,
    screen: Option<&VtScreen>,
    theme: &Theme,
) {
    let rect = preview_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    Clear.render(rect, buffer);
    let title = format!(" preview · {} · {} ", preview.title, preview.address);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.peek))
        .title(Line::from(Span::styled(
            title,
            Style::default().fg(theme.peek).add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    Paragraph::new(Line::from(Span::styled(
        " read-only observer · no keys, mouse or resize are sent · any key closes",
        Style::default().add_modifier(Modifier::DIM),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let grid = preview_grid(area);
    if grid.width == 0 || grid.height == 0 {
        return;
    }
    match screen {
        Some(screen) => crate::screen::view::draw_buffer(buffer, grid, screen),
        None => Paragraph::new(Span::styled(
            "waiting for the first frame…",
            Style::default().add_modifier(Modifier::DIM),
        ))
        .render(grid, buffer),
    }
}

fn draw_help(buffer: &mut Buffer, area: Rect, theme: &Theme) {
    let width = (HELP.iter().map(|line| line.len()).max().unwrap_or(0) as u16 + 4).min(area.width);
    let height = (HELP.len() as u16 + 2).min(area.height);
    if width == 0 || height == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(x, y, width, height);
    Clear.render(rect, buffer);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay))
        .title(Line::from(Span::styled(
            " help ",
            Style::default()
                .fg(theme.overlay)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    block.render(rect, buffer);
    Paragraph::new(
        HELP.iter()
            .map(|line| Line::from(*line))
            .collect::<Vec<_>>(),
    )
    .render(inner, buffer);
}

#[cfg(test)]
mod tests;
