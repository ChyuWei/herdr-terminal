//! Composing the workspace into a ratatui buffer.
//!
//! Pure presentation: the runtime supplies one VT snapshot per visible pane.
//! The sidebar occupies the left edge on wide terminals, the tab bar sits on
//! the first row of the remaining area, the status bar on the last, and panes
//! get the space between. A mode strip overlays the bottom pane row rather
//! than shrinking the panes. Borders use the pane rectangle; the terminal
//! grid uses its inner rectangle. No core, VT or terminal is touched here.

use super::agents::{self, AgentRow};
use super::file::Pane;
use super::geometry::{self, Rects};
use super::state::{Connection, UiState};

mod machine_ui;
mod overlays;
mod sidebar;

pub use machine_ui::machine_list_height;
pub use overlays::{file_body_height, file_rect};

use super::machines::MachineRow;
use super::manager::ManagerRow;
use super::model::{PaneId, Workspace};
use crate::theme::Theme;
use crate::vt::Screen as VtScreen;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
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
    "a sidebar          i peek agent    g/G next/prev agent",
    "t terminal manager (all live terminals in core)",
    "manager: j/k move  g/G first/last  Enter show/hide",
    "/ filter  p preview (read-only)  Esc close manager",
    "m machine manager · Enter connect · a add · r rename · d remove",
    "machines: n new terminal there · / filter · Esc close · add takes target only",
    "o file viewer       e edit in $VISUAL/$EDITOR, diff before write",
    "file: Enter read · e edit · w write · q discard · Esc close",
    "sidebar: j/k move  g/G first/last  Enter switch/jump  Esc/a close",
    "p pin / unpin  u mark unread  i peek  any key closes peek",
    "r resize mode",
    "resize: h/l width  j/k height  Shift for 5  Esc/q done",
    "Ctrl-] detach      prefix twice sends Ctrl-b",
    ": palette          f search scrollback",
    "[ select mode      y copy selection",
    "selection: drag or h/j/k/l or w/b/e, y copy, Esc cancel",
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
    if rects.panes.is_empty() && rects.workspace.height > 0 {
        draw_empty(buffer, rects.workspace, theme);
    }
    sidebar::draw_sidebar(buffer, area, workspace, ui, view.rows, theme);
    draw_strip(buffer, &rects, ui, theme);
    let mut rendered = Render {
        focused: workspace.focused_pane().and_then(|id| {
            rects
                .pane(id)
                .and_then(geometry::inner)
                .map(|inner| (id, inner))
        }),
        prompt_cursor: None,
    };
    rendered.prompt_cursor = draw_status(buffer, workspace, ui, rects.status, theme, view);
    if ui.search.is_some() {
        // The search bar owns the status row; the program's cursor would
        // suggest the keys are still typing into it.
        rendered.focused = None;
    }
    if ui.sidebar.open {
        // The sidebar has keyboard focus; the pane cursor would suggest
        // otherwise.
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.peek.is_some() {
        overlays::draw_peek(buffer, area, ui, view.screens, view.rows, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.manager.open {
        rendered.prompt_cursor = overlays::draw_manager(buffer, area, ui, view.manager, theme);
        rendered.focused = None;
    }
    if ui.machines.open {
        rendered.prompt_cursor = overlays::draw_machines(buffer, area, ui, view.machines, theme);
        rendered.focused = None;
    }
    if ui.files.open {
        rendered.prompt_cursor = overlays::draw_file(buffer, area, ui, view.files, theme);
        rendered.focused = None;
    }
    if let Some(preview) = &ui.preview {
        overlays::draw_preview(buffer, area, preview, view.preview, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.palette.is_some() {
        rendered.prompt_cursor = overlays::draw_palette(buffer, area, ui, theme);
        rendered.focused = None;
    }
    if ui.help {
        overlays::draw_help(buffer, area, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    rendered
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
    buffer.set_style(area, theme.panel());
    let space = workspace.active_space();
    let mut spans = vec![
        Span::styled(
            format!(" {} ", space.name),
            Style::default()
                .fg(theme.accent)
                .bg(theme.panel_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│", Style::default().fg(theme.surface1).bg(theme.panel_bg)),
    ];
    for (index, tab) in space.tabs.iter().enumerate() {
        let style = if tab.id == space.active {
            theme.tab_active
        } else {
            theme.tab_inactive
        };
        spans.push(Span::styled(format!(" {} {} ", index + 1, tab.name), style));
    }
    Paragraph::new(Line::from(spans))
        .style(theme.panel())
        .render(area, buffer);
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
        let border = Style::default()
            .fg(if is_focused {
                theme.accent
            } else {
                theme.surface1
            })
            .bg(theme.panel_bg);
        let title = border.add_modifier(if is_focused {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        let mut label = super::machines::qualified_name(
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
            .title(Line::from(Span::styled(format!(" {label} "), title)))
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

fn draw_empty(buffer: &mut Buffer, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = [
        Line::from(Span::styled(
            "no terminals in this tab",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "prefix v: new pane · prefix c: new tab · prefix ?: help",
            Style::default().fg(theme.overlay0),
        )),
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
/// returns the field cursor. The status bar never reverses the row: values
/// use the accent, secondary text and hints use overlay0.
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
    let background = theme.panel_bg;
    let hint = Style::default().fg(theme.overlay0).bg(background);
    let value = Style::default().fg(theme.accent).bg(background);
    let body = Style::default().fg(theme.text).bg(background);
    if let Some(prompt) = &ui.prompt {
        let text = prompt.text();
        // The prompt label itself is the chip: accent background, panel text.
        let chip = Style::default()
            .fg(theme.panel_bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::styled(prompt.label(), chip),
            Span::styled(text, body.add_modifier(Modifier::BOLD)),
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
        .map(|pane| super::machines::qualified_name(&pane.address, &pane.name))
        .unwrap_or_else(|| "empty".to_owned());
    let row = focused.and_then(|pane| rows.iter().find(|row| row.pane == pane.id));

    let mut left_spans = vec![
        Span::styled(" ", hint),
        Span::styled(space.name.clone(), value),
        Span::styled(" / ", hint),
        Span::styled(tab_name.to_owned(), value),
        Span::styled(" / ", hint),
        Span::styled(pane_name, value),
    ];
    if let Some(message) = &ui.message {
        left_spans.push(Span::styled(" · ", hint));
        left_spans.push(Span::styled(message.clone(), hint));
    } else if let Some(row) = row {
        if !row.title.is_empty() {
            left_spans.push(Span::styled(" · ", hint));
            left_spans.push(Span::styled(row.title.clone(), body));
        }
        left_spans.push(Span::styled(" [", hint));
        left_spans.push(Span::styled(
            row.word.clone(),
            Style::default()
                .fg(theme.status_word(&row.word))
                .bg(background),
        ));
        left_spans.push(Span::styled("]", hint));
        left_spans.push(Span::styled(format!(" · turn {}", row.turn), hint));
        if row.pinned {
            left_spans.push(Span::styled(" · pinned", hint));
        }
        if !row.seen {
            left_spans.push(Span::styled(" · unseen", hint));
        }
        if !row.app().is_empty() {
            left_spans.push(Span::styled(format!(" · {}", row.app()), hint));
        }
        if let Some(progress) = row.status.as_ref().and_then(|status| status.progress) {
            left_spans.push(Span::styled(format!(" {progress}%"), value));
        }
    } else if let Some(pane) = focused {
        if !pane.title.is_empty() {
            left_spans.push(Span::styled(" · ", hint));
            left_spans.push(Span::styled(pane.title.clone(), body));
        }
        if let Some(status) = &pane.status {
            left_spans.push(Span::styled(" [", hint));
            left_spans.push(Span::styled(
                status.clone(),
                Style::default()
                    .fg(theme.status_word(status))
                    .bg(background),
            ));
            left_spans.push(Span::styled("]", hint));
        }
    }
    let mut right = match ui.connection {
        Connection::Reconnecting { attempt: 0 } => "reconnecting…".to_owned(),
        Connection::Reconnecting { attempt } => format!("reconnecting (attempt {attempt})"),
        Connection::Live if ui.preview.is_some() => {
            "preview · read-only · any key closes".to_owned()
        }
        Connection::Live if ui.manager.open => {
            "terminals · Enter show/hide · p preview · Esc close".to_owned()
        }
        Connection::Live if ui.machines.open => {
            "machines · Enter connect · a add · n terminal · Esc close".to_owned()
        }
        Connection::Live if ui.files.open => overlays::file_hint(ui, view.files).to_owned(),
        Connection::Live if ui.peek.is_some() => "peek · any key closes".to_owned(),
        Connection::Live if ui.sidebar.open => "sidebar · Esc closes".to_owned(),
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
    let right_style = if matches!(ui.connection, Connection::Live) {
        hint
    } else {
        Style::default().fg(theme.yellow).bg(background)
    };
    let area_width = area.width as usize;
    let right_width = Span::raw(right.as_str()).width();
    if right_width + 1 >= area_width {
        // The right hint is the one that must survive a narrow bar.
        Paragraph::new(Line::from(Span::styled(right, right_style)))
            .style(theme.status)
            .render(area, buffer);
        return None;
    }
    // Reserve the right edge for the hint and clip the left side first, so a
    // long message never hides the key that gets the person out of a mode.
    let left_width = area_width - right_width - 1;
    Paragraph::new(Line::from(left_spans))
        .style(theme.status)
        .render(
            Rect::new(area.x, area.y, left_width as u16, area.height),
            buffer,
        );
    let hint_area = Rect::new(
        area.x.saturating_add(left_width as u16),
        area.y,
        area.width.saturating_sub(left_width as u16),
        area.height,
    );
    Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(right, right_style),
    ]))
    .style(theme.status)
    .render(hint_area, buffer);
    None
}

/// The search bar drawn in the status row: a chip label, the literal query,
/// the match counts and the cursor.
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
    let chip = Style::default()
        .fg(theme.panel_bg)
        .bg(theme.mauve)
        .add_modifier(Modifier::BOLD);
    let body = Style::default().fg(theme.text).bg(theme.panel_bg);
    let hint = Style::default().fg(theme.overlay0).bg(theme.panel_bg);
    let width = area.width as usize;
    let left_width = Span::raw(left.as_str()).width();
    let right_width = Span::raw(right.as_str()).width();
    let spacer = width.saturating_sub(left_width + right_width).max(1);
    let line = Line::from(vec![
        Span::styled(label, chip),
        Span::styled(query, body.add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(spacer)),
        Span::styled(right, hint),
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

/// The one-row mode strip. It overlays the last pane row and leaves the
/// status bar underneath, so opening or closing a mode never resizes a
/// terminal.
fn draw_strip(buffer: &mut Buffer, rects: &Rects, ui: &UiState, theme: &Theme) {
    let Some(rect) = strip_rect(rects) else {
        return;
    };
    let mode = if ui.resize {
        Some(Mode::Resize)
    } else if ui.selecting.is_some() {
        Some(Mode::Copy)
    } else if ui.prefix {
        Some(Mode::Prefix)
    } else {
        None
    };
    let Some(mode) = mode else {
        return;
    };
    buffer.set_style(rect, theme.panel());
    let hint = Style::default().fg(theme.overlay0).bg(theme.panel_bg);
    let key = Style::default().fg(theme.accent).bg(theme.panel_bg);
    let (chip_label, chip_color) = match mode {
        Mode::Prefix => (" PREFIX ", theme.accent),
        Mode::Copy => (" COPY ", theme.mauve),
        Mode::Resize => (" RESIZE ", theme.yellow),
    };
    let chip = Style::default()
        .fg(theme.panel_bg)
        .bg(chip_color)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(chip_label, chip)];
    match mode {
        Mode::Prefix => {
            spans.extend([
                Span::styled("esc", key),
                Span::styled(" cancel · ", hint),
                Span::styled("^b", key),
                Span::styled(" send prefix · ", hint),
                Span::styled("?", key),
                Span::styled(" keybinds", hint),
            ]);
        }
        Mode::Copy => {
            spans.extend([
                Span::styled("h/j/k/l", key),
                Span::styled(" or ", hint),
                Span::styled("w/b/e", key),
                Span::styled(" · ", hint),
                Span::styled("v/space", key),
                Span::styled(" select · ", hint),
                Span::styled("y/enter", key),
                Span::styled(" copy · ", hint),
                Span::styled("/?", key),
                Span::styled(" search · ", hint),
                Span::styled("esc", key),
                Span::styled(" cancel", hint),
            ]);
        }
        Mode::Resize => {
            spans.extend([
                Span::styled("h/l", key),
                Span::styled(" width · ", hint),
                Span::styled("j/k", key),
                Span::styled(" height · ", hint),
                Span::styled("esc", key),
                Span::styled(" done", hint),
            ]);
        }
    }
    Paragraph::new(Line::from(spans))
        .style(theme.panel())
        .render(rect, buffer);
}

#[derive(Clone, Copy)]
enum Mode {
    Prefix,
    Copy,
    Resize,
}

/// The strip covers the last row of the pane area, never the status bar.
fn strip_rect(rects: &Rects) -> Option<Rect> {
    if rects.workspace.width == 0 || rects.workspace.height == 0 {
        return None;
    }
    Some(Rect::new(
        rects.workspace.x,
        rects
            .workspace
            .y
            .saturating_add(rects.workspace.height.saturating_sub(1)),
        rects.workspace.width,
        1,
    ))
}

#[cfg(test)]
mod tests;
