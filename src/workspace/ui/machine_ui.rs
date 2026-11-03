//! The machine manager's panel: core's machine rows, the add/rename prompt,
//! the removal confirmation and core's last refusal. Extracted from the main
//! drawing module so each file stays near the project's size guidance.

use super::super::machines::{self, MachineRow};
use super::super::state::UiState;
use super::{Theme, manager_rect};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// The machine manager uses the same wide panel as the terminal manager. Each
/// row is fitted by `machines::row_line`.
pub(super) fn machine_rect(area: Rect) -> Rect {
    manager_rect(area)
}

/// How many machine rows fit under the count line and the error/help line.
pub fn machine_list_height(area: Rect) -> usize {
    machine_rect(area)
        .height
        .saturating_sub(2)
        .saturating_sub(2) as usize
}

/// The machine manager: core's machines with state, since and why, plus the
/// add/rename prompt, the removal confirmation and core's last refusal. All
/// facts come from `machine.list`; nothing is inferred from a screen.
pub(super) fn draw_machines(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    rows: &[MachineRow],
    theme: &Theme,
) -> Option<(u16, u16)> {
    if !ui.machines.open {
        return None;
    }
    let rect = machine_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let query = ui.machines.filter.text();
    let filtered = machines::filter(rows, &query);
    let total = rows.len();
    let connected = rows.iter().filter(|row| row.state == "connected").count();
    Clear.render(rect, buffer);
    let title = format!(
        " machine manager · {connected}/{total} connected · Enter connect · a add · \
         r rename · d remove · n new terminal · / filter · Esc close "
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
    let (head, label) = if ui.machines.prompt.is_none() && ui.machines.filtering {
        (
            format!(" filter: {query} · {}/{} match", filtered.len(), total),
            " filter: ",
        )
    } else if query.is_empty() {
        (format!(" {total} machines · / filter"), "")
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

    let mut cursor = None;
    if inner.height > 1 {
        let line = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        if let Some(prompt) = &ui.machines.prompt {
            let text = prompt.line.text();
            Paragraph::new(Line::from(vec![
                Span::styled(prompt.label(), theme.status),
                Span::styled(text, theme.status.add_modifier(Modifier::BOLD)),
            ]))
            .render(line, buffer);
            let before = prompt.line.before();
            let x = inner
                .x
                .saturating_add(Span::raw(prompt.label()).width() as u16)
                .saturating_add(Span::raw(before.as_str()).width() as u16)
                .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
            cursor = Some((x, line.y));
        } else if let Some(machine) = &ui.machines.confirm {
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " remove {machine}? its terminals leave this workspace · \
                     y/Enter remove · n/Esc cancel"
                ),
                Style::default()
                    .fg(theme.highlight)
                    .add_modifier(Modifier::BOLD),
            )))
            .render(line, buffer);
        } else if let Some(error) = &ui.machines.error {
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(theme.highlight),
            )))
            .render(line, buffer);
        } else {
            Paragraph::new(Line::from(Span::styled(
                " target is passed to core unchanged · core names the machine · \
                 j/k move · g/G first/last",
                Style::default().add_modifier(Modifier::DIM),
            )))
            .render(line, buffer);
        }
    }

    if inner.height > 2 {
        let list = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        if total == 0 {
            Paragraph::new(Line::from(Span::styled(
                " no machines · a adds one by target (for example user@host:port)",
                Style::default().add_modifier(Modifier::DIM),
            )))
            .render(list, buffer);
        } else if filtered.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                " no machines match the filter",
                Style::default().add_modifier(Modifier::DIM),
            )))
            .render(list, buffer);
        } else {
            let selected = ui
                .machines
                .selected
                .as_deref()
                .and_then(|name| filtered.iter().position(|row| row.name == name))
                .unwrap_or(0);
            let visible = (list.height as usize).min(filtered.len());
            let mut offset = ui
                .machines
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
                        machines::row_line(row, list.width as usize),
                        style,
                    ))
                })
                .collect();
            Paragraph::new(lines).render(list, buffer);
        }
    }
    if ui.machines.prompt.is_some() {
        return cursor;
    }
    if !ui.machines.filtering {
        return None;
    }
    let before = ui.machines.filter.before();
    let x = inner
        .x
        .saturating_add(Span::raw(label).width() as u16)
        .saturating_add(Span::raw(before.as_str()).width() as u16)
        .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
    Some((x, inner.y))
}
