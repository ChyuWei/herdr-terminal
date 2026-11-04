//! Composing the workspace into a ratatui buffer.
//!
//! Pure presentation: the runtime supplies one VT snapshot per visible pane.
//! On a wide terminal the space rail takes the left edge and the panes start
//! at the first row; on a narrow one the rail is hidden behind a drawer and a
//! single pane-nav row maps the tab. Every pane carries its identity in its
//! top border row. One bottom line spans the whole viewport with the mode,
//! the focused terminal and the context hints. Borders use the pane
//! rectangle; the terminal grid uses its inner rectangle. No core, VT or
//! terminal is touched here.

use super::agents::AgentRow;
use super::file::Pane;
use super::geometry::{self, Rects};
use super::panenav;
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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::BTreeMap;

/// The help text. It is also the golden-tested documented key map.
pub const HELP: &[&str] = &[
    "prefix Ctrl-b      ? help          q detach   any key closes help",
    "c new tab          C new space     T switch layout/tab (compatibility)",
    "n / p tab          N / P space     1..9 select tab",
    "v split left/right s split top/bottom",
    "h/j/k/l focus      H/J/K/L move",
    "z zoom             x close",
    "e keep + close     X kill + close",
    ", rename tab       . rename space",
    "a rail / drawer    i peek agent    g/G next/prev agent",
    "rail: j/k move  g/G first/last  Enter switch/jump  Esc/a close",
    "p pin / unpin  u mark unread  i peek  any key closes peek",
    "tabs: j/k move  Enter switch  Esc/q close",
    "t terminal manager (all live terminals in core)",
    "manager: j/k move  g/G first/last  Enter show/hide",
    "/ filter  p preview (read-only)  Esc close manager",
    "m machine manager · Enter connect · a add · r rename · d remove",
    "machines: n new terminal there · / filter · Esc close · add takes target only",
    "o file viewer       e edit in $VISUAL/$EDITOR, diff before write",
    "file: Enter read · e edit · w write · q discard · Esc close",
    "r resize mode (wide and unzoomed)",
    "resize: h/l width  j/k height  Shift for 5  Esc/q done",
    "Ctrl-] detach      prefix twice sends Ctrl-b",
    ": palette          f search scrollback",
    "[ select mode      y copy selection",
    "selection: drag or h/j/k/l or w/b/e, y copy, Esc cancel",
    "other keys leave selection; paste is ignored while selecting",
    "Shift-PgUp/PgDn scroll history      Shift-Home/End top/bottom",
    "drag a split gap to resize          wheel scrolls local history",
    "narrow: prefix a opens the rail over the panes; Esc closes it",
    "mouse: click a space, agent, pane or chip to focus",
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
    /// The focused terminal's identity for the bottom line: the address and
    /// whatever core reported about it, joined with ` · `.
    pub identity: Option<&'a str>,
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
            identity: None,
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
    buffer.set_style(area, Style::default().bg(theme.page));
    draw_panes(
        buffer,
        workspace,
        &rects,
        view.screens,
        view.selections,
        view.rows,
        theme,
    );
    draw_pane_nav(buffer, workspace, rects.pane_nav, view.rows, theme);
    if rects.panes.is_empty() && rects.workspace.height > 0 {
        draw_empty(buffer, rects.workspace, theme);
    }
    // The rail is drawn beside the panes when wide; on a narrow terminal the
    // same model is an on-demand drawer over them.
    let rail = if rects.rail.width > 0 {
        rects.rail
    } else if ui.sidebar.open {
        super::sidebar::drawer_rect(area)
    } else {
        Rect::new(area.x, area.y, 0, 0)
    };
    if rail.width > 0 {
        sidebar::draw_sidebar(buffer, rail, workspace, ui, view.rows, theme);
    }
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
    if ui.tabs.is_some() {
        overlays::draw_tabs(buffer, area, workspace, ui, theme);
        rendered.focused = None;
        rendered.prompt_cursor = None;
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

/// The narrow pane-nav: one compact chip per pane of the active tab, in split
/// tree order. The row is the map of the tab while only the focused pane is
/// drawn; every chip is clickable, and a long list scrolls a whole-chip
/// window so focus stays visible and the row never overflows.
fn draw_pane_nav(
    buffer: &mut Buffer,
    workspace: &Workspace,
    area: Rect,
    rows: &[AgentRow],
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    buffer.set_style(area, Style::default().bg(theme.bar));
    let entries = panenav::entries(workspace, rows);
    let Some(tab) = workspace.active_tab() else {
        return;
    };
    for chip in panenav::layout(&entries, tab.focused, area) {
        let (base, word) = match chip.word.as_deref() {
            Some(word) => (
                chip.label
                    .strip_suffix(word)
                    .map(|base| base.trim_end().to_owned())
                    .unwrap_or_else(|| chip.label.clone()),
                Some(word),
            ),
            None => (chip.label.clone(), None),
        };
        let (text, background) = if chip.focused {
            (
                Style::default()
                    .fg(theme.bar)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                theme.accent,
            )
        } else {
            (Style::default().fg(theme.muted).bg(theme.bar), theme.bar)
        };
        let mut spans = vec![Span::styled(base, text)];
        if let Some(word) = word {
            let color = if chip.focused || word.starts_with("exited ") {
                theme.bar
            } else {
                theme.status_word(word)
            };
            spans.push(Span::styled(
                format!(" {word}"),
                Style::default().fg(color).bg(background),
            ));
        }
        Paragraph::new(Line::from(spans)).render(chip.rect, buffer);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panes(
    buffer: &mut Buffer,
    workspace: &Workspace,
    rects: &Rects,
    screens: &BTreeMap<PaneId, VtScreen>,
    selections: &BTreeMap<PaneId, Vec<(u16, u16, u16)>>,
    rows: &[AgentRow],
    theme: &Theme,
) {
    let focused = workspace.focused_pane();
    let tab = workspace.active_tab();
    for (pane_id, rect) in &rects.panes {
        let Some(pane) = workspace.pane(*pane_id) else {
            continue;
        };
        let is_focused = focused == Some(*pane_id);
        let border = Style::default()
            .fg(if is_focused { theme.accent } else { theme.line })
            .bg(theme.panel_bg);
        Block::new()
            .borders(Borders::ALL)
            .border_style(border)
            .render(*rect, buffer);
        let word = panenav::word(&pane.status, rows, *pane_id);
        draw_pane_header(
            buffer,
            *rect,
            pane,
            word.as_deref(),
            geometry::pane_number(tab, *pane_id).unwrap_or(1),
            is_focused,
            theme,
        );
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

/// The identity row inside a pane's top border: focused marker, pane number
/// and name on the left; the OSC 7501 word and the address on the right. The
/// address is dropped first, then the title detail; the number, name and word
/// survive longest. An exited pane shows the client's own `exited N` fact in
/// red instead of a status word.
fn draw_pane_header(
    buffer: &mut Buffer,
    rect: Rect,
    pane: &super::model::Pane,
    word: Option<&str>,
    number: usize,
    focused: bool,
    theme: &Theme,
) {
    if rect.width <= 2 || rect.height == 0 {
        return;
    }
    let area = Rect::new(
        rect.x.saturating_add(1),
        rect.y,
        rect.width.saturating_sub(2),
        1,
    );
    let background = if focused {
        theme.active_bg
    } else {
        theme.panel_bg
    };
    buffer.set_style(area, Style::default().bg(background));
    // The block drew a border rule through this row; the identity replaces it
    // with a clean band before the left and right segments are drawn.
    for x in area.x..area.x.saturating_add(area.width) {
        if let Some(cell) = buffer.cell_mut((x, area.y)) {
            cell.set_symbol(" ")
                .set_style(Style::default().bg(background));
        }
    }
    let name = pane.name.clone();
    // A name that is just the address is not worth repeating: the readable
    // title leads and the address stays in the right segment. When there is
    // no title either, the left identity is only the number and the address
    // is still shown once, on the right. A name the person chose keeps both.
    let detail = if name == pane.address {
        if pane.title.is_empty() || pane.title == name {
            String::new()
        } else {
            pane.title.clone()
        }
    } else if pane.title.is_empty() || pane.title == pane.name {
        name.clone()
    } else {
        format!("{name}: {}", pane.title)
    };
    let (word, word_color) = match pane.exit {
        Some(code) => (format!("exited {code}"), theme.red),
        None => match word {
            Some(word) => (word.to_owned(), theme.status_word(word)),
            None => (String::new(), theme.muted),
        },
    };
    let has_word = !word.is_empty();
    let address = pane.address.as_str();
    let width = area.width as usize;
    // The marker, a space, the two-digit number and its trailing space.
    let fixed = 5;
    let gap = 1;
    let word_width = span_width(&word);
    let address_width = if address.is_empty() {
        0
    } else {
        usize::from(has_word) + span_width(address)
    };
    // The gap before the right segment is only spent when one exists.
    let room = |right: usize| width.saturating_sub(fixed + usize::from(right > 0) * gap + right);
    // Prefer the richest header that still leaves room to read a name.
    let stages = [
        (
            detail.clone(),
            word_width + address_width,
            has_word,
            !address.is_empty(),
        ),
        (detail, word_width, has_word, false),
        (name.clone(), word_width, has_word, false),
        (name.clone(), 0, false, false),
    ];
    let (mut label, right_width, show_word, show_address) = stages
        .into_iter()
        .find(|(candidate, right, _, _)| {
            width >= fixed + usize::from(*right > 0) * gap + right
                && room(*right) >= span_width(candidate)
        })
        .unwrap_or((name, 0, false, false));
    label = panenav::truncate(&label, room(right_width));

    let marker = if focused { "›" } else { " " };
    let marker_style = Style::default()
        .fg(if focused { theme.accent } else { theme.dim })
        .bg(background);
    let number_style = Style::default()
        .fg(if focused { theme.accent } else { theme.muted })
        .bg(background)
        .add_modifier(if focused {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let name_style = Style::default()
        .fg(if focused { theme.mint } else { theme.subtext0 })
        .bg(background)
        .add_modifier(if focused {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let left_width = width.saturating_sub(right_width + usize::from(right_width > 0) * gap);
    let left = Rect::new(area.x, area.y, left_width as u16, 1);
    Paragraph::new(Line::from(vec![
        Span::styled(marker.to_owned(), marker_style),
        Span::styled(format!(" {number:02} "), number_style),
        Span::styled(label, name_style),
    ]))
    .render(left, buffer);
    if right_width == 0 {
        return;
    }
    let right = Rect::new(
        area.x
            .saturating_add(left_width as u16)
            .saturating_add(gap as u16),
        area.y,
        right_width as u16,
        1,
    );
    let mut spans = Vec::new();
    if show_word {
        spans.push(Span::styled(
            word,
            Style::default().fg(word_color).bg(background),
        ));
    }
    if show_address {
        if show_word {
            spans.push(Span::styled(
                " ",
                Style::default().fg(theme.dim).bg(background),
            ));
        }
        spans.push(Span::styled(
            address.to_owned(),
            Style::default().fg(theme.dim).bg(background),
        ));
    }
    Paragraph::new(Line::from(spans)).render(right, buffer);
}

fn span_width(text: &str) -> usize {
    Span::raw(text).width()
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

/// Draws the single bottom line: the mode chip, the focused terminal's
/// identity and the context hints. A rename prompt or the search bar replaces
/// the line. The left side is drawn first, so at a narrow width the hints are
/// what disappear; the mode and the focused identity stay.
fn draw_status(
    buffer: &mut Buffer,
    workspace: &Workspace,
    ui: &UiState,
    area: Rect,
    theme: &Theme,
    view: &View<'_>,
) -> Option<(u16, u16)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    if let Some(search) = &ui.search {
        return draw_search(buffer, area, search, theme);
    }
    let background = theme.bar;
    buffer.set_style(area, Style::default().bg(background));
    let hint = Style::default().fg(theme.muted).bg(background);
    let value = Style::default().fg(theme.text).bg(background);
    let dim = Style::default().fg(theme.dim).bg(background);
    if let Some(prompt) = &ui.prompt {
        let text = prompt.text();
        // The prompt label itself is the chip: accent background, bar text.
        let chip = Style::default()
            .fg(theme.panel_bg)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::styled(prompt.label(), chip),
            Span::styled(text, value.add_modifier(Modifier::BOLD)),
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

    let mode = Mode::current(workspace, ui);
    let chip = Style::default()
        .fg(theme.panel_bg)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut left = vec![Span::styled(format!(" {} ", mode.label()), chip)];
    // The runtime supplies the reported identity; a bare frame (tests, simple
    // callers) still names the focused terminal from the pane record.
    let identity = view.identity.map(str::to_owned).or_else(|| {
        workspace
            .focused_pane()
            .and_then(|id| workspace.pane(id))
            .map(|pane| {
                let name = if pane.title.is_empty() || pane.title == pane.name {
                    pane.name.clone()
                } else {
                    format!("{}: {}", pane.name, pane.title)
                };
                if name == pane.address {
                    pane.address.clone()
                } else {
                    format!("{} · {name}", pane.address)
                }
            })
    });
    match &identity {
        Some(identity) => left.push(Span::styled(format!(" {identity}"), value)),
        None => left.push(Span::styled(" empty", dim)),
    }
    if let Some(message) = &ui.message {
        left.push(Span::styled(format!(" · {message}"), hint));
    }

    let (mut right, mut right_style) =
        right_hint(workspace, ui, mode, view.files, theme, background);
    let scrolled = view
        .scroll_hint
        .filter(|_| matches!(ui.connection, Connection::Live));
    if let Some(scroll) = scrolled {
        // A scroll position is state, not a hint: it replaces the context
        // hints instead of competing with them for the last columns.
        right = scroll.to_owned();
        right_style = hint;
    }
    let area_width = area.width as usize;
    let left_width: usize = left.iter().map(Span::width).sum();
    Paragraph::new(Line::from(left)).render(area, buffer);
    // The hints are the first thing to go when the line is tight. A scroll
    // position is the exception: it is truncated to the free columns so the
    // user can still tell that the view is not live.
    let mut right_width = span_width(&right);
    if scrolled.is_some() && left_width + 1 + right_width > area_width {
        let available = area_width.saturating_sub(left_width + 2);
        right = super::panenav::truncate(&right, available);
        right_width = span_width(&right);
    }
    if right_width + 1 < area_width && left_width + 1 + right_width <= area_width {
        let hint_area = Rect::new(
            area.x.saturating_add((area_width - right_width - 1) as u16),
            area.y,
            (right_width + 1) as u16,
            1,
        );
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(right, right_style),
        ]))
        .render(hint_area, buffer);
    }
    None
}

/// The one mode the bottom line names, most specific first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    Prefix,
    Copy,
    Resize,
    Zoom,
}

impl Mode {
    fn current(workspace: &Workspace, ui: &UiState) -> Mode {
        if ui.resize {
            Mode::Resize
        } else if ui.selecting.is_some() {
            Mode::Copy
        } else if ui.prefix {
            Mode::Prefix
        } else if workspace
            .active_tab()
            .and_then(|tab| tab.zoomed)
            .is_some_and(|pane| workspace.active_tab().is_some_and(|tab| tab.contains(pane)))
        {
            Mode::Zoom
        } else {
            Mode::Normal
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Prefix => "PREFIX",
            Mode::Copy => "COPY",
            Mode::Resize => "RESIZE",
            Mode::Zoom => "ZOOM",
        }
    }
}

/// The right side of the bottom line: a mode's essential keys, or the current
/// context (connection, overlay or the default hints). Transient overrides
/// keep priority, as before.
fn right_hint(
    _workspace: &Workspace,
    ui: &UiState,
    mode: Mode,
    files: Option<&Pane>,
    theme: &Theme,
    background: Color,
) -> (String, Style) {
    if let Connection::Reconnecting { attempt } = ui.connection {
        let text = if attempt == 0 {
            "reconnecting…".to_owned()
        } else {
            format!("reconnecting (attempt {attempt})")
        };
        return (text, Style::default().fg(theme.yellow).bg(background));
    }
    let hint = Style::default().fg(theme.muted).bg(background);
    match mode {
        Mode::Prefix => {
            return ("esc cancel · ^b prefix · ? keys".to_owned(), hint);
        }
        Mode::Copy => {
            return ("h/j/k/l move · y copy · Esc cancel".to_owned(), hint);
        }
        Mode::Resize => {
            return (
                "h/l width · j/k height · Shift 5 · Esc done".to_owned(),
                hint,
            );
        }
        Mode::Normal | Mode::Zoom => {}
    }
    let text = if ui.preview.is_some() {
        "preview · read-only · any key closes"
    } else if ui.manager.open {
        "terminals · Enter show/hide · p preview · Esc close"
    } else if ui.machines.open {
        "machines · Enter connect · a add · n terminal · Esc close"
    } else if ui.files.open {
        return (overlays::file_hint(ui, files).to_owned(), hint);
    } else if ui.peek.is_some() {
        "peek · any key closes"
    } else if ui.sidebar.open {
        "rail · j/k move · Enter jump · Esc closes"
    } else if mode == Mode::Zoom {
        "^b z unzoom  ^b ? help"
    } else {
        "^b a rail  ^b z zoom  ^b ? help"
    };
    (text.to_owned(), hint)
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

#[cfg(test)]
mod tests;
