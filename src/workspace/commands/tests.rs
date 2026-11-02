use super::*;
use crate::workspace::model::{
    Axis, CloseAction, CloseKind, Direction, PaneId, Placement, StartKind,
};
use ratatui::layout::Rect;

fn viewport() -> Rect {
    Rect::new(0, 0, 80, 24)
}

fn add(workspace: &mut Workspace, kind: StartKind, name: &str) -> PaneId {
    let request = workspace.start_request(kind);
    workspace.insert_terminal(&request, name.to_owned(), name.to_owned())
}

fn two_panes() -> (Workspace, PaneId, PaneId) {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let second = add(&mut workspace, StartKind::Split(Axis::Columns), "t2");
    (workspace, first, second)
}

#[test]
fn start_commands_return_start_requests_with_new_ids() {
    let mut workspace = Workspace::new();
    let first = add(&mut workspace, StartKind::Initial, "t1");
    let old_tab = workspace.active_tab_id().expect("tab");

    let Effect::Start(request) = workspace.dispatch(Command::NewTab, viewport()) else {
        panic!("new tab did not ask for a terminal")
    };
    assert!(request.new_tab);
    assert_ne!(request.tab, old_tab);
    assert_eq!(request.space, workspace.active_space_id());

    let Effect::Start(request) = workspace.dispatch(Command::NewSpace, viewport()) else {
        panic!("new space did not ask for a terminal")
    };
    assert!(request.new_space && request.new_tab);

    let focused = workspace.focused_pane().expect("focused");
    let Effect::Start(request) = workspace.dispatch(Command::Split(Axis::Rows), viewport()) else {
        panic!("split did not ask for a terminal")
    };
    assert_eq!(
        request.placement,
        Placement::Split {
            axis: Axis::Rows,
            near: focused
        }
    );
    assert_eq!(
        workspace.pane_count(),
        1,
        "dispatching alone starts nothing"
    );
    assert_eq!(workspace.focused_pane(), Some(first));
}

#[test]
fn help_rename_and_detach_are_effects_not_model_changes() {
    let mut workspace = Workspace::new();
    add(&mut workspace, StartKind::Initial, "t1");
    let space = workspace.active_space_id();
    let tab = workspace.active_tab_id().expect("tab");
    assert_eq!(workspace.dispatch(Command::Help, viewport()), Effect::Help);
    assert_eq!(
        workspace.dispatch(Command::RenameTab, viewport()),
        Effect::Rename(RenameTarget::Tab { space, tab })
    );
    assert_eq!(
        workspace.dispatch(Command::RenameSpace, viewport()),
        Effect::Rename(RenameTarget::Space(space))
    );
    assert_eq!(
        workspace.dispatch(Command::Detach, viewport()),
        Effect::Detach
    );
}

#[test]
fn navigation_commands_change_the_view_and_wrap() {
    let (mut workspace, _, _) = two_panes();
    let request = workspace.start_request(StartKind::Tab);
    workspace.insert_terminal(&request, "t3".into(), "t3".into());
    let first = workspace.active_space().tabs[0].id;
    let second = workspace.active_space().tabs[1].id;
    assert_eq!(workspace.active_tab_id(), Some(second));

    assert_eq!(
        workspace.dispatch(Command::NextTab, viewport()),
        Effect::Changed
    );
    assert_eq!(workspace.active_tab_id(), Some(first));
    assert_eq!(
        workspace.dispatch(Command::SelectTab(2), viewport()),
        Effect::Changed
    );
    assert_eq!(workspace.active_tab_id(), Some(second));
    assert_eq!(
        workspace.dispatch(Command::SelectTab(2), viewport()),
        Effect::None
    );

    let (mut workspace, _, _) = two_panes();
    assert_eq!(
        workspace.dispatch(Command::Focus(Direction::Left), viewport()),
        Effect::Changed
    );
    assert_eq!(
        workspace.dispatch(Command::Focus(Direction::Left), viewport()),
        Effect::None
    );
    assert_eq!(
        workspace.dispatch(Command::Move(Direction::Right), viewport()),
        Effect::Changed
    );
    assert_eq!(
        workspace.dispatch(Command::Zoom, viewport()),
        Effect::Changed
    );
}

#[test]
fn close_ends_an_owned_pane_and_reports_its_address() {
    let (mut workspace, first, second) = two_panes();
    assert_eq!(workspace.focused_pane(), Some(second));
    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Close), viewport())
    else {
        panic!("close did not ask for terminal cleanup")
    };
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].pane, second);
    assert_eq!(closed[0].address, "t2");
    assert_eq!(closed[0].action, CloseAction::End);
    assert_eq!(workspace.pane(second), None);
    assert_eq!(workspace.focused_pane(), Some(first));

    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Close), viewport())
    else {
        panic!("close of the last pane")
    };
    assert_eq!(closed[0].action, CloseAction::End);
    assert_eq!(workspace.pane_count(), 0);
    assert_eq!(workspace.active_space().tabs.len(), 1, "empty state");
    assert_eq!(
        workspace.dispatch(Command::Close(CloseKind::Close), viewport()),
        Effect::None
    );
    assert_eq!(
        workspace.dispatch(Command::Focus(Direction::Left), viewport()),
        Effect::None
    );
    assert_eq!(workspace.dispatch(Command::Zoom, viewport()), Effect::None);
    assert_eq!(
        workspace.dispatch(Command::Move(Direction::Up), viewport()),
        Effect::None
    );
}

#[test]
fn keep_kill_and_the_pane_flags_pick_the_close_action() {
    let (mut workspace, _, second) = two_panes();
    workspace.pane_mut(second).expect("pane").keep = true;
    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Close), viewport())
    else {
        panic!("close")
    };
    assert_eq!(
        closed[0].action,
        CloseAction::Detach,
        "kept panes survive close"
    );

    let (mut workspace, _, _) = two_panes();
    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Keep), viewport())
    else {
        panic!("keep")
    };
    assert_eq!(closed[0].action, CloseAction::Detach);

    let (mut workspace, _, adopted) = two_panes();
    workspace.pane_mut(adopted).expect("pane").owned = false;
    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Close), viewport())
    else {
        panic!("close adopted")
    };
    assert_eq!(closed[0].action, CloseAction::Detach);

    let (mut workspace, _, _) = two_panes();
    let Effect::Close(closed) = workspace.dispatch(Command::Close(CloseKind::Kill), viewport())
    else {
        panic!("kill")
    };
    assert_eq!(closed[0].action, CloseAction::End);
}

#[test]
fn closing_a_finished_pane_only_forgets_its_record() {
    let (mut workspace, _, second) = two_panes();
    workspace.mark_finished(second, 7);
    for kind in [CloseKind::Close, CloseKind::Keep, CloseKind::Kill] {
        // Put the pane back for each kind.
        if workspace.pane(second).is_none() {
            let request = workspace.start_request(StartKind::Split(Axis::Columns));
            let pane = workspace.insert_terminal(&request, "t2".into(), "t2".into());
            workspace.mark_finished(pane, 7);
        }
        let focused = workspace
            .focused_pane()
            .expect("the finished pane is focused");
        let Effect::Close(closed) = workspace.dispatch(Command::Close(kind), viewport()) else {
            panic!("close of a finished pane")
        };
        assert_eq!(closed[0].pane, focused);
        assert_eq!(
            closed[0].action,
            CloseAction::Forget,
            "a finished pane has no program left"
        );
        assert!(workspace.pane(focused).is_none());
    }
}

#[test]
fn an_empty_workspace_offers_a_way_to_start_again() {
    let mut workspace = Workspace::new();
    let active = workspace.active_tab_id().expect("tab");
    let Effect::Start(request) = workspace.dispatch(Command::NewTab, viewport()) else {
        panic!("new tab in an empty workspace")
    };
    assert!(request.new_tab);
    assert_ne!(request.tab, active);
    let Effect::Start(split) = workspace.dispatch(Command::Split(Axis::Columns), viewport()) else {
        panic!("split without a pane starts in the empty tab")
    };
    assert_eq!(split.placement, Placement::Only);
}
