//! Overlay and mode-strip buffer tests: the manager, preview, file overlay,
//! mode chips and the sidebar's selected rows.

use super::*;

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

#[test]
fn the_mode_strips_use_theme_chips_and_leave_the_panes_alone() {
    let (workspace, first) = one_pane();
    let (plain, _) = render_ws(&workspace, &UiState::default(), 60, 12);
    let plain_rows = lines(&plain);

    // PREFIX: accent chip, and the status bar stays underneath.
    let ui = UiState {
        prefix: true,
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let rows = lines(&buffer);
    assert!(rows[10].starts_with(" PREFIX "), "{rows:?}");
    assert!(rows[10].contains("? keybinds"), "{rows:?}");
    assert_eq!(buffer[(0, 10)].bg, Color::Rgb(137, 180, 250));
    assert_eq!(buffer[(0, 10)].fg, Color::Rgb(24, 24, 37));
    assert!(buffer[(0, 10)].modifier.contains(Modifier::BOLD));

    // COPY: mauve chip, motions and copy/search hints.
    let ui = UiState {
        selecting: Some(first),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let rows = lines(&buffer);
    assert!(rows[10].starts_with(" COPY "), "{rows:?}");
    assert!(rows[10].contains("h/j/k/l"), "{rows:?}");
    assert_eq!(buffer[(0, 10)].bg, Color::Rgb(203, 166, 247));
    assert_eq!(buffer[(0, 10)].fg, Color::Rgb(24, 24, 37));

    // The whole hint line needs a wider workspace: at 120 columns the strip
    // starts after the sidebar and shows every hint.
    let (wide, _) = render_ws(&workspace, &ui, 120, 12);
    let wide_rows = lines(&wide);
    assert!(wide_rows[10].contains(" COPY "), "{wide_rows:?}");
    for hint in [
        "h/j/k/l",
        "w/b/e",
        "v/space select",
        "y/enter copy",
        "/? search",
        "esc cancel",
    ] {
        assert!(
            wide_rows[10].contains(hint),
            "missing {hint:?}: {wide_rows:?}"
        );
    }

    // RESIZE: yellow chip, width/height hints.
    let ui = UiState {
        resize: true,
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let rows = lines(&buffer);
    assert!(rows[10].starts_with(" RESIZE "), "{rows:?}");
    assert!(
        rows[10].contains("h/l width") && rows[10].contains("j/k height"),
        "{rows:?}"
    );
    assert!(rows[10].contains("esc done"), "{rows:?}");
    assert_eq!(buffer[(0, 10)].bg, Color::Rgb(249, 226, 175));
    assert_eq!(buffer[(0, 10)].fg, Color::Rgb(24, 24, 37));

    // The status bar is still the last row in every mode.
    assert!(
        rows[11].contains("space 1 / tab 1 / t1"),
        "status bar under the strip: {rows:?}"
    );
    assert!(!rows[11].contains("RESIZE"));

    // Closing the strip restores the pane's bottom border and keeps its text.
    let (restored, _) = render_ws(&workspace, &UiState::default(), 60, 12);
    let restored_rows = lines(&restored);
    assert_eq!(restored_rows[8], plain_rows[8]);
    assert_ne!(restored_rows[10], rows[10]);
    assert!(restored_rows[2].contains("hi"), "{restored_rows:?}");
    assert!(
        restored_rows[10].starts_with('└'),
        "the bottom border returns: {restored_rows:?}"
    );
}

#[test]
fn the_search_and_rename_chips_are_colored_prompt_markers() {
    let (workspace, _) = one_pane();
    let mut ui = UiState {
        search: Some(SearchState::new(PaneId(1))),
        ..UiState::default()
    };
    ui.search.as_mut().unwrap().line = Line::new("hi");
    let (buffer, rendered) = render_ws(&workspace, &ui, 60, 12);
    let row: String = (0..60).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.starts_with(" search: hi"), "{row:?}");
    assert_eq!(buffer[(0, 11)].bg, Color::Rgb(203, 166, 247));
    assert_eq!(buffer[(0, 11)].fg, Color::Rgb(24, 24, 37));
    assert_eq!(rendered.prompt_cursor, Some((11, 11)));

    let space = workspace.active_space_id();
    let tab = workspace.active_tab_id().expect("tab");
    let ui = UiState {
        prompt: Some(Prompt::new(RenameTarget::Tab { space, tab }, "tab 1")),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 60, 12);
    let row: String = (0..60).map(|x| buffer[(x, 11)].symbol()).collect();
    assert!(row.starts_with("rename tab: "), "{row:?}");
    assert_eq!(buffer[(0, 11)].bg, Color::Rgb(137, 180, 250));
    assert_eq!(buffer[(0, 11)].fg, Color::Rgb(24, 24, 37));
}

#[test]
fn the_selected_palette_row_is_the_accent_block() {
    let (workspace, _) = one_pane();
    let ui = UiState {
        palette: Some(PaletteState {
            line: Line::new("split"),
            ..PaletteState::default()
        }),
        ..UiState::default()
    };
    let (buffer, _) = render_ws(&workspace, &ui, 80, 20);
    let rows = lines(&buffer);
    let y = rows
        .iter()
        .position(|row| row.contains("split left/right"))
        .expect("selected entry") as u16;
    let x = rows[y as usize]
        .find("split left/right")
        .expect("entry label") as u16
        - 1;
    assert_eq!(
        buffer[(x, y)].bg,
        Color::Rgb(137, 180, 250),
        "accent background on the selected row"
    );
    assert_eq!(buffer[(x, y)].fg, Color::Rgb(24, 24, 37));
    assert!(buffer[(x, y)].modifier.contains(Modifier::BOLD));
    let below = rows
        .iter()
        .position(|row| row.contains("split top/bottom"))
        .expect("an unselected row") as u16;
    assert_ne!(
        buffer[(x, below)].bg,
        Color::Rgb(137, 180, 250),
        "unselected rows are not the accent block"
    );
}
