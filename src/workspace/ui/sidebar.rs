//! The space rail: SPACES above the cross-space AGENTS index. It is drawn
//! beside the panes on a wide terminal, or as a drawer over them when the
//! window is narrow. One model (`sidebar::layout`) drives scrolling, drawing
//! and hit testing, including the two-line rows.

use super::super::agents::AgentRow;
use super::super::model::Workspace;
use super::super::panenav::truncate;
use super::super::sidebar::{self, FOOTER, Item, LineKind};
use super::super::state::UiState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Draws the whole rail into `rail`, with its one separator column.
pub(super) fn draw_sidebar(
    buffer: &mut Buffer,
    rail: Rect,
    workspace: &Workspace,
    ui: &UiState,
    rows: &[AgentRow],
    theme: &Theme,
) {
    if rail.width == 0 || rail.height == 0 {
        return;
    }
    // The rail is also the narrow drawer, painted over the panes. Clear every
    // cell first, including the separator column: setting only a background
    // would leave the covered pane's glyphs and attributes in cells the rail
    // text does not overwrite.
    for y in rail.y..rail.y.saturating_add(rail.height) {
        for x in rail.x..rail.x.saturating_add(rail.width) {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(Style::default().bg(theme.side));
            }
        }
    }
    let separator = rail.x.saturating_add(rail.width.saturating_sub(1));
    let rule = Style::default().fg(theme.line).bg(theme.side);
    for y in rail.y..rail.y.saturating_add(rail.height) {
        if let Some(cell) = buffer.cell_mut((separator, y)) {
            cell.set_symbol("│").set_style(rule);
        }
    }
    let layout = sidebar::layout(workspace, rows, rail, ui.sidebar.offset);
    let selected = ui.sidebar.selected;
    let focused = workspace.focused_pane();
    let space_count = workspace.spaces().len();
    let agent_count = rows.len();
    for line in &layout.lines {
        match line.kind {
            LineKind::SpacesHeader => draw_section(
                buffer,
                line.rect,
                "SPACES",
                space_count,
                Style::default().fg(theme.muted).bg(theme.side),
                theme,
            ),
            LineKind::Divider => draw_text(
                buffer,
                line.rect,
                "─".repeat(line.rect.width as usize),
                Style::default().fg(theme.line).bg(theme.side),
            ),
            LineKind::AgentHeader => draw_section(
                buffer,
                line.rect,
                "AGENTS",
                agent_count,
                Style::default().fg(theme.muted).bg(theme.side),
                theme,
            ),
            LineKind::Attention => {
                if let Some(summary) = super::super::agents::attention(rows).summary() {
                    draw_text(
                        buffer,
                        line.rect,
                        format!(" {summary}"),
                        Style::default().fg(theme.dim).bg(theme.side),
                    );
                }
            }
            LineKind::Space(id) => {
                let Some(space) = workspace.space(id) else {
                    continue;
                };
                let Some(index) = workspace
                    .spaces()
                    .iter()
                    .position(|candidate| candidate.id == id)
                else {
                    continue;
                };
                let active = workspace.active_space_id() == id;
                let highlighted = selected == Some(Item::Space(id));
                draw_space(
                    buffer,
                    line.rect,
                    active,
                    highlighted,
                    index + 1,
                    &space.name,
                    space.tabs.len(),
                    theme,
                );
            }
            LineKind::SpaceFolder(id) => {
                let Some(space) = workspace.space(id) else {
                    continue;
                };
                if space.folder.is_empty() {
                    continue;
                }
                let active = workspace.active_space_id() == id;
                let highlighted = selected == Some(Item::Space(id));
                draw_row_background(buffer, line.rect, active, highlighted, theme);
                let style = Style::default()
                    .fg(theme.dim)
                    .bg(row_bg(active, highlighted, theme));
                draw_text(
                    buffer,
                    line.rect,
                    format!("   {}", truncate(&space.folder, budget(line.rect, 3))),
                    style,
                );
            }
            LineKind::Agent(pane) => {
                let Some(row) = rows.iter().find(|row| row.pane == pane) else {
                    continue;
                };
                let active = focused == Some(pane);
                let highlighted = selected == Some(Item::Agent(pane));
                draw_agent(buffer, line.rect, row, active, highlighted, theme);
            }
            LineKind::AgentDetail(pane) => {
                let Some(row) = rows.iter().find(|row| row.pane == pane) else {
                    continue;
                };
                let active = focused == Some(pane);
                let highlighted = selected == Some(Item::Agent(pane));
                draw_row_background(buffer, line.rect, active, highlighted, theme);
                let detail = sidebar::space_of(workspace, pane).map_or_else(
                    || row.address.clone(),
                    |space| format!("{} · {}", space.name, row.address),
                );
                let style = Style::default()
                    .fg(theme.dim)
                    .bg(row_bg(active, highlighted, theme));
                draw_text(
                    buffer,
                    line.rect,
                    format!("   {}", truncate(&detail, budget(line.rect, 3))),
                    style,
                );
            }
            LineKind::EmptyAgents => draw_text(
                buffer,
                line.rect,
                " no agents yet".to_owned(),
                Style::default().fg(theme.dim).bg(theme.side),
            ),
            LineKind::Footer => {}
        }
    }
    if let Some(footer) = layout.footer {
        draw_text(
            buffer,
            footer,
            format!(" {FOOTER}"),
            Style::default().fg(theme.dim).bg(theme.side),
        );
    }
}

fn row_bg(active: bool, highlighted: bool, theme: &Theme) -> ratatui::style::Color {
    if active {
        theme.active_bg
    } else if highlighted {
        theme.surface0
    } else {
        theme.side
    }
}

fn draw_row_background(
    buffer: &mut Buffer,
    rect: Rect,
    active: bool,
    selected: bool,
    theme: &Theme,
) {
    if active {
        buffer.set_style(rect, Style::default().bg(theme.active_bg));
    } else if selected {
        buffer.set_style(rect, theme.selected_sidebar().bg(theme.side));
    } else {
        buffer.set_style(rect, Style::default().bg(theme.side));
    }
}

/// A section head: the uppercase name on the left, its count on the right.
fn draw_section(
    buffer: &mut Buffer,
    rect: Rect,
    name: &str,
    count: usize,
    style: Style,
    theme: &Theme,
) {
    buffer.set_style(rect, Style::default().bg(theme.side));
    let left = vec![Span::styled(
        format!(" {name}"),
        style.add_modifier(Modifier::BOLD),
    )];
    draw_right(
        buffer,
        rect,
        left,
        &count.to_string(),
        Style::default().fg(theme.dim).bg(theme.side),
        theme.side,
        theme,
    );
}

/// One space row: index, name and the tab count. The active space gets the
/// mint left edge and the active background; keyboard selection is a quieter
/// surface.
#[allow(clippy::too_many_arguments)]
fn draw_space(
    buffer: &mut Buffer,
    rect: Rect,
    active: bool,
    selected: bool,
    index: usize,
    name: &str,
    tabs: usize,
    theme: &Theme,
) {
    draw_row_background(buffer, rect, active, selected, theme);
    let bg = row_bg(active, selected, theme);
    let usable = budget(rect, 1);
    let count = if tabs == 1 {
        "1 tab".to_owned()
    } else {
        format!("{tabs} tabs")
    };
    let count_width = Span::raw(count.as_str()).width();
    let left_budget = usable.saturating_sub(count_width + 1);
    let label = truncate(&format!("{index:02} {name}"), left_budget.saturating_sub(2));
    let fg = if active { theme.mint } else { theme.text };
    let mut style = Style::default().fg(fg).bg(bg);
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    let marker = if active { "▌" } else { " " };
    let left = vec![
        Span::styled(format!("{marker} "), Style::default().fg(theme.mint).bg(bg)),
        Span::styled(label, style),
    ];
    draw_right(
        buffer,
        rect,
        left,
        &count,
        Style::default().fg(theme.muted).bg(bg),
        bg,
        theme,
    );
}

/// One agent row's first line: pin/unread marker, title and the Core word.
/// The focused pane's agent is the active row; a keyboard selection is a
/// quieter surface. Both stay legible together.
fn draw_agent(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    active: bool,
    selected: bool,
    theme: &Theme,
) {
    draw_row_background(buffer, rect, active, selected, theme);
    let bg = row_bg(active, selected, theme);
    let usable = budget(rect, 1);
    let word = row.word.as_str();
    let word_width = Span::raw(word).width();
    let left_budget = usable.saturating_sub(word_width + 1);
    let name = if row.title.is_empty() {
        row.name.as_str()
    } else {
        row.title.as_str()
    };
    let name = truncate(name, left_budget.saturating_sub(3));
    let pin = if row.pinned {
        "*"
    } else if !row.seen {
        "•"
    } else {
        " "
    };
    let fg = if active || selected {
        theme.text
    } else {
        theme.subtext0
    };
    let marker = if active { "▌" } else { " " };
    let left = vec![
        Span::styled(marker, Style::default().fg(theme.mint).bg(bg)),
        Span::styled(format!("{pin} "), Style::default().fg(theme.muted).bg(bg)),
        Span::styled(
            name,
            Style::default().fg(fg).bg(bg).add_modifier(if active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
    ];
    draw_right(
        buffer,
        rect,
        left,
        word,
        Style::default().fg(theme.status_word(word)).bg(bg),
        bg,
        theme,
    );
}

/// Draws left spans and a right-aligned value in one rect, keeping the value
/// fully readable and reserving one cell after it.
fn draw_right(
    buffer: &mut Buffer,
    rect: Rect,
    left: Vec<Span<'_>>,
    right: &str,
    right_style: Style,
    _background: ratatui::style::Color,
    _theme: &Theme,
) {
    let usable = budget(rect, 1);
    let left_width: usize = left.iter().map(Span::width).sum();
    let right_width = Span::raw(right).width();
    if left_width + right_width + 1 > usable {
        // The word survives; the name was already truncated to its budget.
        Paragraph::new(Line::from(left)).render(rect, buffer);
        return;
    }
    let spacer = usable - left_width - right_width;
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(spacer)));
    spans.push(Span::styled(right.to_owned(), right_style));
    Paragraph::new(Line::from(spans)).render(rect, buffer);
}

/// The cells left of the separator and the right padding.
fn budget(rect: Rect, padding: u16) -> usize {
    (rect.width as usize).saturating_sub(usize::from(padding))
}

fn draw_text(buffer: &mut Buffer, rect: Rect, text: String, style: Style) {
    // Every rail line stays inside the content width (the separator is
    // outside `rect`); a line that does not fit ends with an ellipsis rather
    // than a hard cut at the rect edge.
    let text = truncate(&text, rect.width as usize);
    Paragraph::new(Span::styled(text, style)).render(rect, buffer);
}
