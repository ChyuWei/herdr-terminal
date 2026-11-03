use super::*;
use crate::theme::{ColorDepth, ThemeChoice, theme};
use crate::vt::Vt;
use crate::workspace::agents::{AgentRow, Priority, sort_rows};
use crate::workspace::commands::RenameTarget;
use crate::workspace::model::{Axis, PaneId, StartKind, Workspace};
use crate::workspace::state::{Line, PaletteState, Prompt, SearchState, UiState};
use ratatui::style::Color;
use std::collections::BTreeMap;

fn add(workspace: &mut Workspace, kind: StartKind, name: &str) -> PaneId {
    let request = workspace.start_request(kind);
    workspace.insert_terminal(&request, name.to_owned(), name.to_owned())
}

fn one_pane() -> (Workspace, PaneId) {
    let mut workspace = Workspace::new();
    let pane = add(&mut workspace, StartKind::Initial, "t1");
    (workspace, pane)
}

fn two_panes() -> (Workspace, PaneId, PaneId) {
    let (mut workspace, first) = one_pane();
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    (workspace, first, second)
}

/// Snapshots each visible pane's VT with the same "hi" text, so golden rows
/// show the terminal grid inside the border.
fn render_ws(workspace: &Workspace, ui: &UiState, width: u16, height: u16) -> (Buffer, Render) {
    let area = Rect::new(0, 0, width, height);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    for (pane, rect) in &rects.panes {
        let Some(inner) = geometry::inner(*rect) else {
            continue;
        };
        let mut vt = Vt::new(inner.width, inner.height).expect("vt");
        vt.feed(b"hi");
        screens.insert(*pane, vt.snapshot().expect("snapshot"));
    }
    let mut buffer = Buffer::empty(area);
    let rendered = render(&mut buffer, area, workspace, ui, &screens);
    (buffer, rendered)
}

fn lines(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn agent(
    pane: PaneId,
    address: &str,
    word: &str,
    priority: Priority,
    title: &str,
    app: &str,
) -> AgentRow {
    let kind = (word == "blocked").then(|| "permission".to_owned());
    let msg = if word == "blocked" {
        "allow?".to_owned()
    } else {
        String::new()
    };
    AgentRow {
        pane,
        address: address.to_owned(),
        title: title.to_owned(),
        name: address.to_owned(),
        word: word.to_owned(),
        priority,
        turn: if word == "done" { 4 } else { 3 },
        pinned: word == "done",
        seen: word != "done",
        seen_turn: Some(3),
        status: Some(crate::core::Status {
            state: word.to_owned(),
            kind,
            progress: (word == "blocked").then_some(42),
            app: app.to_owned(),
            title: title.to_owned(),
            msg,
        }),
        foreground: Some(crate::core::Foreground {
            pid: 7,
            name: app.to_owned(),
            argv: vec![app.to_owned()],
            cwd: "/work".into(),
        }),
        state: "live".into(),
        exit: None,
        order: (0, 0, 0),
    }
}

fn render_agents(
    workspace: &Workspace,
    ui: &UiState,
    width: u16,
    height: u16,
    rows: &[AgentRow],
) -> (Buffer, Render) {
    let area = Rect::new(0, 0, width, height);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    for (pane, rect) in &rects.panes {
        let Some(inner) = geometry::inner(*rect) else {
            continue;
        };
        let mut vt = Vt::new(inner.width, inner.height).expect("vt");
        vt.feed(if Some(*pane) == ui.peek {
            b"PEEKED"
        } else {
            b"hi"
        });
        screens.insert(*pane, vt.snapshot().expect("snapshot"));
    }
    let mut buffer = Buffer::empty(area);
    let theme = Theme::default();
    let empty = BTreeMap::new();
    let view = View {
        screens: &screens,
        rows,
        theme: &theme,
        selections: &empty,
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let rendered = render_with(&mut buffer, area, workspace, ui, &view);
    (buffer, rendered)
}

#[test]
fn a_single_pane_matches_the_hand_written_frame() {
    let (workspace, _) = one_pane();
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 40, 12);
    let rows = lines(&buffer);
    let blank_body = format!("│{}│", " ".repeat(38));
    let expected = vec![
        format!(" space 1  1 tab 1{}", " ".repeat(23)),
        format!("┌ t1 {}┐", "─".repeat(34)),
        format!("│hi{}│", " ".repeat(36)),
        blank_body.clone(),
        blank_body.clone(),
        blank_body.clone(),
        blank_body.clone(),
        blank_body.clone(),
        blank_body.clone(),
        blank_body,
        format!("└{}┘", "─".repeat(38)),
        format!(" space 1 / tab 1 / t1{}^b ? help", " ".repeat(10)),
    ];
    assert_eq!(rows, expected);
    assert_eq!(
        rendered.focused,
        Some((workspace.focused_pane().unwrap(), Rect::new(1, 2, 38, 8)))
    );
    assert_eq!(rendered.prompt_cursor, None);
}

#[test]
fn the_focused_border_is_highlighted_and_the_other_is_dim() {
    let (workspace, first, second) = two_panes();
    assert_eq!(workspace.focused_pane(), Some(second));
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 40, 12);
    assert_eq!(buffer[(0, 1)].fg, Color::DarkGray, "left pane, unfocused");
    assert_eq!(buffer[(21, 1)].fg, Color::Cyan, "right pane, focused");
    assert_eq!(rendered.focused, Some((second, Rect::new(22, 2, 17, 8))));
    let _ = first;
}

#[test]
fn the_active_tab_is_the_reversed_one() {
    let (mut workspace, _) = one_pane();
    add(&mut workspace, StartKind::Tab, "t2");
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 40, 12);
    let row: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
    let first = row.find("1 tab 1").expect("first tab");
    let second = row.find("2 tab 2").expect("second tab");
    assert!(
        !buffer[(first as u16, 0)]
            .modifier
            .contains(Modifier::REVERSED),
        "inactive tab is not highlighted: {row:?}"
    );
    assert!(
        buffer[(second as u16, 0)]
            .modifier
            .contains(Modifier::REVERSED),
        "active tab is highlighted: {row:?}"
    );
}

#[test]
fn zoom_shows_only_the_focused_pane_with_the_bars_intact() {
    let (mut workspace, _, second) = two_panes();
    assert!(workspace.toggle_zoom());
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 40, 12);
    let rows = lines(&buffer);
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.zoomed),
        Some(second)
    );
    assert!(rows[0].starts_with(" space 1  1 tab 1"));
    assert!(rows[1].starts_with("┌ t2 "));
    assert!(!rows.iter().any(|row| row.contains("t1")), "{rows:?}");
    assert!(rows[11].contains("^b ? help"));
    assert_eq!(rendered.focused.map(|(pane, _)| pane), Some(second));
}

#[test]
fn an_empty_workspace_says_how_to_start() {
    let workspace = Workspace::new();
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 40, 12);
    let rows = lines(&buffer);
    assert!(
        rows.iter()
            .any(|row| row.contains("no terminals in this tab")),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("prefix v: new pane")),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("prefix c: new tab")),
        "{rows:?}"
    );
    assert!(rows[11].contains("space 1 / tab 1 / empty"));
    assert_eq!(rendered.focused, None);
}

#[test]
fn the_rename_prompt_replaces_the_status_bar_and_places_the_cursor() {
    let (workspace, _) = one_pane();
    let space = workspace.active_space_id();
    let tab = workspace.active_tab_id().expect("tab");
    let mut ui = UiState {
        prompt: Some(Prompt::new(RenameTarget::Tab { space, tab }, "tab 1")),
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 40, 12);
    let rows = lines(&buffer);
    assert!(rows[11].contains("rename tab: tab 1"), "{rows:?}");
    assert_eq!(rendered.prompt_cursor, Some((17, 11)));
    assert!(!rows[11].contains("^b ? help"));

    let prompt = ui.prompt.as_mut().expect("prompt");
    prompt.edit(crate::workspace::state::Edit::Home);
    let (_, rendered) = render_ws(&workspace, &ui, 40, 12);
    assert_eq!(rendered.prompt_cursor, Some((12, 11)));
}

#[test]
fn help_is_an_overlay_listing_the_prefix_keys() {
    let (workspace, _) = one_pane();
    let ui = UiState {
        help: true,
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 100, 30);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("help"), "{text}");
    assert!(text.contains("prefix Ctrl-b"), "{text}");
    assert!(text.contains("e keep + close") && text.contains("X kill + close"));
    assert!(text.contains("a agents") && text.contains("i peek agent"));
    assert!(text.contains("g/G next/prev agent"));
    assert!(text.contains("sidebar: j/k move") && text.contains("p pin / unpin"));
    assert!(text.contains(": palette") && text.contains("f search scrollback"));
    assert!(text.contains("selection: drag or h/j/k/l"));
    assert!(text.contains("Shift-PgUp/PgDn scroll history"));
    assert!(text.contains("drag a split gap to resize"));
    assert!(text.contains("any key closes help"));
    assert_eq!(rendered.focused, None);
    assert_eq!(rendered.prompt_cursor, None);
}

#[test]
fn the_status_bar_shows_the_focused_title_status_and_message() {
    let (mut workspace, first) = one_pane();
    workspace.set_title(first, "vim".into());
    workspace.set_status(first, Some("working".into()));
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 60, 12);
    let row: String = (0..60).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(
        row.contains("space 1 / tab 1 / t1 · vim [working]"),
        "{row:?}"
    );

    let mut ui = UiState {
        message: Some("left t7 running (hc attach t7)".into()),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let row: String = (0..60).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.contains("left t7 running"), "{row:?}");

    ui.message = None;
    ui.prefix = true;
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let row: String = (0..60).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.contains("prefix ^b…"), "{row:?}");

    ui.prefix = false;
    ui.connection = Connection::Reconnecting { attempt: 2 };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 12);
    let row: String = (0..80).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.contains("reconnecting (attempt 2)"), "{row:?}");
}

#[test]
fn the_sidebar_shows_word_pin_title_facts_and_attention() {
    let (workspace, first, second) = two_panes();
    let mut rows = vec![
        agent(first, "t1", "blocked", Priority::Blocked, "Build", "claude"),
        agent(second, "t2", "done", Priority::Done, "Review", "codex"),
    ];
    sort_rows(&mut rows);
    assert_eq!(rows[0].address, "t2", "the pinned row sorts first");
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    ui.sidebar.selected = Some(second);
    let (buffer, rendered) = render_agents(&workspace, &ui, 80, 14, &rows);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("agents"), "{text}");
    assert!(text.contains("blocked"), "{text}");
    assert!(text.contains("Build") && text.contains("claude"), "{text}");
    assert!(
        text.contains("app: permission") || text.contains("permission"),
        "{text}"
    );
    assert!(text.contains("42%"), "{text}");
    assert!(text.contains("allow?"), "{text}");
    assert!(text.contains("t2") && text.contains("Review"), "{text}");
    assert!(text.contains("turn 4") && text.contains("unseen"), "{text}");
    assert!(
        text.contains("* done"),
        "pin marker on the pinned row: {text}"
    );
    assert!(
        text.contains("1 blocked · 1 done"),
        "attention count in the sidebar title: {text}"
    );
    assert_eq!(rendered.focused, None);
    assert_eq!(rendered.prompt_cursor, None);

    // The selected row is drawn reversed; the other is not.
    let (x, y) = (0..80)
        .flat_map(|x| (0..14).map(move |y| (x, y)))
        .find(|(x, y)| buffer[(*x, *y)].symbol() == "R" && y > &1)
        .unwrap_or((0, 0));
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::REVERSED),
        "selected row is highlighted at {x},{y}"
    );
}

#[test]
fn the_status_bar_shows_agent_turn_pin_and_seen_state() {
    let (workspace, pane) = one_pane();
    let rows = vec![agent(pane, "t1", "done", Priority::Done, "Review", "codex")];
    let ui = UiState::default();
    let (buffer, _) = render_agents(&workspace, &ui, 120, 12, &rows);
    let row: String = (0..120).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.contains("[done] · turn 4 · pinned · unseen"), "{row:?}");
    assert!(row.contains("codex"), "{row:?}");
    assert!(row.contains("1 done"), "attention count: {row:?}");
    assert!(row.contains("^b ? help"), "{row:?}");
}

#[test]
fn peek_shows_local_vt_and_core_facts_without_stealing_the_cursor() {
    let (workspace, pane) = one_pane();
    let rows = vec![agent(
        pane,
        "t1",
        "working",
        Priority::Working,
        "Build",
        "claude",
    )];
    let ui = UiState {
        peek: Some(pane),
        ..UiState::default()
    };
    let (buffer, rendered) = render_agents(&workspace, &ui, 80, 16, &rows);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("peek · any key closes"), "{text}");
    assert!(text.contains("t1") && text.contains("Build"), "{text}");
    assert!(
        text.contains("working") && text.contains("turn 3"),
        "{text}"
    );
    assert!(text.contains("claude"), "{text}");
    assert!(
        text.contains("fg claude · claude · /work"),
        "core foreground facts: {text}"
    );
    assert!(text.contains("PEEKED"), "the local VT is drawn: {text}");
    assert_eq!(rendered.focused, None);
    assert_eq!(rendered.prompt_cursor, None);
}

#[test]
fn the_palette_filters_the_command_table_and_shows_key_hints() {
    let (workspace, _) = one_pane();
    let state = PaletteState {
        line: Line::new("split"),
        ..PaletteState::default()
    };
    let ui = UiState {
        palette: Some(state),
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 80, 20);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("palette · Enter run · Esc close"), "{text}");
    assert!(text.contains("› split"), "{text}");
    assert!(text.contains("split left/right"), "{text}");
    assert!(text.contains("split top/bottom"), "{text}");
    assert!(text.contains("^b v"), "configured key hint: {text}");
    assert!(text.contains("^b s"), "configured key hint: {text}");
    assert!(!text.contains("new tab"), "the filter hides others: {text}");
    assert_eq!(rendered.focused, None);
    let (x, y) = rendered.prompt_cursor.expect("palette cursor");
    assert_eq!(y, 2, "the query line is inside the palette border");
    assert!(
        lines(&buffer)[y as usize].contains("split"),
        "cursor row: {y}"
    );
    assert_eq!(x, 12, "after '› ' and the query text");
}

#[test]
fn the_palette_says_when_nothing_matches() {
    let (workspace, _) = one_pane();
    let state = PaletteState {
        line: Line::new("zzzz"),
        ..PaletteState::default()
    };
    let ui = UiState {
        palette: Some(state),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 20);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("no matching commands"), "{text}");
}

#[test]
fn the_search_bar_shows_the_query_and_match_counts() {
    let (workspace, pane) = one_pane();
    let mut search = SearchState::new(pane);
    search.line = Line::new("hi");
    search.total = 2;
    search.selected = Some(0);
    search.complete = true;
    let ui = UiState {
        search: Some(search),
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 80, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.contains("search: hi"), "{row:?}");
    assert!(row.contains("1/2 matches"), "{row:?}");
    assert!(
        row.contains("Enter next") && row.contains("Esc close"),
        "{row:?}"
    );
    assert!(
        !row.contains("^b ? help"),
        "the search bar owns the status row"
    );
    assert_eq!(
        rendered.prompt_cursor,
        Some((11, 11)),
        "after ' search: ' plus the two-char query"
    );
    assert_eq!(rendered.focused, None);
}

#[test]
fn the_search_bar_reports_progress_and_an_empty_query() {
    let (workspace, pane) = one_pane();
    let search = SearchState::new(pane);
    let ui = UiState {
        search: Some(search),
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 80, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.contains("type to search scrollback"), "{row:?}");
    assert_eq!(rendered.prompt_cursor, Some((9, 11)), "after ' search: '");

    let mut search = SearchState::new(pane);
    search.line = Line::new("needle");
    search.pending = true;
    let ui = UiState {
        search: Some(search),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.contains("searching…"), "{row:?}");
}

#[test]
fn a_local_selection_paints_its_cells_with_the_theme_selection_color() {
    let (workspace, pane) = one_pane();
    let area = Rect::new(0, 0, 40, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    let mut vt = Vt::new(2, 1).expect("vt");
    vt.feed(b"hi");
    screens.insert(pane, vt.snapshot().expect("snapshot"));
    let mut selections = BTreeMap::new();
    selections.insert(pane, vec![(0, 0, 2)]);
    let theme = Theme::default();
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &selections,
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    let inner = geometry::inner(rects.pane(pane).expect("pane")).expect("inner");
    assert_eq!(buffer[(inner.x, inner.y)].symbol(), "h");
    assert_eq!(buffer[(inner.x, inner.y)].bg, theme.selection);
    assert_eq!(buffer[(inner.x + 1, inner.y)].bg, theme.selection);
    assert_ne!(buffer[(inner.x + 2, inner.y)].bg, theme.selection);
    assert_ne!(buffer[(inner.x, inner.y + 1)].bg, theme.selection);
}

#[test]
fn the_scroll_hint_replaces_the_idle_status_hint() {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 80, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    let inner = geometry::inner(rects.panes[0].1).expect("inner");
    let mut vt = Vt::new(inner.width, inner.height).expect("vt");
    vt.feed(b"hi");
    screens.insert(rects.panes[0].0, vt.snapshot().expect("snapshot"));
    let theme = Theme::default();
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &BTreeMap::new(),
        scroll_hint: Some("scroll 2/18 · Shift-PgDn live"),
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    let row = lines(&buffer)[11].clone();
    assert!(row.contains("scroll 2/18"), "{row:?}");
    assert!(row.contains("Shift-PgDn live"), "{row:?}");
}

#[test]
fn explicit_themes_style_the_chrome_consistently() {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 40, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    let inner = geometry::inner(rects.panes[0].1).expect("inner");
    let mut vt = Vt::new(inner.width, inner.height).expect("vt");
    vt.feed(b"hi");
    screens.insert(rects.panes[0].0, vt.snapshot().expect("snapshot"));
    let mut seen = Vec::new();
    for choice in [ThemeChoice::Dark, ThemeChoice::Light] {
        let theme = theme(choice, ColorDepth::TrueColor);
        let view = View {
            screens: &screens,
            rows: &[],
            theme: &theme,
            selections: &BTreeMap::new(),
            scroll_hint: None,
            manager: &[],
            machines: &[],
            preview: None,
            files: None,
        };
        let mut buffer = Buffer::empty(area);
        render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
        let space = buffer[(1, 0)].fg;
        let border = buffer[(0, 1)].fg;
        let status_bg = buffer[(0, 11)].bg;
        assert_eq!(space, theme.accent, "tab bar accent: {choice:?}");
        assert_eq!(border, theme.accent, "focused border: {choice:?}");
        assert_eq!(
            status_bg,
            theme.status.bg.expect("status background"),
            "status bar: {choice:?}"
        );
        seen.push((space, border, status_bg));
    }
    assert_ne!(seen[0], seen[1], "dark and light differ");
}

#[test]
fn the_terminal_grid_is_not_painted_by_the_theme() {
    let (workspace, pane) = one_pane();
    let area = Rect::new(0, 0, 40, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let inner = geometry::inner(rects.pane(pane).expect("pane")).expect("inner");
    let mut vt = Vt::new(inner.width, inner.height).expect("vt");
    vt.feed(b"hi");
    let mut screens = BTreeMap::new();
    screens.insert(pane, vt.snapshot().expect("snapshot"));
    let theme = theme(ThemeChoice::Light, ColorDepth::TrueColor);
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &BTreeMap::new(),
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    assert_eq!(buffer[(inner.x, inner.y)].symbol(), "h");
    assert_ne!(
        buffer[(inner.x, inner.y)].bg,
        theme.status.bg.expect("status background")
    );
}

#[test]
fn redrawing_a_scrolled_snapshot_shows_history_and_resumes_at_the_bottom() {
    let (workspace, pane) = one_pane();
    let area = Rect::new(0, 0, 40, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let inner = geometry::inner(rects.pane(pane).expect("pane")).expect("inner");
    let mut vt = Vt::new_with_scrollback(inner.width, inner.height, 100).expect("vt");
    for index in 0..100 {
        vt.feed(format!("LINE-{index:03}\r\n").as_bytes());
    }
    vt.scroll_lines(-20);
    let theme = Theme::default();
    let mut screens = BTreeMap::new();
    screens.insert(pane, vt.snapshot().expect("snapshot"));
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &BTreeMap::new(),
        scroll_hint: Some("scroll 20/60 · Shift-PgDn live"),
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    let started = std::time::Instant::now();
    for _ in 0..100 {
        buffer = Buffer::empty(area);
        render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    }
    let elapsed = started.elapsed();
    let text = lines(&buffer).join("\n");
    assert!(text.contains("LINE-079"), "history is drawn: {text}");
    assert!(
        !text.contains("LINE-099"),
        "live rows are below the viewport: {text}"
    );
    assert!(text.contains("scroll 20/60"), "scroll hint: {text}");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "100 redraws took {elapsed:?}"
    );

    vt.scroll_to_bottom();
    screens.insert(pane, vt.snapshot().expect("snapshot"));
    let stopped = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &BTreeMap::new(),
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &stopped);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("LINE-099"), "the live bottom is back: {text}");
    assert!(!text.contains("scroll 20/60"));
}

fn manager_row(address: &str, name: &str, word: &str, shown: bool) -> ManagerRow {
    ManagerRow {
        address: address.to_owned(),
        machine: manager::machine(address),
        name: name.to_owned(),
        title: String::new(),
        parent: None,
        word: word.to_owned(),
        cwd: "/work".into(),
        age: "2m".into(),
        created_ms: Some(2_000),
        shown,
    }
}

fn render_manager(ui: &UiState, rows: &[ManagerRow], width: u16, height: u16) -> (Buffer, Render) {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, width, height);
    let theme = Theme::default();
    let selections = BTreeMap::new();
    let screens = BTreeMap::new();
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &selections,
        scroll_hint: None,
        manager: rows,
        machines: &[],
        preview: None,
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    let rendered = render_with(&mut buffer, area, &workspace, ui, &view);
    (buffer, rendered)
}

#[test]
fn the_terminal_manager_lists_every_row_with_a_shown_marker() {
    let mut ui = UiState::default();
    ui.manager.open = true;
    ui.manager.selected = Some("m3/t9".into());
    let rows = vec![
        manager_row("t7", "worker", "blocked", true),
        manager_row("m3/t9", "builder", "working", false),
    ];
    let (buffer, rendered) = render_manager(&ui, &rows, 90, 18);
    let text = lines(&buffer).join("\n");
    for expected in [
        "terminal manager",
        "1 shown · 2 live",
        "* blocked",
        "t7",
        "worker",
        "  working",
        "m3/t9",
        "builder",
        "machine m3",
        "/work",
        "Enter show/hide",
    ] {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    assert!(rendered.focused.is_none(), "the manager covers the panes");
    assert!(rendered.prompt_cursor.is_none());
}

#[test]
fn the_manager_empty_and_no_match_states_say_so() {
    let mut ui = UiState::default();
    ui.manager.open = true;
    let (buffer, _) = render_manager(&ui, &[], 70, 14);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("no live terminals in core"), "{text}");
    assert!(text.contains("0 live"), "{text}");

    ui.manager.filter = Line::new("zzz");
    let rows = vec![manager_row("t7", "worker", "blocked", true)];
    let (buffer, _) = render_manager(&ui, &rows, 70, 14);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("no terminals match the filter"), "{text}");
    assert!(text.contains("0/1 match"), "{text}");
    assert!(
        !text.contains("worker"),
        "the hidden row is filtered: {text}"
    );
}

#[test]
fn the_manager_filter_owns_the_cursor_and_selection_moves_over_matches() {
    let mut ui = UiState::default();
    ui.manager.open = true;
    ui.manager.filtering = true;
    ui.manager.filter = Line::new("work");
    ui.manager.selected = Some("t9".into());
    let rows = vec![
        manager_row("t7", "worker", "blocked", true),
        {
            let mut other = manager_row("t8", "other", "idle", false);
            other.cwd = "/elsewhere".into();
            other
        },
        manager_row("t9", "workbench", "working", false),
    ];
    let (buffer, rendered) = render_manager(&ui, &rows, 80, 16);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("filter: work"), "{text}");
    assert!(
        text.contains("worker") && text.contains("workbench"),
        "{text}"
    );
    assert!(!text.contains("other"), "the filter is live: {text}");
    assert!(text.contains("2/3 match"), "{text}");
    assert!(rendered.prompt_cursor.is_some(), "the filter has a cursor");
    let (x, y) = rendered.prompt_cursor.expect("cursor");
    assert_eq!(y, manager_rect(Rect::new(0, 0, 80, 16)).y + 1);
    assert!(x > 0);
}

#[test]
fn the_preview_draws_a_read_only_border_title_and_terminal() {
    let ui = UiState {
        preview: Some(PreviewState {
            address: "m1/t9".into(),
            title: "worker".into(),
        }),
        ..UiState::default()
    };
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 80, 20);
    let grid = preview_grid(area);
    assert!(grid.width > 0 && grid.height > 0);
    let mut vt = Vt::new(grid.width, grid.height).expect("vt");
    vt.feed(b"PREVIEW");
    let screen = vt.snapshot().expect("snapshot");
    let theme = Theme::default();
    let selections = BTreeMap::new();
    let screens = BTreeMap::new();
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &selections,
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: Some(&screen),
        files: None,
    };
    let mut buffer = Buffer::empty(area);
    let rendered = render_with(&mut buffer, area, &workspace, &ui, &view);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("preview · worker · m1/t9"), "{text}");
    assert!(text.contains("read-only observer"), "{text}");
    assert!(text.contains("any key closes"), "{text}");
    assert!(text.contains("PREVIEW"), "the observer VT is drawn: {text}");
    assert!(rendered.focused.is_none());
    assert!(rendered.prompt_cursor.is_none());
}

// --- file overlay ----------------------------------------------------------

fn file_view<'a>(
    workspace: &'a Workspace,
    ui: &'a UiState,
    pane: Option<&'a crate::workspace::file::Pane>,
) -> (Buffer, Render) {
    let area = Rect::new(0, 0, 100, 30);
    let theme = Theme::default();
    let selections = BTreeMap::new();
    let screens = BTreeMap::new();
    let view = View {
        screens: &screens,
        rows: &[],
        theme: &theme,
        selections: &selections,
        scroll_hint: None,
        manager: &[],
        machines: &[],
        preview: None,
        files: pane,
    };
    let mut buffer = Buffer::empty(area);
    let rendered = render_with(&mut buffer, area, workspace, ui, &view);
    (buffer, rendered)
}

fn file_pane(
    path: &str,
    text: &str,
    first: usize,
    last: usize,
    lines: usize,
) -> crate::workspace::file::Pane {
    use crate::workspace::file::{Context, Pane, Window};
    Pane::viewing(
        path,
        Context::default(),
        Window {
            text: text.into(),
            first,
            last,
            lines,
        },
    )
}

#[test]
fn the_file_prompt_names_the_machine_cwd_and_path() {
    use crate::workspace::file::Context;
    use crate::workspace::state::FileState;
    let (workspace, _) = one_pane();
    let ui = UiState {
        files: FileState {
            open: true,
            context: Context {
                on: Some("alpha".into()),
                cwd: Some("/srv/app".into()),
            },
            prompt: Some(Line::new("/srv/app/")),
            error: None,
        },
        ..UiState::default()
    };
    let (buffer, rendered) = file_view(&workspace, &ui, None);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("file · Enter read · Esc close"), "{text}");
    assert!(text.contains("alpha · cwd /srv/app"), "{text}");
    assert!(text.contains("path: /srv/app/"), "{text}");
    assert!(
        text.contains("a path, not a directory"),
        "the prompt explains itself: {text}"
    );
    let (_, y) = rendered
        .prompt_cursor
        .expect("the path prompt has a cursor");
    assert_eq!(y, file_rect(Rect::new(0, 0, 100, 30)).y + 2);
}

#[test]
fn the_file_viewer_shows_the_path_range_and_lines() {
    use crate::workspace::state::FileState;
    let (workspace, _) = one_pane();
    let pane = file_pane("notes.txt", "one\ntwo\n", 1, 2, 10);
    let ui = UiState {
        files: FileState {
            open: true,
            ..FileState::default()
        },
        ..UiState::default()
    };
    let (buffer, rendered) = file_view(&workspace, &ui, Some(&pane));
    let text = lines(&buffer).join("\n");
    assert!(text.contains("file · local · notes.txt"), "{text}");
    assert!(text.contains("lines 1-2 of 10"), "{text}");
    assert!(text.contains("one") && text.contains("two"), "{text}");
    assert!(
        text.contains("file · e edit · Esc close"),
        "the viewer offers the editor: {text}"
    );
    assert_eq!(rendered.prompt_cursor, None, "the viewer has no cursor");
}

#[test]
fn the_diff_draws_add_and_remove_marks_with_a_summary() {
    use crate::workspace::file::{Context, Pane, diff};
    use crate::workspace::state::FileState;
    let (workspace, _) = one_pane();
    let mut pane = Pane::viewing(
        "notes.txt",
        Context::default(),
        crate::workspace::file::Window::default(),
    );
    pane.viewer = None;
    pane.diff = Some(diff("a\nb\n", "a\nc\n"));
    let ui = UiState {
        files: FileState {
            open: true,
            ..FileState::default()
        },
        ..UiState::default()
    };
    let (buffer, _) = file_view(&workspace, &ui, Some(&pane));
    let text = lines(&buffer).join("\n");
    assert!(
        text.contains("file diff · w write · q discard · Esc close"),
        "{text}"
    );
    assert!(text.contains("1 added, 1 removed"), "{text}");
    assert!(text.contains("+     2 c"), "the added line: {text}");
    assert!(text.contains("-     2 b"), "the removed line: {text}");
}

#[test]
fn a_conflict_is_a_title_and_an_error_that_keeps_the_overlay() {
    use crate::workspace::file::{Context, Pane, diff};
    use crate::workspace::state::FileState;
    let (workspace, _) = one_pane();
    let mut pane = Pane::viewing(
        "notes.txt",
        Context::default(),
        crate::workspace::file::Window::default(),
    );
    pane.viewer = None;
    pane.diff = Some(diff("a\n", "b\n"));
    pane.conflict = true;
    let ui = UiState {
        files: FileState {
            open: true,
            error: Some("notes.txt changed since it was read; o overwrite or q discard".into()),
            ..FileState::default()
        },
        ..UiState::default()
    };
    let (buffer, _) = file_view(&workspace, &ui, Some(&pane));
    let text = lines(&buffer).join("\n");
    assert!(text.contains("changed on disk"), "{text}");
    assert!(text.contains("o overwrite"), "{text}");
    assert!(text.contains("changed since it was read"), "{text}");
}

#[test]
fn an_empty_file_or_a_read_error_stays_inside_the_overlay() {
    use crate::workspace::state::FileState;
    let (workspace, _) = one_pane();
    let pane = file_pane("notes.txt", "", 1, 0, 0);
    let ui = UiState {
        files: FileState {
            open: true,
            error: Some("notes.txt: not_found".into()),
            ..FileState::default()
        },
        ..UiState::default()
    };
    let (buffer, _) = file_view(&workspace, &ui, Some(&pane));
    let text = lines(&buffer).join("\n");
    assert!(text.contains("empty file"), "{text}");
    assert!(text.contains("notes.txt: not_found"), "{text}");
    assert!(text.contains("file · e edit · Esc close"), "{text}");
}
