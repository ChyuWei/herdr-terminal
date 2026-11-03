use super::*;
use ratatui::layout::Rect;

fn add(workspace: &mut Workspace, kind: StartKind, name: &str) -> PaneId {
    let request = workspace.start_request(kind);
    workspace.insert_terminal(&request, name.to_owned(), name.to_owned())
}

fn viewport() -> Rect {
    Rect::new(0, 0, 80, 24)
}

fn panes(workspace: &Workspace) -> Vec<PaneId> {
    workspace.active_tab().map(Tab::panes).unwrap_or_default()
}

#[test]
fn a_new_workspace_has_one_empty_space_and_tab() {
    let workspace = Workspace::new();
    assert_eq!(workspace.spaces().len(), 1);
    assert_eq!(workspace.active_space().name, "space 1");
    assert_eq!(workspace.active_space().tabs.len(), 1);
    let tab = workspace.active_tab().expect("tab");
    assert_eq!(tab.name, "tab 1");
    assert_eq!(tab.layout, None);
    assert_eq!(workspace.pane_count(), 0);
    assert_eq!(workspace.focused_pane(), None);
}

#[test]
fn a_mouse_click_focuses_a_pane_in_the_active_tab_only() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    assert_eq!(workspace.focused_pane(), Some(second));
    assert!(workspace.focus_pane(first));
    assert_eq!(workspace.focused_pane(), Some(first));
    assert!(!workspace.focus_pane(first), "already focused");
    assert!(!workspace.focus_pane(PaneId(999)), "unknown pane");

    add(&mut workspace, StartKind::Tab, "tab 2 terminal");
    assert_eq!(
        workspace.focused_pane(),
        workspace.active_tab().and_then(|tab| tab.focused)
    );
    assert!(
        !workspace.focus_pane(first),
        "the click cannot focus a pane in a background tab"
    );
}

#[test]
fn a_tab_bar_click_selects_a_tab_of_the_active_space() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    let first_tab = workspace.active_tab_id().expect("tab");
    add(&mut workspace, StartKind::Tab, "t2");
    let second_tab = workspace.active_tab_id().expect("tab");
    assert_ne!(first_tab, second_tab);
    assert!(workspace.select_tab_id(first_tab));
    assert_eq!(workspace.active_tab_id(), Some(first_tab));
    assert!(!workspace.select_tab_id(first_tab), "already active");
    assert!(!workspace.select_tab_id(TabId(999)), "unknown tab");
}

#[test]
fn dragging_a_split_boundary_moves_it_by_exact_cells() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    let viewport = viewport();
    let before = geometry::compute(workspace.active_tab(), viewport);
    assert_eq!(before.panes[0].1.width, 40);
    assert_eq!(before.panes[1].1.width, 39);

    assert!(workspace.resize_gap(viewport, &[], 0, 4));
    let after = geometry::compute(workspace.active_tab(), viewport);
    assert_eq!(after.panes[0].1.width, 44, "{after:?}");
    assert_eq!(after.panes[1].1.width, 35, "{after:?}");
    assert_eq!(after.panes[1].1.x, 45, "the one-cell gap moved with it");
    assert_eq!(after.panes[0].1.width + 1 + after.panes[1].1.width, 80);

    // The new spans become the ratio, so the next drag is equally exact.
    assert!(workspace.resize_gap(viewport, &[], 0, -2));
    let after = geometry::compute(workspace.active_tab(), viewport);
    assert_eq!(after.panes[0].1.width, 42);
    assert_eq!(after.panes[1].1.width, 37);
}

#[test]
fn a_split_drag_clamps_both_sides_and_refuses_impossible_ones() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    add(&mut workspace, StartKind::Split(Axis::Rows), "t2");
    let viewport = viewport();
    assert!(workspace.resize_gap(viewport, &[], 0, -100));
    let heights = geometry::compute(workspace.active_tab(), viewport)
        .panes
        .iter()
        .map(|(_, rect)| rect.height)
        .collect::<Vec<_>>();
    assert_eq!(heights[0], geometry::MIN_PANE, "{heights:?}");
    assert!(
        !workspace.resize_gap(viewport, &[], 0, -1),
        "already at the floor"
    );
    assert!(
        !workspace.resize_gap(viewport, &[], 1, 1),
        "no boundary after the last child"
    );
    assert!(!workspace.resize_gap(viewport, &[9], 0, 1), "bad path");

    assert!(workspace.toggle_zoom());
    assert!(
        !workspace.resize_gap(viewport, &[], 0, 2),
        "zoom hides boundaries"
    );
}

#[test]
fn the_initial_terminal_becomes_the_focused_leaf() {
    let mut workspace = Workspace::new();
    let pane = add(&mut workspace, StartKind::Initial, "t1");
    assert_eq!(workspace.pane_count(), 1);
    assert_eq!(workspace.focused_pane(), Some(pane));
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.layout.clone()),
        Some(Layout::Leaf(pane))
    );
    let record = workspace.pane(pane).expect("pane");
    assert_eq!(record.address, "t1");
    assert_eq!(record.name, "t1");
    assert!(record.owned);
    assert!(!record.keep);
}

#[test]
fn same_axis_splits_add_siblings_and_other_axes_nest() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    let third = add(&mut workspace, StartKind::Split(Axis::Columns), "t3");
    assert_eq!(panes(&workspace), vec![first, second, third]);
    let Layout::Split { axis, children } = workspace
        .active_tab()
        .and_then(|tab| tab.layout.clone())
        .expect("layout")
    else {
        panic!("expected a split")
    };
    assert_eq!(axis, Axis::Columns);
    assert_eq!(children.len(), 3);

    let fourth = add(&mut workspace, StartKind::Split(Axis::Rows), "t4");
    let tab = workspace.active_tab().expect("tab");
    assert_eq!(tab.panes(), vec![first, second, third, fourth]);
    let Some(Layout::Split { axis, children }) = &tab.layout else {
        panic!("expected the root split")
    };
    assert_eq!(*axis, Axis::Columns);
    assert!(matches!(
        &children[2].layout,
        Layout::Split { axis: Axis::Rows, children } if children.len() == 2
    ));
    assert_eq!(tab.focused, Some(fourth));
    assert_eq!(tab.zoomed, None);
}

#[test]
fn closing_the_focused_pane_refocuses_history_and_collapses_the_split() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    assert_eq!(workspace.focused_pane(), Some(second));
    assert!(workspace.remove_pane(second));
    assert_eq!(workspace.focused_pane(), Some(first));
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.layout.clone()),
        Some(Layout::Leaf(first))
    );
    assert_eq!(workspace.pane_count(), 1);
}

#[test]
fn the_last_pane_closes_the_tab_and_the_last_tab_closes_the_space() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let request = workspace.start_request(StartKind::Tab);
    let second = workspace.insert_terminal(&request, "t2".into(), "t2".into());
    assert_eq!(workspace.active_space().tabs.len(), 2);
    assert!(workspace.remove_pane(second));
    assert_eq!(workspace.active_space().tabs.len(), 1);
    assert_eq!(workspace.focused_pane(), Some(first));

    let request = workspace.start_request(StartKind::Space);
    let other = workspace.insert_terminal(&request, "t3".into(), "t3".into());
    assert_eq!(workspace.spaces().len(), 2);
    assert_eq!(workspace.focused_pane(), Some(other));
    assert!(workspace.remove_pane(other));
    assert_eq!(workspace.spaces().len(), 1);
    assert_eq!(workspace.focused_pane(), Some(first));

    assert!(workspace.remove_pane(first));
    assert_eq!(workspace.pane_count(), 0);
    assert_eq!(workspace.spaces().len(), 1, "the empty state keeps a space");
    let tab = workspace.active_tab().expect("tab");
    assert_eq!(tab.layout, None);
    assert_eq!(tab.focused, None);
}

#[test]
fn focus_and_move_follow_geometry_in_a_two_by_two_grid() {
    let mut workspace = Workspace::new();
    let top_left = add(&mut workspace, StartKind::Initial, "t1");
    let bottom_left = add(&mut workspace, StartKind::Split(Axis::Rows), "t2");
    assert!(workspace.focus_direction(viewport(), Direction::Up));
    assert_eq!(workspace.focused_pane(), Some(top_left));
    let top_right = add(&mut workspace, StartKind::Split(Axis::Columns), "t3");
    assert_eq!(workspace.focused_pane(), Some(top_right));
    assert!(workspace.focus_direction(viewport(), Direction::Down));
    assert_eq!(workspace.focused_pane(), Some(bottom_left));
    let bottom_right = add(&mut workspace, StartKind::Split(Axis::Columns), "t4");
    assert_eq!(workspace.focused_pane(), Some(bottom_right));

    assert!(workspace.focus_direction(viewport(), Direction::Up));
    assert_eq!(workspace.focused_pane(), Some(top_right));
    assert!(workspace.focus_direction(viewport(), Direction::Left));
    assert_eq!(workspace.focused_pane(), Some(top_left));

    // Move the focused pane right; the panes swap cells but keep their ids.
    assert!(workspace.move_direction(viewport(), Direction::Right));
    let tab = workspace.active_tab().expect("tab");
    assert_eq!(tab.focused, Some(top_left));
    assert_eq!(
        tab.panes(),
        vec![top_right, top_left, bottom_left, bottom_right]
    );
    assert!(!workspace.move_direction(viewport(), Direction::Right));
}

#[test]
fn focusing_while_zoomed_clears_zoom() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    assert!(workspace.toggle_zoom());
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.zoomed),
        Some(second)
    );
    assert!(workspace.focus_direction(viewport(), Direction::Left));
    assert_eq!(workspace.focused_pane(), Some(first));
    assert_eq!(workspace.active_tab().and_then(|tab| tab.zoomed), None);
    assert!(workspace.toggle_zoom());
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.zoomed),
        Some(first)
    );
    assert!(workspace.toggle_zoom());
    assert_eq!(workspace.active_tab().and_then(|tab| tab.zoomed), None);
}

#[test]
fn tabs_and_spaces_select_and_wrap() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    let request = workspace.start_request(StartKind::Tab);
    workspace.insert_terminal(&request, "t2".into(), "t2".into());
    assert_eq!(workspace.active_space().tabs.len(), 2);
    assert!(workspace.next_tab());
    assert_eq!(
        workspace.active_tab().map(|tab| tab.name.as_str()),
        Some("tab 1")
    );
    assert!(workspace.prev_tab());
    assert_eq!(
        workspace.active_tab().map(|tab| tab.name.as_str()),
        Some("tab 2")
    );
    assert!(workspace.select_tab(1));
    assert_eq!(
        workspace.active_tab().map(|tab| tab.name.as_str()),
        Some("tab 1")
    );
    assert!(!workspace.select_tab(1));
    assert!(!workspace.select_tab(9));

    let request = workspace.start_request(StartKind::Space);
    workspace.insert_terminal(&request, "t3".into(), "t3".into());
    assert_eq!(workspace.active_space().name, "space 2");
    assert!(workspace.next_space());
    assert_eq!(workspace.active_space().name, "space 1");
    assert!(workspace.prev_space());
    assert_eq!(workspace.active_space().name, "space 2");
}

#[test]
fn renaming_trims_and_ignores_empty_or_unknown_names() {
    let mut workspace = Workspace::new();
    let space = workspace.active_space_id();
    let tab = workspace.active_tab_id().expect("tab");
    assert!(workspace.rename_tab(space, tab, "  build  "));
    assert_eq!(
        workspace.active_tab().map(|tab| tab.name.as_str()),
        Some("build")
    );
    assert!(!workspace.rename_tab(space, tab, "   "));
    assert!(!workspace.rename_tab(space, TabId(999), "x"));
    assert!(workspace.rename_space(space, " work "));
    assert_eq!(workspace.active_space().name, "work");
    assert!(!workspace.rename_space(space, ""));
    assert!(!workspace.rename_space(SpaceId(999), "x"));
}

#[test]
fn core_titles_and_statuses_update_only_on_change() {
    let mut workspace = Workspace::new();
    let pane = add(&mut workspace, StartKind::Initial, "t1");
    assert!(workspace.set_title(pane, "vim".into()));
    assert!(!workspace.set_title(pane, "vim".into()));
    assert!(workspace.set_status(pane, Some("working".into())));
    assert!(!workspace.set_status(pane, Some("working".into())));
    assert!(workspace.set_status(pane, None));
    let record = workspace.pane(pane).expect("pane");
    assert_eq!(record.title, "vim");
    assert_eq!(record.status, None);
}

#[test]
fn start_requests_reserve_unique_ids_and_write_our_labels() {
    let mut workspace = Workspace::new();
    let initial = workspace.start_request(StartKind::Initial);
    assert!(!initial.new_space && !initial.new_tab);
    assert_eq!(initial.placement, Placement::Only);
    let first = workspace.insert_terminal(&initial, "t1".into(), "t1".into());

    let tab = workspace.start_request(StartKind::Tab);
    assert!(tab.new_tab && !tab.new_space);
    assert_eq!(tab.space, initial.space);
    assert_ne!(tab.tab, initial.tab);
    let labels = tab.labels();
    assert_eq!(labels["herdr.client"], "herdr-terminal");
    assert_eq!(labels["herdr.terminal.space"], tab.space.0.to_string());
    assert_eq!(labels["herdr.terminal.tab"], tab.tab.0.to_string());
    assert_eq!(labels["herdr.terminal.pane"], tab.pane.0.to_string());
    assert!(!labels.contains_key("herdr.space"));
    assert!(!labels.contains_key("herdr.tab"));
    let second = workspace.insert_terminal(&tab, "t2".into(), "t2".into());

    let space = workspace.start_request(StartKind::Space);
    assert!(space.new_space && space.new_tab);
    assert_ne!(space.space, initial.space);
    assert_ne!(space.tab, initial.tab);
    let third = workspace.insert_terminal(&space, "t3".into(), "t3".into());
    assert_eq!(workspace.spaces().len(), 2);
    assert_eq!(workspace.active_space().id, space.space);
    assert_eq!(workspace.active_tab().map(|tab| tab.id), Some(space.tab));

    let split = workspace.start_request(StartKind::Split(Axis::Rows));
    assert_eq!(split.space, space.space);
    assert_eq!(split.tab, space.tab);
    assert!(matches!(
        split.placement,
        Placement::Split { axis: Axis::Rows, near } if near == third
    ));
    let fourth = workspace.insert_terminal(&split, "t4".into(), "t4".into());
    assert!(first != second && second != third && third != fourth);
    assert_eq!(workspace.pane_count(), 4);
}

#[test]
fn ids_are_not_reused_after_panes_and_tabs_close() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    let third = add(&mut workspace, StartKind::Tab, "t3");
    let third_tab = workspace.active_tab_id();
    workspace.remove_pane(third);
    let fourth = add(&mut workspace, StartKind::Tab, "t4");
    assert_ne!(fourth, first);
    assert_ne!(fourth, second);
    assert_ne!(fourth, third);
    assert_ne!(workspace.active_tab_id(), third_tab);
    assert!(workspace.remove_pane(second));
    let fifth = add(&mut workspace, StartKind::Tab, "t5");
    assert!(fifth.0 > fourth.0);
}

#[test]
fn a_split_with_a_stale_target_still_inserts_the_pane() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let request = StartRequest {
        space: workspace.active_space_id(),
        tab: workspace.active_tab_id().expect("tab"),
        pane: PaneId(9998),
        placement: Placement::Split {
            axis: Axis::Columns,
            near: PaneId(9999),
        },
        new_space: false,
        new_tab: false,
    };
    let second = workspace.insert_terminal(&request, "t2".into(), "t2".into());
    assert_eq!(
        workspace.active_tab().expect("tab").panes(),
        vec![first, second]
    );
    assert_eq!(workspace.pane_count(), 2);
}

#[test]
fn close_actions_follow_ownership_and_the_keep_flag() {
    let owned = Pane {
        id: PaneId(1),
        address: "t1".into(),
        name: "t1".into(),
        labels: BTreeMap::new(),
        title: String::new(),
        status: None,
        owned: true,
        keep: false,
        exit: None,
    };
    let mut kept = owned.clone();
    kept.keep = true;
    let mut adopted = owned.clone();
    adopted.owned = false;
    let mut finished = owned.clone();
    finished.exit = Some(7);

    assert_eq!(close_action(&owned, CloseKind::Close), CloseAction::End);
    assert_eq!(close_action(&kept, CloseKind::Close), CloseAction::Detach);
    assert_eq!(
        close_action(&adopted, CloseKind::Close),
        CloseAction::Detach
    );
    assert_eq!(close_action(&owned, CloseKind::Keep), CloseAction::Detach);
    assert_eq!(close_action(&owned, CloseKind::Kill), CloseAction::End);
    // A finished pane has no program left: every close only forgets it.
    for kind in [CloseKind::Close, CloseKind::Keep, CloseKind::Kill] {
        assert_eq!(close_action(&finished, kind), CloseAction::Forget);
    }
}

#[test]
fn a_finished_pane_can_become_live_again() {
    let mut workspace = Workspace::new();
    let pane = add(&mut workspace, StartKind::Initial, "t1");
    assert!(!workspace.pane(pane).expect("pane").finished());

    assert!(workspace.mark_finished(pane, 7));
    assert!(
        !workspace.mark_finished(pane, 7),
        "the same status is no change"
    );
    assert!(workspace.pane(pane).expect("pane").finished());
    assert_eq!(workspace.pane(pane).expect("pane").exit, Some(7));
    assert!(workspace.mark_finished(pane, 9));
    assert_eq!(workspace.pane(pane).expect("pane").exit, Some(9));

    assert!(workspace.mark_live(pane));
    assert!(!workspace.mark_live(pane));
    assert!(!workspace.pane(pane).expect("pane").finished());
}

#[test]
fn the_location_of_a_pane_is_its_space_and_tab() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let request = workspace.start_request(StartKind::Tab);
    let second = workspace.insert_terminal(&request, "t2".into(), "t2".into());
    let space = workspace.active_space_id();
    assert_eq!(
        workspace.location(first),
        Some((space, workspace.active_space().tabs[0].id))
    );
    assert_eq!(
        workspace.location(second),
        Some((space, workspace.active_space().tabs[1].id))
    );
    assert_eq!(workspace.location(PaneId(999)), None);
    assert_eq!(
        workspace.pane_id_by_address("t2"),
        Some(second),
        "events find panes by address"
    );
    assert_eq!(workspace.pane_by_address("nope"), None);
}

#[test]
fn showing_an_existing_terminal_adopts_it_without_taking_ownership() {
    let mut workspace = Workspace::new();
    let request = workspace.adopt_request();
    let labels = BTreeMap::from([
        ("herdr.client".to_owned(), "herdr-app".to_owned()),
        ("herdr.space".to_owned(), "7".to_owned()),
    ]);
    let pane = workspace
        .insert_adopted(
            &request,
            AdoptedTerminal {
                address: "t9".to_owned(),
                name: "worker".to_owned(),
                labels: labels.clone(),
                title: "Build".to_owned(),
                status: Some("working".to_owned()),
                keep: true,
            },
        )
        .expect("adopted");
    let record = workspace.pane(pane).expect("pane");
    assert_eq!(record.address, "t9");
    assert_eq!(record.name, "worker");
    assert_eq!(record.title, "Build");
    assert_eq!(record.status.as_deref(), Some("working"));
    assert_eq!(record.labels, labels, "core labels are kept verbatim");
    assert!(!record.owned, "a shown terminal is never owned");
    assert!(record.keep);
    assert_eq!(workspace.focused_pane(), Some(pane));
    assert_eq!(
        workspace.active_tab().and_then(|tab| tab.layout.clone()),
        Some(Layout::Leaf(pane)),
        "an empty tab takes the adopted pane alone"
    );
    assert_eq!(
        close_action(record, CloseKind::Close),
        CloseAction::Detach,
        "closing a shown terminal never ends it"
    );
}

#[test]
fn an_adopted_terminal_splits_the_current_tab_and_is_never_duplicated() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let request = workspace.adopt_request();
    let adopted = workspace
        .insert_adopted(
            &request,
            AdoptedTerminal {
                address: "t9".to_owned(),
                name: String::new(),
                labels: BTreeMap::new(),
                title: String::new(),
                status: None,
                keep: false,
            },
        )
        .expect("adopted");
    assert_eq!(
        workspace.pane(adopted).expect("pane").name,
        "t9",
        "an empty core name falls back to the address"
    );
    assert_eq!(panes(&workspace), vec![first, adopted]);
    assert_eq!(workspace.focused_pane(), Some(adopted));
    assert_eq!(
        workspace.location(first),
        workspace.location(adopted),
        "the adopted pane stays in the current tab"
    );

    let tabs = workspace.active_space().tabs.len();
    let second = workspace.adopt_request();
    assert_eq!(
        workspace.insert_adopted(
            &second,
            AdoptedTerminal {
                address: "t9".to_owned(),
                name: "again".to_owned(),
                labels: BTreeMap::new(),
                title: String::new(),
                status: None,
                keep: false,
            },
        ),
        None,
        "an address can back only one pane"
    );
    assert_eq!(workspace.pane_count(), 2);
    assert_eq!(
        workspace.active_space().tabs.len(),
        tabs,
        "a refused adoption leaves no empty tab"
    );
}

#[test]
fn an_adopted_pane_captures_and_restores_by_address() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    let request = workspace.adopt_request();
    workspace
        .insert_adopted(
            &request,
            AdoptedTerminal {
                address: "t9".to_owned(),
                name: "worker".to_owned(),
                labels: BTreeMap::new(),
                title: String::new(),
                status: None,
                keep: false,
            },
        )
        .expect("adopted");
    let document = workspace.capture_doc();
    let text = document.to_value().to_string();
    assert!(text.contains("\"t9\""), "{text}");
    let t1_labels = workspace
        .pane_by_address("t1")
        .expect("own terminal")
        .labels
        .clone();

    let infos = vec![
        TerminalInfo::new("t1", t1_labels),
        TerminalInfo::new("t9", BTreeMap::new()),
    ];
    let restored = restore(&document, IdAllocator::new("default"), (1, 1), &infos);
    assert_eq!(restored.missing, 0);
    let ours = restored
        .workspace
        .pane_by_address("t1")
        .expect("own terminal");
    let adopted = restored
        .workspace
        .pane_by_address("t9")
        .expect("adopted terminal");
    assert!(ours.owned);
    assert!(!adopted.owned, "restore keeps an adopted terminal unowned");
    assert_eq!(adopted.name, "worker");
}
