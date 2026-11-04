use super::*;
use crate::theme::{ColorDepth, ThemeChoice, theme};
use crate::vt::Vt;
use crate::workspace::agents::{AgentRow, Priority, sort_rows};
use crate::workspace::commands::RenameTarget;
use crate::workspace::manager;
use crate::workspace::model::{Axis, PaneId, StartKind, Workspace};
use crate::workspace::state::{Line, PaletteState, PreviewState, Prompt, SearchState, UiState};
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

/// Snapshots each drawn pane's VT with the same "hi" text, so the terminal
/// grid inside the border is visible.
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
    render_agents_theme(workspace, ui, width, height, rows, Theme::default())
}

fn render_agents_theme(
    workspace: &Workspace,
    ui: &UiState,
    width: u16,
    height: u16,
    rows: &[AgentRow],
    theme: Theme,
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
        identity: None,
    };
    let rendered = render_with(&mut buffer, area, workspace, ui, &view);
    (buffer, rendered)
}

/// One pane on a narrow terminal: the pane-nav row, the identity row on the
/// top border, the grid and the single bottom line. There is no tab bar, no
/// brand header and no per-pane footer.
#[test]
fn a_narrow_frame_is_the_pane_nav_above_one_framed_pane_and_one_bottom_line() {
    let (workspace, _) = one_pane();
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 40, 12);
    let rows = lines(&buffer);
    assert_eq!(
        rows[0],
        format!("01 t1{}", " ".repeat(35)),
        "the pane-nav row names the pane without renumbering"
    );
    assert_eq!(
        rows[1],
        format!("┌› 01{}t1┐", " ".repeat(32)),
        "the identity lives on the top border; the default name is shown once, on the right"
    );
    assert_eq!(rows[2], format!("│hi{}│", " ".repeat(36)));
    for row in &rows[3..10] {
        assert_eq!(row, &format!("│{}│", " ".repeat(38)));
    }
    assert_eq!(rows[10], format!("└{}┘", "─".repeat(38)));
    assert!(
        rows[11].starts_with(" NORMAL  t1"),
        "the bottom line starts with the mode and the identity: {:?}",
        rows[11]
    );
    assert!(
        !rows.iter().any(|row| row.contains("tabs")),
        "no tab bar is drawn: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("tab 1")),
        "no tab chrome is drawn: {rows:?}"
    );
    assert_eq!(
        rendered.focused,
        Some((workspace.focused_pane().unwrap(), Rect::new(1, 2, 38, 8)))
    );
    assert_eq!(rendered.prompt_cursor, None);
}

#[test]
fn a_wide_terminal_puts_the_rail_left_and_the_panes_start_at_row_zero() {
    let (workspace, _) = one_pane();
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 120, 40);
    let rows = lines(&buffer);
    assert!(rows[0].contains(" SPACES"), "{:?}", rows[0]);
    assert_eq!(rows[0].chars().nth(29), Some('│'), "rail separator");
    let pane_row: String = rows[0].chars().skip(30).collect();
    assert!(
        pane_row.starts_with('┌'),
        "the pane area starts at row zero beside the rail: {:?}",
        rows[0]
    );
    assert!(rows[0].contains("› 01"), "{:?}", rows[0]);
    assert!(
        rows[0].contains("t1┐"),
        "the address is on the right: {:?}",
        rows[0]
    );
    assert_eq!(
        rows[39].as_bytes()[0],
        b' ',
        "the status row spans the rail"
    );
    assert!(rows[39].starts_with(" NORMAL  t1"), "{:?}", rows[39]);
    assert!(
        !rows.iter().any(|row| row.contains("tab 1")),
        "no tab bar: {rows:?}"
    );
}

#[test]
fn the_focused_frame_and_header_are_mint_and_the_other_is_a_hairline() {
    let (workspace, first, second) = two_panes();
    assert_eq!(workspace.focused_pane(), Some(second));
    let theme = Theme::default();
    let (buffer, rendered) = render_ws(&workspace, &UiState::default(), 120, 20);
    // Both panes are drawn wide: 01 at x=30 and 02 at x=76.
    assert_eq!(buffer[(30, 0)].fg, theme.line, "unfocused frame");
    assert_eq!(buffer[(76, 0)].fg, theme.accent, "focused frame is mint");
    assert_eq!(
        buffer[(77, 0)].bg,
        theme.active_bg,
        "the focused header is a color block"
    );
    assert_eq!(
        buffer[(31, 0)].bg,
        theme.panel_bg,
        "the unfocused header stays quiet"
    );
    assert!(
        buffer[(79, 0)].modifier.contains(Modifier::BOLD),
        "the focused number is bold"
    );
    assert_eq!(rendered.focused.map(|(pane, _)| pane), Some(second));
    let _ = first;
}

#[test]
fn the_pane_header_keeps_the_number_name_and_word_and_drops_the_address_first() {
    let mut workspace = Workspace::new();
    let request = workspace.start_request(StartKind::Initial);
    let first = workspace.insert_terminal(&request, "t7".to_owned(), "build".to_owned());
    workspace.set_title(first, "claude".into());
    workspace.set_status(first, Some("blocked".into()));
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 60, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("› 01 build: claude"), "{header:?}");
    assert!(header.contains("blocked"), "{header:?}");
    assert!(
        header.contains(" t7"),
        "the address is on the right: {header:?}"
    );

    // A narrow pane drops the address, then the title detail, before the word.
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 24, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("01 build"), "{header:?}");
    assert!(header.contains("blocked"), "{header:?}");
    assert!(
        !header.contains("t7"),
        "the address was dropped: {header:?}"
    );
    assert!(
        !header.contains("claude"),
        "the detail was dropped: {header:?}"
    );
}

#[test]
fn a_narrow_header_never_hard_clips_the_name() {
    // 16 columns: the full name still wins over the word and the address.
    let mut workspace = Workspace::new();
    let request = workspace.start_request(StartKind::Initial);
    let first = workspace.insert_terminal(&request, "t1".to_owned(), "build".to_owned());
    workspace.set_status(first, Some("idle".into()));
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 16, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("› 01 build"), "{header:?}");
    assert!(
        !header.contains("buil ") && !header.contains("buil│"),
        "the name is not cut before the border: {header:?}"
    );

    // 11 columns: the name no longer fits, so it ends with an ellipsis rather
    // than being clipped at the rect edge.
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 11, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("bui…"), "{header:?}");
}

#[test]
fn a_header_label_that_exactly_fills_the_room_keeps_its_address() {
    let mut workspace = Workspace::new();
    let request = workspace.start_request(StartKind::Initial);
    let first = workspace.insert_terminal(&request, "t7".to_owned(), "app".to_owned());
    workspace.set_title(first, "ui".into());
    // Inner width 15 = prefix 5 + gap 1 + address 2 + label 7 ("app: ui").
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 17, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("› 01 app: ui"), "{header:?}");
    assert_eq!(
        header.matches("app: ui").count(),
        1,
        "the label is drawn once, undamaged: {header:?}"
    );
    assert!(header.contains("t7"), "the address still fits: {header:?}");
}

#[test]
fn a_fresh_pane_shows_its_default_address_once() {
    let (workspace, _) = one_pane();
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 120, 20);
    let header = lines(&buffer)[0].clone();
    assert!(header.contains("› 01"), "{header:?}");
    assert_eq!(
        header.matches("t1").count(),
        1,
        "the default name is the address and appears once: {header:?}"
    );

    // A custom name keeps its own identity beside the address.
    let mut custom = Workspace::new();
    let request = custom.start_request(StartKind::Initial);
    custom.insert_terminal(&request, "t7".to_owned(), "build".to_owned());
    let (buffer, _) = render_ws(&custom, &UiState::default(), 120, 20);
    let header = lines(&buffer)[0].clone();
    assert!(header.contains("› 01 build"), "{header:?}");
    assert!(header.contains("t7"), "{header:?}");
}

#[test]
fn the_pane_header_and_the_narrow_chip_use_the_agent_rows_derived_word() {
    // Core reports `done`; the person has seen it, so the index calls it
    // idle. The pane chrome must not disagree with the index.
    let (mut workspace, first) = one_pane();
    workspace.set_status(first, Some("done".into()));
    let rows = vec![agent(
        first,
        "t1",
        "idle",
        Priority::Idle,
        "Build",
        "claude",
    )];

    let (buffer, _) = render_agents(&workspace, &UiState::default(), 120, 20, &rows);
    let header = lines(&buffer)[0].clone();
    assert!(header.contains("idle"), "{header:?}");
    assert!(!header.contains("done"), "{header:?}");

    let (buffer, _) = render_agents(&workspace, &UiState::default(), 60, 12, &rows);
    let chip = lines(&buffer)[0].clone();
    assert!(chip.contains("01 t1") && chip.contains("idle"), "{chip:?}");
    assert!(!chip.contains("done"), "{chip:?}");
}

#[test]
fn an_exited_pane_shows_the_client_fact_and_never_invents_status() {
    let (mut workspace, first) = one_pane();
    workspace.mark_finished(first, 3);
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 60, 12);
    let header = lines(&buffer)[1].clone();
    assert!(header.contains("exited 3"), "{header:?}");
    assert_eq!(
        buffer[(header.find("exited").expect("fact") as u16, 1)].fg,
        Color::Rgb(0xdc, 0x92, 0x92),
        "the exit fact is red"
    );
    assert!(
        !header.contains("SHELL") && !header.contains("idle"),
        "no status is invented: {header:?}"
    );
}

#[test]
fn the_bottom_line_follows_the_focused_pane_without_a_runtime_identity() {
    // `render` leaves View.identity empty (a bare frame), so the status line
    // must derive from the same-frame focused pane rather than any other one.
    let (workspace, first, second) = two_panes();
    assert_eq!(workspace.focused_pane(), Some(second));
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 80, 12);
    let rows = lines(&buffer);
    assert!(rows[1].contains("› 02"), "focused header: {:?}", rows[1]);
    assert!(
        rows[0].contains("02 t2"),
        "the chip row names the focused pane: {:?}",
        rows[0]
    );
    let bottom = rows[11].clone();
    assert!(
        bottom.starts_with(" NORMAL  t2"),
        "the fallback identity follows the focused pane: {bottom:?}"
    );
    assert!(
        !bottom.contains("t1"),
        "the bottom line never names the unfocused pane: {bottom:?}"
    );

    let mut workspace = workspace;
    assert!(workspace.focus_pane(first));
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 80, 12);
    let bottom = lines(&buffer)[11].clone();
    assert!(bottom.starts_with(" NORMAL  t1"), "{bottom:?}");
}

#[test]
fn the_bottom_line_carries_the_mode_the_identity_and_the_hints() {
    let (workspace, first) = one_pane();
    let theme = Theme::default();
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 120, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" NORMAL  t1"), "{row:?}");
    assert!(row.contains("^b a rail"), "{row:?}");
    assert!(row.contains("^b z zoom"), "{row:?}");
    assert!(row.contains("^b ? help"), "{row:?}");
    assert!(
        !row.contains("space 1"),
        "no active space repetition: {row:?}"
    );
    assert!(!row.contains("tab 1"), "no tab name repetition: {row:?}");
    assert_eq!(buffer[(0, 11)].bg, theme.accent, "the mode chip is accent");
    assert_eq!(buffer[(0, 11)].fg, theme.panel_bg);
    assert!(buffer[(0, 11)].modifier.contains(Modifier::BOLD));

    let ui = UiState {
        prefix: true,
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 120, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" PREFIX  t1"), "{row:?}");
    assert!(row.contains("esc cancel"), "{row:?}");
    let mut ui = UiState {
        selecting: Some(first),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 120, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" COPY  t1"), "{row:?}");
    assert!(row.contains("y copy"), "{row:?}");
    ui.selecting = None;
    ui.resize = true;
    let (buffer, _) = render_ws(&workspace, &ui, 120, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" RESIZE  t1"), "{row:?}");
    assert!(row.contains("h/l width"), "{row:?}");

    // A zoomed tab names ZOOM; no mode strip exists to overwrite a pane row.
    let (mut workspace, _) = one_pane();
    add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    assert!(workspace.toggle_zoom());
    let (buffer, _) = render_ws(&workspace, &UiState::default(), 120, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" ZOOM"), "{row:?}");
    assert!(row.contains("unzoom"), "{row:?}");
    assert!(
        lines(&buffer)[10].chars().nth(30) == Some('└'),
        "the pane keeps its row"
    );
}

#[test]
fn the_bottom_line_keeps_the_mode_and_identity_at_eighty_columns_and_drops_hints() {
    let (workspace, _) = one_pane();
    let mut ui = UiState {
        message: Some("left t7 running (hc attach t7) · a long transient notice".into()),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" NORMAL  t1"), "{row:?}");
    assert!(
        row.contains("left t7 running"),
        "the message stays: {row:?}"
    );
    assert!(
        !row.contains("^b a rail") && !row.contains("^b z zoom"),
        "the hints are the first thing truncated: {row:?}"
    );

    ui.message = None;
    ui.connection = Connection::Reconnecting { attempt: 2 };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 12);
    let row = lines(&buffer)[11].clone();
    assert!(row.starts_with(" NORMAL  t1"), "{row:?}");
    assert!(row.contains("reconnecting (attempt 2)"), "{row:?}");
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
    assert!(rows[11].starts_with(" NORMAL  empty"), "{:?}", rows[11]);
    assert_eq!(rendered.focused, None);
}

#[test]
fn the_rename_prompt_replaces_the_bottom_line_and_places_the_cursor() {
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
    assert!(!rows[11].contains("NORMAL"));

    let prompt = ui.prompt.as_mut().expect("prompt");
    prompt.edit(crate::workspace::state::Edit::Home);
    let (_, rendered) = render_ws(&workspace, &ui, 40, 12);
    assert_eq!(rendered.prompt_cursor, Some((12, 11)));
}

#[test]
fn help_names_the_rail_drawer_and_the_tab_selector() {
    let (workspace, _) = one_pane();
    let ui = UiState {
        help: true,
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 100, 40);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("prefix Ctrl-b"), "{text}");
    assert!(
        text.contains("switch layout/tab (compatibility)"),
        "the old multi-tab path is discoverable: {text}"
    );
    assert!(text.contains("a rail / drawer"), "{text}");
    assert!(text.contains("rail: j/k move"), "{text}");
    assert!(text.contains("tabs: j/k move"), "{text}");
    assert!(text.contains("resize mode (wide and unzoomed)"), "{text}");
    assert!(text.contains("narrow: prefix a"), "{text}");
    assert!(text.contains("any key closes help"));
    assert_eq!(rendered.focused, None);
}

#[test]
fn the_rail_lists_spaces_then_the_cross_space_agent_index() {
    let (workspace, first, second) = two_panes();
    let mut rows = vec![
        agent(first, "t1", "blocked", Priority::Blocked, "Build", "claude"),
        agent(second, "t2", "done", Priority::Done, "Review", "codex"),
    ];
    sort_rows(&mut rows);
    assert_eq!(rows[0].address, "t2", "the pinned row sorts first");
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    ui.sidebar.selected = Some(crate::workspace::sidebar::Item::Agent(second));
    let (buffer, rendered) = render_agents(&workspace, &ui, 120, 14, &rows);
    let text = lines(&buffer);
    assert!(text[0].starts_with(" SPACES"), "{text:?}");
    assert!(text[1].contains("01 space 1"), "{text:?}");
    assert!(text[1].contains("1 tab"), "{text:?}");
    assert!(
        text[2].starts_with('─'),
        "the section rule sits between SPACES and AGENTS: {text:?}"
    );
    assert!(text[3].starts_with(" AGENTS"), "{text:?}");
    assert!(
        text[4].contains("1 blocked") && text[4].contains("1 done unread"),
        "the attention summary: {text:?}"
    );
    assert!(text[5].contains("* Review"), "{text:?}");
    assert!(text[5].contains("done"), "{text:?}");
    assert!(text[6].contains("space 1 · t2"), "row detail: {text:?}");
    assert!(text[7].contains("Build"), "{text:?}");
    assert!(text[7].contains("blocked"), "{text:?}");
    assert!(
        text[12].starts_with(" ^b a navigate · Enter jump"),
        "the pinned footer: {text:?}"
    );
    assert!(text[13].starts_with(" NORMAL"), "the bottom line: {text:?}");
    assert_eq!(rendered.focused, None);
    assert_eq!(rendered.prompt_cursor, None);
    assert_eq!(rendered.prompt_cursor, None);

    // The active space is mint on its active background, with a left edge.
    let active = text[1].find('▌').expect("active marker") as u16;
    assert_eq!(buffer[(active, 1)].fg, Color::Rgb(0x9d, 0xdb, 0xc6));
    assert_eq!(buffer[(active, 1)].bg, Color::Rgb(0x27, 0x3e, 0x36));
    assert!(
        buffer[(active + 2, 1)].modifier.contains(Modifier::BOLD),
        "the active space name is bold"
    );

    // Core-reported words keep their role colors.
    let done = text[5].find("done").expect("done word") as u16;
    let blocked = text[7].find("blocked").expect("blocked word") as u16;
    assert_eq!(buffer[(done, 5)].fg, Color::Rgb(0x9d, 0xdb, 0xc6));
    assert_eq!(buffer[(blocked, 7)].fg, Color::Rgb(0xe6, 0xbd, 0x7c));
}

#[test]
fn a_sidebars_folder_line_is_only_drawn_when_the_space_has_one() {
    let (mut workspace, first) = one_pane();
    let space = workspace.spaces()[0].id;
    let rows = vec![agent(
        first,
        "t1",
        "idle",
        Priority::Idle,
        "Build",
        "claude",
    )];
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    let (buffer, _) = render_agents(&workspace, &ui, 120, 14, &rows);
    let text = lines(&buffer).join("\n");
    assert!(!text.contains("cwd"), "no empty path line: {text}");
    workspace.set_space_folder(space, "/work/herdr");
    let (buffer, _) = render_agents(&workspace, &ui, 120, 14, &rows);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("/work/herdr"), "the folder line: {text}");
}

#[test]
fn a_narrow_terminal_hides_the_rail_and_shows_the_pane_nav() {
    let (workspace, first, _) = two_panes();
    let rows = vec![agent(
        first,
        "t1",
        "done",
        Priority::Done,
        "Review",
        "codex",
    )];
    let (buffer, _) = render_agents(&workspace, &UiState::default(), 60, 12, &rows);
    let text = lines(&buffer);
    assert!(
        !text.iter().any(|row| row.contains(" SPACES")),
        "no rail below 90 columns: {text:?}"
    );
    assert!(
        text[0].contains("01 t1") && text[0].contains("02 t2"),
        "the pane-nav maps every pane: {text:?}"
    );
    assert!(
        text[0].contains("01 t1"),
        "the focused pane keeps its tree number: {text:?}"
    );
}

#[test]
fn the_narrow_drawer_draws_the_same_rail_over_the_panes() {
    let (workspace, first, _) = two_panes();
    let rows = vec![agent(
        first,
        "t1",
        "done",
        Priority::Done,
        "Review",
        "codex",
    )];
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    let (buffer, _) = render_agents(&workspace, &ui, 60, 12, &rows);
    let text = lines(&buffer);
    assert!(
        text[0].starts_with(" SPACES"),
        "the drawer is the rail: {text:?}"
    );
    assert!(text[1].contains("space 1"), "{text:?}");
    assert!(
        text[0].chars().nth(19) == Some('│'),
        "the drawer keeps the separator: {:?}",
        text[0]
    );
    // The drawer ends above the bottom line, so its pinned footer is visible
    // while the status row keeps its own hint.
    assert!(
        text[10].contains("navigate"),
        "the drawer footer is painted above the status row: {text:?}"
    );
    assert!(
        text[11].contains("rail · j/k move"),
        "the status row keeps the drawer hint: {text:?}"
    );
    assert!(
        !text[11].contains("navigate"),
        "the status row does not overwrite the drawer footer: {text:?}"
    );
    // The panes were not shrunk: the pane-nav and workspace still span the
    // whole width behind the drawer.
    let rects = geometry::compute(workspace.active_tab(), Rect::new(0, 0, 60, 12));
    assert_eq!(rects.workspace, Rect::new(0, 1, 60, 10));
    assert_eq!(rects.pane_nav.width, 60);
}

#[test]
fn the_narrow_drawer_clears_the_pane_and_clips_every_line() {
    let (workspace, pane) = one_pane();
    let area = Rect::new(0, 0, 80, 24);
    let rects = geometry::compute(workspace.active_tab(), area);
    let inner = geometry::inner(rects.pane(pane).expect("pane")).expect("inner");
    let mut vt = Vt::new(inner.width, inner.height).expect("vt");
    // A busy pane: a sentinel glyph, a wide character and a full-width fill.
    // Any surviving pane cell inside the drawer is unmistakable.
    let mut text = String::new();
    for _ in 0..inner.height {
        text.push('X');
        text.push('漢');
        text.push_str(&"X".repeat(usize::from(inner.width).saturating_sub(3)));
        text.push_str("\r\n");
    }
    vt.feed(text.as_bytes());
    let mut screens = BTreeMap::new();
    screens.insert(pane, vt.snapshot().expect("snapshot"));
    let theme = Theme::default();
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    // Two long agent rows exercise the clipped agent lines and right-aligned
    // status words inside the drawer.
    let rows = vec![
        agent(
            pane,
            "t1",
            "blocked",
            Priority::Blocked,
            "feature-authentication-build",
            "claude",
        ),
        agent(
            PaneId(999),
            "t2",
            "working",
            Priority::Working,
            "another-very-long-agent-title",
            "codex",
        ),
    ];
    let view = View {
        screens: &screens,
        rows: &rows,
        theme: &theme,
        ..View::default()
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &ui, &view);

    let drawer = crate::workspace::sidebar::drawer_rect(area);
    let separator = drawer.x + drawer.width - 1;
    assert_eq!(separator, 19);
    for y in drawer.y..drawer.y + drawer.height {
        assert_eq!(
            buffer[(separator, y)].symbol(),
            "│",
            "row {y} keeps its separator"
        );
        for x in drawer.x..separator {
            let symbol = buffer[(x, y)].symbol();
            assert!(
                symbol != "X" && symbol != "漢",
                "the drawer left a pane glyph at ({x},{y}): {symbol:?}"
            );
        }
    }
    // The footer is the last drawer row (the status row is below the drawer),
    // and it is ellipsized inside the content width.
    let footer = lines(&buffer)[usize::from(drawer.height - 1)].clone();
    assert!(footer.contains('…'), "the footer is ellipsized: {footer:?}");
    assert_eq!(
        footer.chars().nth(usize::from(separator)),
        Some('│'),
        "the footer stays inside the content width: {footer:?}"
    );

    // The wide rail uses the same drawing path and is unchanged: it sits left
    // of the panes, keeps its separator and never touches pane content.
    let mut wide_ui = UiState::default();
    wide_ui.sidebar.open = true;
    let (wide_buffer, _) = render_ws(&workspace, &wide_ui, 120, 40);
    for y in 0..39 {
        assert_eq!(wide_buffer[(29, y)].symbol(), "│", "wide separator row {y}");
    }
    assert!(lines(&wide_buffer)[0].contains(" SPACES"));
    assert_eq!(
        wide_buffer[(30, 0)].symbol(),
        "┌",
        "the pane starts after the rail"
    );
}

#[test]
fn the_tab_selector_lists_the_active_spaces_tabs_and_marks_the_active_one() {
    let (mut workspace, _) = one_pane();
    add(&mut workspace, StartKind::Tab, "t2");
    add(&mut workspace, StartKind::Tab, "t3");
    let space = workspace.active_space_id();
    let ui = UiState {
        tabs: Some(crate::workspace::state::TabsState { space, selected: 2 }),
        ..UiState::default()
    };
    let (buffer, rendered) = render_ws(&workspace, &ui, 80, 20);
    let text = lines(&buffer).join("\n");
    assert!(text.contains(" tabs ·"), "{text}");
    assert!(text.contains("3 tabs"), "{text}");
    assert!(text.contains("1  tab 1"), "{text}");
    assert!(text.contains("2  tab 2"), "{text}");
    assert!(text.contains("3  tab 3"), "{text}");
    assert!(text.contains("1 pane"), "{text}");
    assert!(
        text.contains("● 3  tab 3"),
        "the active tab is marked: {text}"
    );
    assert!(text.contains("Enter switch"), "{text}");
    assert_eq!(rendered.focused, None);
    assert_eq!(rendered.prompt_cursor, None);
}

#[test]
fn the_tab_selector_hint_ends_inside_the_popup_border() {
    let (mut workspace, _) = one_pane();
    add(&mut workspace, StartKind::Tab, "t2");
    let space = workspace.active_space_id();
    let ui = UiState {
        tabs: Some(crate::workspace::state::TabsState { space, selected: 2 }),
        ..UiState::default()
    };
    // The popup geometry draw_tabs uses: width min(area - 4, 64), centered,
    // one row below the top; its first inner row is the hint.
    let popup = |width: u16| {
        let popup_width = width.saturating_sub(4).min(64);
        let x = width.saturating_sub(popup_width) / 2;
        (x, popup_width, 2u16)
    };
    let inner_text = |row: &str, x: u16, popup_width: u16| -> String {
        row.chars()
            .skip(usize::from(x) + 1)
            .take(usize::from(popup_width) - 2)
            .collect()
    };

    for width in [120, 80] {
        let (buffer, _) = render_ws(&workspace, &ui, width, 20);
        let (x, popup_width, y) = popup(width);
        let row = lines(&buffer)[usize::from(y)].clone();
        let right = usize::from(x + popup_width - 1);
        assert_eq!(
            row.chars().nth(right),
            Some('│'),
            "the popup border closes the row at {width}: {row:?}"
        );
        assert_eq!(
            row.chars().nth(right - 1),
            Some(' '),
            "the hint ends inside the border at {width}: {row:?}"
        );
        let inner = inner_text(&row, x, popup_width);
        assert!(
            inner.contains("Esc close"),
            "the hint is complete at {width}: {inner:?}"
        );
        assert!(!inner.contains('…'), "the hint fits at {width}: {inner:?}");
    }

    // A narrow popup ellipsizes the hint instead of cutting it at the border.
    let width = 40;
    let (buffer, _) = render_ws(&workspace, &ui, width, 20);
    let (x, popup_width, y) = popup(width);
    let row = lines(&buffer)[usize::from(y)].clone();
    let right = usize::from(x + popup_width - 1);
    assert_eq!(row.chars().nth(right), Some('│'), "{row:?}");
    let inner = inner_text(&row, x, popup_width);
    assert!(
        inner.ends_with('…'),
        "the narrow hint is ellipsized at the inner edge: {inner:?}"
    );
}

#[test]
fn wide_titles_keep_the_status_word_inside_the_rail() {
    let (workspace, first) = one_pane();
    let rows = vec![agent(
        first,
        "t1",
        "working",
        Priority::Working,
        "构建界面现在非常长的一个标题",
        "claude",
    )];
    let mut ui = UiState::default();
    ui.sidebar.open = true;
    let (buffer, _) = render_agents(&workspace, &ui, 120, 14, &rows);
    let text = lines(&buffer).join("\n");
    assert!(text.contains('…'), "the title was truncated: {text}");
    assert!(text.contains("working"), "the status word was lost: {text}");
    let row = lines(&buffer)
        .into_iter()
        .find(|row| row.chars().take(29).collect::<String>().contains("working"))
        .expect("agent row");
    assert_eq!(
        row.chars().nth(29),
        Some('│'),
        "the row crossed the separator"
    );
    assert!(row.chars().take(29).collect::<String>().contains("working"));
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
fn the_palette_names_the_tab_selector_as_the_compatibility_path() {
    let (workspace, _) = one_pane();
    let ui = UiState {
        palette: Some(PaletteState {
            line: Line::new("layout"),
            ..PaletteState::default()
        }),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 100, 30);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("switch layout/tab (compatibility)"), "{text}");
    assert!(text.contains("^b T"), "{text}");
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
        !row.contains("^b a rail"),
        "the search bar owns the bottom line"
    );
    assert_eq!(
        rendered.prompt_cursor,
        Some((11, 11)),
        "after ' search: ' and the two-char query"
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
        identity: None,
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
fn the_scroll_hint_replaces_the_context_hints_and_survives_a_narrow_line() {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 120, 12);
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
        identity: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    let row = lines(&buffer)[11].clone();
    assert!(row.contains("scroll 2/18"), "{row:?}");
    assert!(row.contains("Shift-PgDn live"), "{row:?}");
    assert!(
        !row.contains("^b a rail"),
        "the scroll position replaces the hints: {row:?}"
    );

    // At 80 columns the identity fills most of the line; the scroll position
    // is truncated but still says the view is not live.
    let area = Rect::new(0, 0, 80, 12);
    let rects = geometry::compute(workspace.active_tab(), area);
    let mut screens = BTreeMap::new();
    let inner = geometry::inner(rects.panes[0].1).expect("inner");
    let mut vt = Vt::new(inner.width, inner.height).expect("vt");
    vt.feed(b"hi");
    screens.insert(rects.panes[0].0, vt.snapshot().expect("snapshot"));
    let view = View {
        screens: &screens,
        scroll_hint: Some("scroll 2/18 · Shift-PgDn live"),
        identity: Some("t1 · claude · /Users/changyu/Project/herdr/herdr-terminal"),
        ..view
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    let row = lines(&buffer)[11].clone();
    assert!(
        row.contains("scroll 2/"),
        "the scroll position survives: {row:?}"
    );
    assert!(
        row.starts_with(" NORMAL  t1"),
        "the identity survives: {row:?}"
    );
}

#[test]
fn the_runtime_identity_replaces_the_fallback_name() {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 120, 12);
    let theme = Theme::default();
    let view = View {
        identity: Some("t1 · claude · /work/herdr"),
        theme: &theme,
        ..View::default()
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    let row = lines(&buffer)[11].clone();
    assert!(
        row.starts_with(" NORMAL  t1 · claude · /work/herdr"),
        "the reported identity wins: {row:?}"
    );
}

#[test]
fn explicit_themes_style_the_chrome_consistently() {
    let (workspace, _) = one_pane();
    let area = Rect::new(0, 0, 120, 12);
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
            identity: None,
        };
        let mut buffer = Buffer::empty(area);
        render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
        let rail = buffer[(0, 0)].bg;
        let header = buffer[(31, 0)].bg;
        let bar = buffer[(50, 11)].bg;
        assert_eq!(rail, theme.side, "rail background: {choice:?}");
        assert_eq!(header, theme.active_bg, "focused header: {choice:?}");
        assert_eq!(bar, theme.bar, "bottom line: {choice:?}");
        seen.push((rail, header, bar));
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
        identity: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &view);
    assert_eq!(buffer[(inner.x, inner.y)].symbol(), "h");
    assert_ne!(buffer[(inner.x, inner.y)].bg, theme.bar);
}

#[test]
fn redrawing_a_scrolled_snapshot_shows_history_and_resumes_at_the_bottom() {
    let (workspace, pane) = one_pane();
    let area = Rect::new(0, 0, 80, 12);
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
        identity: None,
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
        identity: None,
    };
    let mut buffer = Buffer::empty(area);
    render_with(&mut buffer, area, &workspace, &UiState::default(), &stopped);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("LINE-099"), "the live bottom is back: {text}");
    assert!(!text.contains("scroll 20/60"));
}

mod panels;
