//! Composing the workspace into a ratatui buffer.
//!
//! Pure presentation: the runtime supplies one VT snapshot per visible pane.
//! The tab bar sits on the first row, the status bar on the last, and panes
//! get the area between. Borders use the pane rectangle; the terminal grid
//! uses its inner rectangle. No core, VT or terminal is touched here.

use super::agents::{self, AgentRow};
use super::geometry::{self, Rects};
use super::model::{PaneId, Workspace};
use super::state::{Connection, UiState};
use crate::vt::Screen as VtScreen;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use std::collections::BTreeMap;

/// The help text. It is also the golden-tested documented key map.
pub const HELP: &[&str] = &[
    "prefix Ctrl-b      ? help          q detach",
    "c new tab          C new space",
    "n / p tab          N / P space     1..9 select tab",
    "v split left/right s split top/bottom",
    "h/j/k/l focus      H/J/K/L move",
    "z zoom             x close",
    "e keep + close     X kill + close",
    ", rename tab       . rename space",
    "a agents           i peek agent    g/G next/prev agent",
    "sidebar: j/k move  g/G first/last  Enter jump",
    "p pin / unpin      u mark unread   i peek selected",
    "Esc / a close sidebar               any key closes peek",
    "Ctrl-] detach      prefix twice sends Ctrl-b",
    "any key closes help",
];

/// Where the cursor belongs after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Render {
    /// The focused pane and its terminal grid, when it is visible.
    pub focused: Option<(PaneId, Rect)>,
    /// The rename prompt's cursor, when the prompt is open.
    pub prompt_cursor: Option<(u16, u16)>,
}

/// Paints a frame and describes the cursor to place.
pub fn render(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &Workspace,
    ui: &UiState,
    screens: &BTreeMap<PaneId, VtScreen>,
) -> Render {
    render_view(buffer, area, workspace, ui, screens, &[])
}

/// Paints a frame with the current agent rows: the status-bar attention count,
/// the sidebar overlay and the peek overlay.
pub fn render_view(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &Workspace,
    ui: &UiState,
    screens: &BTreeMap<PaneId, VtScreen>,
    rows: &[AgentRow],
) -> Render {
    let rects = geometry::compute(workspace.active_tab(), area);
    draw_panes(buffer, workspace, &rects, screens);
    draw_tab_bar(buffer, workspace, rects.tab_bar);
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
    rendered.prompt_cursor = draw_status(buffer, workspace, ui, rects.status, rows);
    if ui.sidebar.open {
        draw_sidebar(buffer, area, ui, rows);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.peek.is_some() {
        draw_peek(buffer, area, ui, screens, rows);
        rendered.focused = None;
        rendered.prompt_cursor = None;
    }
    if ui.help {
        draw_help(buffer, area);
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

fn draw_tab_bar(buffer: &mut Buffer, workspace: &Workspace, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let space = workspace.active_space();
    let mut spans = vec![Span::styled(
        format!(" {} ", space.name),
        Style::default().add_modifier(Modifier::BOLD),
    )];
    for (index, tab) in space.tabs.iter().enumerate() {
        let style = if tab.id == space.active {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
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
) {
    let focused = workspace.focused_pane();
    for (pane_id, rect) in &rects.panes {
        let Some(pane) = workspace.pane(*pane_id) else {
            continue;
        };
        let is_focused = focused == Some(*pane_id);
        let border = if is_focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut label = if pane.title.is_empty() || pane.title == pane.name {
            pane.name.clone()
        } else {
            format!("{}: {}", pane.name, pane.title)
        };
        if let Some(code) = pane.exit {
            label.push_str(&format!(" [exited {code}]"));
        }
        Block::new()
            .borders(Borders::ALL)
            .border_style(border)
            .title(Line::from(Span::styled(format!(" {label} "), border)))
            .render(*rect, buffer);
        if let Some(inner) = geometry::inner(*rect)
            && let Some(screen) = screens.get(pane_id)
        {
            crate::screen::view::draw_buffer(buffer, inner, screen);
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

/// Draws the status bar (or the rename prompt in its place) and returns the
/// prompt cursor.
fn draw_status(
    buffer: &mut Buffer,
    workspace: &Workspace,
    ui: &UiState,
    area: Rect,
    rows: &[AgentRow],
) -> Option<(u16, u16)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    buffer.set_style(area, Style::default().add_modifier(Modifier::REVERSED));
    if let Some(prompt) = &ui.prompt {
        let text = prompt.text();
        let line = Line::from(vec![
            Span::raw(prompt.label()),
            Span::styled(text, Style::default().add_modifier(Modifier::BOLD)),
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
    let pane_name = focused.map(|pane| pane.name.as_str()).unwrap_or("empty");
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
        Connection::Live if ui.peek.is_some() => "peek · any key closes".to_owned(),
        Connection::Live if ui.sidebar.open => "agents · Esc closes".to_owned(),
        Connection::Live => "^b ? help".to_owned(),
    };
    if let Some(attention) = agents::attention(rows).text() {
        right = format!("{attention} · {right}");
    }
    let width = area.width as usize;
    let left_width = Span::raw(left.as_str()).width();
    let right_width = Span::raw(right.as_str()).width();
    let spacer = width.saturating_sub(left_width + right_width).max(1);
    let style = Style::default().add_modifier(Modifier::REVERSED);
    let line = Line::from(vec![
        Span::styled(left, style),
        Span::raw(" ".repeat(spacer)),
        Span::styled(right, style.add_modifier(Modifier::BOLD)),
    ]);
    Paragraph::new(line).style(style).render(area, buffer);
    None
}

fn draw_sidebar(buffer: &mut Buffer, area: Rect, ui: &UiState, rows: &[AgentRow]) {
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
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
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
        .border_style(Style::default().fg(Color::Magenta))
        .title(Line::from(Span::styled(
            " peek · any key closes ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
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
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
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

fn draw_help(buffer: &mut Buffer, area: Rect) {
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
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(Span::styled(
            " help ",
            Style::default()
                .fg(Color::Cyan)
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
