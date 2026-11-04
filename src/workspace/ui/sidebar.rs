//! The persistent left sidebar: spaces on top, agents below. It is drawn
//! beside the panes, not over them, and keeps a quiet panel background with a
//! single separator column.

use super::super::agents::AgentRow;
use super::super::model::Workspace;
use super::super::sidebar::{self, Item, LineKind};
use super::super::state::UiState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Draws the whole panel. It stays visible whether or not it has keyboard
/// focus; focus only changes where keys go.
pub(super) fn draw_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &Workspace,
    ui: &UiState,
    rows: &[AgentRow],
    theme: &Theme,
) {
    let rect = sidebar::rect(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    buffer.set_style(rect, theme.panel());
    // One separator column in surface1; the text area is everything left of
    // it.
    let separator = rect.x.saturating_add(rect.width.saturating_sub(1));
    let rule = Style::default().fg(theme.surface1).bg(theme.panel_bg);
    for y in rect.y..rect.y.saturating_add(rect.height) {
        if let Some(cell) = buffer.cell_mut((separator, y)) {
            cell.set_symbol("│").set_style(rule);
        }
    }
    let layout = sidebar::layout(workspace, rows, area, ui.sidebar.offset);
    let selected = ui.sidebar.selected;
    for line in &layout.lines {
        match line.kind {
            LineKind::SpacesHeader => {
                draw_line(
                    buffer,
                    line.rect,
                    " spaces",
                    Style::default().fg(theme.overlay0),
                );
            }
            LineKind::AgentHeader => {
                draw_line(
                    buffer,
                    line.rect,
                    " agents",
                    Style::default().fg(theme.overlay0),
                );
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
                let highlighted = selected == Some(Item::Space(id)) || active;
                let background = if highlighted {
                    theme.selected_sidebar()
                } else {
                    theme.panel()
                };
                let marker = if active { "●" } else { " " };
                let label = format!(" {} {} {}", marker, index + 1, space.name);
                let style = Style::default()
                    .fg(if active { theme.accent } else { theme.text })
                    .add_modifier(if active {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
                    .patch(background);
                draw_line(buffer, line.rect, &label, style);
            }
            LineKind::Agent(pane) => {
                let Some(row) = rows.iter().find(|row| row.pane == pane) else {
                    continue;
                };
                let highlighted = selected == Some(Item::Agent(pane));
                draw_agent(buffer, line.rect, row, highlighted, theme);
            }
            LineKind::EmptyAgents => {
                draw_line(
                    buffer,
                    line.rect,
                    " no agents yet",
                    Style::default().fg(theme.overlay0),
                );
            }
        }
    }
}

fn draw_agent(buffer: &mut Buffer, rect: Rect, row: &AgentRow, selected: bool, theme: &Theme) {
    if rect.width == 0 {
        return;
    }
    let background = if selected {
        theme.selected_sidebar()
    } else {
        theme.panel()
    };
    let pin = if row.pinned { "*" } else { " " };
    let name = if row.title.is_empty() {
        row.name.as_str()
    } else {
        row.title.as_str()
    };
    // Keep one space and the full status word readable even in the narrow
    // panel. The prefix " {pin} " is always three cells and the word span is
    // one space plus the word's display width; counting characters here would
    // let wide titles push the word past the separator.
    let fixed = 4 + Span::raw(row.word.as_str()).width();
    let room = (rect.width as usize).saturating_sub(fixed);
    let name = truncate(name, room);
    let line = Line::from(vec![
        Span::styled(
            format!(" {pin} "),
            Style::default().fg(theme.overlay0).patch(background),
        ),
        Span::styled(
            name,
            Style::default()
                .fg(if selected { theme.text } else { theme.subtext0 })
                .patch(background),
        ),
        Span::styled(
            format!(" {}", row.word),
            Style::default()
                .fg(theme.status_word(&row.word))
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                })
                .patch(background),
        ),
    ]);
    Paragraph::new(line).render(rect, buffer);
}

/// Truncates to `room` display columns, reserving one for an ellipsis. Counting
/// characters would let a CJK or emoji title overflow its budget.
fn truncate(text: &str, room: usize) -> String {
    if Span::raw(text).width() <= room {
        return text.to_owned();
    }
    if room == 0 {
        return String::new();
    }
    let budget = room - 1;
    let mut used = 0;
    let mut shortened = String::new();
    for ch in text.chars() {
        let width = Span::raw(ch.to_string()).width();
        if used + width > budget {
            break;
        }
        used += width;
        shortened.push(ch);
    }
    shortened.push('…');
    shortened
}

fn draw_line(buffer: &mut Buffer, rect: Rect, text: &str, style: Style) {
    Paragraph::new(Span::styled(text.to_owned(), style)).render(rect, buffer);
}
