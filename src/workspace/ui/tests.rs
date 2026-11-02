use super::*;
use crate::vt::Vt;
use crate::workspace::commands::RenameTarget;
use crate::workspace::model::{Axis, PaneId, StartKind, Workspace};
use crate::workspace::state::{Prompt, UiState};
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
    let (buffer, rendered) = render_ws(&workspace, &ui, 60, 16);
    let text = lines(&buffer).join("\n");
    assert!(text.contains("help"), "{text}");
    assert!(text.contains("prefix Ctrl-b"), "{text}");
    assert!(text.contains("e keep + close") && text.contains("X kill + close"));
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
