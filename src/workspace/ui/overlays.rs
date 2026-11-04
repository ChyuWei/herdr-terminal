//! Modal overlays: the command palette, agent peek, terminal manager,
//! machine manager, file viewer and help. They share one panel treatment:
//! a panel background, an overlay0 border, an accent title and an accent
//! selected row.

use super::super::agents::AgentRow;
use super::super::file::{DiffTag, Pane};
use super::super::machines::{self, MachineRow};
use super::super::manager::{self, ManagerRow};
use super::super::model::PaneId;
use super::super::panenav::truncate;
use super::super::state::{PreviewState, UiState};
use super::{HELP, Theme};
use crate::vt::Screen as VtScreen;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use std::collections::BTreeMap;

fn hint(theme: &Theme) -> Style {
    Style::default().fg(theme.overlay0).bg(theme.panel_bg)
}

fn key(theme: &Theme) -> Style {
    Style::default().fg(theme.accent).bg(theme.panel_bg)
}

fn name(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.accent)
        .bg(theme.panel_bg)
        .add_modifier(Modifier::BOLD)
}

fn body(theme: &Theme) -> Style {
    Style::default().fg(theme.text).bg(theme.panel_bg)
}

fn dim(theme: &Theme) -> Style {
    Style::default().fg(theme.overlay0).bg(theme.panel_bg)
}

fn value(theme: &Theme) -> Style {
    Style::default().fg(theme.accent).bg(theme.panel_bg)
}

fn warn(theme: &Theme) -> Style {
    Style::default().fg(theme.yellow).bg(theme.panel_bg)
}

fn error_style(theme: &Theme) -> Style {
    Style::default().fg(theme.red).bg(theme.panel_bg)
}

/// A panel background behind an overlay, so the workspace never shows through.
fn fill(buffer: &mut Buffer, rect: Rect, theme: &Theme) {
    Clear.render(rect, buffer);
    buffer.set_style(rect, theme.panel());
}

/// The overlay frame shared by every panel.
fn frame<'a>(rect: Rect, title: Line<'a>, theme: &Theme) -> (Block<'a>, Rect) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay0).bg(theme.panel_bg))
        .style(theme.panel())
        .title(title);
    let inner = block.inner(rect);
    (block, inner)
}

/// The command palette overlay: a query line, the filtered commands and their
/// configured prefix key hints. It reuses the command table; it never writes
/// the layout or the shared document itself.
pub(super) fn draw_palette(
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
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" palette ", name(theme)),
        Span::styled("· ", hint(theme)),
        Span::styled("Enter", key(theme)),
        Span::styled(" run · ", hint(theme)),
        Span::styled("Esc", key(theme)),
        Span::styled(" close ", hint(theme)),
    ]);
    let (block, inner) = frame(rect, title, theme);
    block.render(rect, buffer);
    if inner.width < 4 || inner.height < 2 {
        return None;
    }
    let query = state.line.text();
    let before = state.line.before();
    let head = format!("› {query}");
    Paragraph::new(Line::from(Span::styled(
        head,
        Style::default()
            .fg(theme.accent)
            .bg(theme.panel_bg)
            .add_modifier(Modifier::BOLD),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let rows = super::super::palette::filter(&super::super::palette::entries(&ui.keymap), &query);
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
        Paragraph::new(Line::from(Span::styled("no matching commands", dim(theme))))
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
                    theme.selected_row()
                } else {
                    body(theme)
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

/// The on-demand tab selector: the active space's tabs with their index,
/// name, pane count and active marker. It is the compatibility path for an
/// old multi-tab space and the only tab chrome; it appears only when summoned
/// with `prefix T`, never by itself.
pub(super) fn draw_tabs(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &super::super::model::Workspace,
    ui: &UiState,
    theme: &Theme,
) {
    let Some(state) = ui.tabs else {
        return;
    };
    let Some(space) = workspace.space(state.space) else {
        return;
    };
    if area.width < 16 || area.height < 5 {
        return;
    }
    let width = area.width.saturating_sub(4).min(64);
    let height = (space.tabs.len() as u16 + 3)
        .min(area.height.saturating_sub(2))
        .max(5);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + 1,
        width,
        height,
    );
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" tabs ", name(theme)),
        Span::styled(format!("· {} · ", space.name), hint(theme)),
        Span::styled("Enter", key(theme)),
        Span::styled(" switch · ", hint(theme)),
        Span::styled("Esc", key(theme)),
        Span::styled(" close ", hint(theme)),
    ]);
    let (block, inner) = frame(rect, title, theme);
    block.render(rect, buffer);
    if inner.width < 4 || inner.height < 2 {
        return;
    }
    let active = space.tabs.len();
    // The hint is shortened and then ellipsized to the popup's inner width:
    // it must end inside the border, never be hard-cut by it.
    let hint = format!(
        " {active} {} · j/k · g/G first/last · Enter switch · Esc close",
        if active == 1 { "tab" } else { "tabs" }
    );
    let hint = truncate(&hint, inner.width as usize);
    Paragraph::new(Line::from(Span::styled(hint, dim(theme))))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let list = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    if list.height == 0 || space.tabs.is_empty() {
        return;
    }
    let visible = (list.height as usize).min(space.tabs.len());
    let selected = state.selected.min(space.tabs.len() - 1);
    let offset = selected.saturating_add(1).saturating_sub(visible);
    let lines: Vec<Line> = space
        .tabs
        .iter()
        .skip(offset)
        .take(visible)
        .enumerate()
        .map(|(index, tab)| {
            let is_selected = offset + index == selected;
            let style = if is_selected {
                theme.selected_row()
            } else {
                body(theme)
            };
            let marker = if tab.id == space.active { "●" } else { " " };
            let panes = tab.panes().len();
            let count = format!("{panes} {}", if panes == 1 { "pane" } else { "panes" });
            let left = format!(" {marker} {}  {}", offset + index + 1, tab.name);
            let pad = (list.width as usize).saturating_sub(
                Span::raw(left.as_str()).width() + Span::raw(count.as_str()).width() + 1,
            );
            Line::from(Span::styled(
                format!("{left}{}{count} ", " ".repeat(pad)),
                style,
            ))
        })
        .collect();
    Paragraph::new(lines).render(list, buffer);
}

/// The peek overlay: the agent's core facts over its local VT. It is
/// read-only; any key closes it.
pub(super) fn draw_peek(
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
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" peek ", name(theme)),
        Span::styled("· ", hint(theme)),
        Span::styled("any key", key(theme)),
        Span::styled(" closes ", hint(theme)),
    ]);
    let (block, inner) = frame(rect, title, theme);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut facts = vec![
        Span::styled(row.title.clone(), value(theme)),
        Span::styled(" · ", hint(theme)),
        Span::styled(row.address.clone(), hint(theme)),
        Span::styled(" · ", hint(theme)),
        Span::styled(
            row.word.clone(),
            Style::default()
                .fg(theme.status_word(&row.word))
                .bg(theme.panel_bg),
        ),
        Span::styled(format!(" · turn {}", row.turn), hint(theme)),
    ];
    if row.pinned {
        facts.push(Span::styled(" · pinned", hint(theme)));
    }
    if !row.seen {
        facts.push(Span::styled(" · unseen", hint(theme)));
    }
    if !row.app().is_empty() {
        facts.push(Span::styled(format!(" · {}", row.app()), hint(theme)));
    }
    if let Some(progress) = row.status.as_ref().and_then(|status| status.progress) {
        facts.push(Span::styled(format!(" · {progress}%"), hint(theme)));
    }
    if let Some(kind) = row
        .status
        .as_ref()
        .and_then(|status| status.kind.as_deref())
        .filter(|kind| !kind.is_empty())
    {
        facts.push(Span::styled(format!(" · {kind}"), hint(theme)));
    }
    if let Some(msg) = row
        .status
        .as_ref()
        .map(|status| status.msg.as_str())
        .filter(|msg| !msg.is_empty())
    {
        facts.push(Span::styled(format!(" · {msg}"), hint(theme)));
    }
    Paragraph::new(Line::from(facts)).render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
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
        Paragraph::new(Line::from(Span::styled(line, dim(theme))))
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
        None => Paragraph::new(Span::styled("terminal not attached yet", dim(theme)))
            .render(grid, buffer),
    }
}

/// The Core-wide terminal manager: a modal list of every live terminal with
/// its marker, word, age and address, a filter/count line and the local
/// selection. It draws only the rows the runtime derived from core; it never
/// reads a screen or infers a status.
pub(super) fn draw_manager(
    buffer: &mut Buffer,
    area: Rect,
    ui: &UiState,
    rows: &[ManagerRow],
    theme: &Theme,
) -> Option<(u16, u16)> {
    if !ui.manager.open {
        return None;
    }
    let rect = super::manager_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let query = ui.manager.filter.text();
    let filtered = manager::filter(rows, &query);
    let total = rows.len();
    let shown = rows.iter().filter(|row| row.shown).count();
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" terminal manager ", name(theme)),
        Span::styled(format!("· {shown} shown · {total} live · "), hint(theme)),
        Span::styled("Enter", key(theme)),
        Span::styled(" show/hide · ", hint(theme)),
        Span::styled("p", key(theme)),
        Span::styled(" preview · ", hint(theme)),
        Span::styled("/", key(theme)),
        Span::styled(" filter · ", hint(theme)),
        Span::styled("Esc", key(theme)),
        Span::styled(" close ", hint(theme)),
    ]);
    let (block, inner) = frame(rect, title, theme);
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
    Paragraph::new(Line::from(Span::styled(head, value(theme))))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);

    if inner.height > 1 {
        let list = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        if total == 0 {
            Paragraph::new(Line::from(Span::styled(
                " no live terminals in core",
                dim(theme),
            )))
            .render(list, buffer);
        } else if filtered.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                " no terminals match the filter",
                dim(theme),
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
                        theme.selected_row()
                    } else {
                        body(theme)
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
pub(super) fn file_hint(ui: &UiState, pane: Option<&Pane>) -> &'static str {
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
pub(super) fn draw_file(
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
    fill(buffer, rect, theme);
    let title = if ui.files.prompt.is_some() {
        Line::from(vec![
            Span::styled(" file ", name(theme)),
            Span::styled("· ", hint(theme)),
            Span::styled("Enter", key(theme)),
            Span::styled(" read · ", hint(theme)),
            Span::styled("Esc", key(theme)),
            Span::styled(" close ", hint(theme)),
        ])
    } else if pane.is_some_and(|pane| pane.conflict) {
        Line::from(vec![
            Span::styled(" file diff ", warn(theme)),
            Span::styled("· changed on disk · ", hint(theme)),
            Span::styled("o", key(theme)),
            Span::styled(" overwrite · ", hint(theme)),
            Span::styled("q", key(theme)),
            Span::styled(" discard ", hint(theme)),
        ])
    } else if pane.is_some_and(|pane| pane.diff.is_some()) {
        Line::from(vec![
            Span::styled(" file diff ", name(theme)),
            Span::styled("· ", hint(theme)),
            Span::styled("w", key(theme)),
            Span::styled(" write · ", hint(theme)),
            Span::styled("q", key(theme)),
            Span::styled(" discard · ", hint(theme)),
            Span::styled("Esc", key(theme)),
            Span::styled(" close ", hint(theme)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" file ", name(theme)),
            Span::styled("· ", hint(theme)),
            Span::styled("e", key(theme)),
            Span::styled(" edit · ", hint(theme)),
            Span::styled("Esc", key(theme)),
            Span::styled(" close ", hint(theme)),
        ])
    };
    let (block, inner) = frame(rect, title, theme);
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
        rows.push(Line::from(Span::styled(facts, dim(theme))));
        let label = " path: ";
        let text = line.text();
        rows.push(Line::from(vec![
            Span::styled(label, value(theme)),
            Span::styled(text, body(theme).add_modifier(Modifier::BOLD)),
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
            rows.push(Line::from(Span::styled(error.clone(), error_style(theme))));
        }
        rows.push(Line::from(Span::styled(
            "a path, not a directory; local or a machine's path",
            dim(theme),
        )));
        Paragraph::new(rows).render(inner, buffer);
        return Some(cursor);
    }

    let Some(pane) = pane else {
        Paragraph::new(Span::styled("no file open", dim(theme))).render(inner, buffer);
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
        value(theme).add_modifier(Modifier::BOLD),
    )));
    if let Some(error) = &ui.files.error {
        rows.push(Line::from(Span::styled(error.clone(), error_style(theme))));
    }

    if pane.diff.is_some() {
        for row in pane.visible_diff(inner.height.saturating_sub(rows.len() as u16) as usize) {
            let (mark, number, style) = match row.tag {
                DiffTag::Same => (" ", row.new.or(row.old).unwrap_or(0), dim(theme)),
                DiffTag::Add => (
                    "+",
                    row.new.unwrap_or(0),
                    Style::default().fg(theme.green).bg(theme.panel_bg),
                ),
                DiffTag::Remove => (
                    "-",
                    row.old.unwrap_or(0),
                    Style::default().fg(theme.red).bg(theme.panel_bg),
                ),
            };
            rows.push(Line::from(Span::styled(
                format!("{mark} {number:>5} {}", row.text),
                style,
            )));
        }
    } else if let Some(viewer) = &pane.viewer {
        let height = inner.height.saturating_sub(rows.len() as u16) as usize;
        if viewer.window.is_empty() {
            rows.push(Line::from(Span::styled("empty file", dim(theme))));
        } else {
            for (number, text) in viewer.visible(height) {
                rows.push(Line::from(Span::styled(
                    format!("  {number:>5} {text}"),
                    body(theme),
                )));
            }
        }
    }
    Paragraph::new(rows).render(inner, buffer);
    None
}

/// The read-only observer preview: a border and title, a facts line, and the
/// local VT fed by the observer stream. It never shows a caret and never
/// offers input.
pub(super) fn draw_preview(
    buffer: &mut Buffer,
    area: Rect,
    preview: &PreviewState,
    screen: Option<&VtScreen>,
    theme: &Theme,
) {
    let rect = super::preview_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" preview ", name(theme)),
        Span::styled(
            format!("· {} · {} ", preview.title, preview.address),
            hint(theme),
        ),
    ]);
    let (block, inner) = frame(rect, title, theme);
    block.render(rect, buffer);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    Paragraph::new(Line::from(Span::styled(
        " read-only observer · no keys, mouse or resize are sent · any key closes",
        dim(theme),
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let grid = super::preview_grid(area);
    if grid.width == 0 || grid.height == 0 {
        return;
    }
    match screen {
        Some(screen) => crate::screen::view::draw_buffer(buffer, grid, screen),
        None => Paragraph::new(Span::styled("waiting for the first frame…", dim(theme)))
            .render(grid, buffer),
    }
}

/// The help overlay: the golden-tested documented key map.
pub(super) fn draw_help(buffer: &mut Buffer, area: Rect, theme: &Theme) {
    let width = (HELP.iter().map(|line| line.len()).max().unwrap_or(0) as u16 + 4).min(area.width);
    let height = (HELP.len() as u16 + 2).min(area.height);
    if width == 0 || height == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(x, y, width, height);
    fill(buffer, rect, theme);
    let title = Line::from(Span::styled(" help ", name(theme)));
    let (block, inner) = frame(rect, title, theme);
    block.render(rect, buffer);
    Paragraph::new(
        HELP.iter()
            .map(|line| Line::from(Span::styled(*line, body(theme))))
            .collect::<Vec<_>>(),
    )
    .style(theme.panel())
    .render(inner, buffer);
}

/// The machine manager's panel: core's machine rows, the add/rename prompt,
/// the removal confirmation and core's last refusal.
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
    let rect = super::manager_rect(area);
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let query = ui.machines.filter.text();
    let filtered = machines::filter(rows, &query);
    let total = rows.len();
    let connected = rows.iter().filter(|row| row.state == "connected").count();
    fill(buffer, rect, theme);
    let title = Line::from(vec![
        Span::styled(" machine manager ", name(theme)),
        Span::styled(format!("· {connected}/{total} connected · "), hint(theme)),
        Span::styled("Enter", key(theme)),
        Span::styled(" connect · ", hint(theme)),
        Span::styled("a", key(theme)),
        Span::styled(" add · ", hint(theme)),
        Span::styled("r", key(theme)),
        Span::styled(" rename · ", hint(theme)),
        Span::styled("d", key(theme)),
        Span::styled(" remove · ", hint(theme)),
        Span::styled("n", key(theme)),
        Span::styled(" new terminal · ", hint(theme)),
        Span::styled("/", key(theme)),
        Span::styled(" filter · ", hint(theme)),
        Span::styled("Esc", key(theme)),
        Span::styled(" close ", hint(theme)),
    ]);
    let (block, inner) = frame(rect, title, theme);
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
    Paragraph::new(Line::from(Span::styled(head, value(theme))))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);

    let mut cursor = None;
    if inner.height > 1 {
        let line = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        if let Some(prompt) = &ui.machines.prompt {
            let text = prompt.line.text();
            Paragraph::new(Line::from(vec![
                Span::styled(prompt.label(), value(theme)),
                Span::styled(text, body(theme).add_modifier(Modifier::BOLD)),
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
                warn(theme).add_modifier(Modifier::BOLD),
            )))
            .render(line, buffer);
        } else if let Some(error) = &ui.machines.error {
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                error_style(theme),
            )))
            .render(line, buffer);
        } else {
            Paragraph::new(Line::from(Span::styled(
                " target is passed to core unchanged · core names the machine · \
                 j/k move · g/G first/last",
                dim(theme),
            )))
            .render(line, buffer);
        }
    }

    if inner.height > 2 {
        let list = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        if total == 0 {
            Paragraph::new(Line::from(Span::styled(
                " no machines · a adds one by target (for example user@host:port)",
                dim(theme),
            )))
            .render(list, buffer);
        } else if filtered.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                " no machines match the filter",
                dim(theme),
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
                        theme.selected_row()
                    } else {
                        body(theme)
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
