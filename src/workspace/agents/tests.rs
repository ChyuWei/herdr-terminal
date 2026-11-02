use super::*;
use crate::core::{Snapshot, Terminal};
use crate::workspace::model::{StartKind, Workspace};

fn terminal(address: &str, state: Option<&str>, reported: bool, turn: u64) -> Terminal {
    Terminal {
        address: address.into(),
        state: "live".into(),
        reported,
        turn,
        status: state.map(|state| Status {
            state: state.into(),
            ..Status::default()
        }),
        ..Terminal::default()
    }
}

fn row(terminal: &Terminal, local_seen: bool) -> AgentRow {
    AgentRow::from_terminal(PaneId(1), "pane", (0, 0, 0), terminal, local_seen)
        .expect("an agent row")
}

fn seen(turn: u64) -> BTreeMap<String, String> {
    BTreeMap::from([(SEEN_LABEL.into(), turn.to_string())])
}

#[test]
fn only_a_reported_terminal_is_an_agent() {
    assert!(!is_agent(&terminal("t1", None, false, 0)));
    assert!(is_agent(&terminal("t1", Some("idle"), false, 0)));
    assert!(is_agent(&terminal("t1", Some("idle"), true, 0)));
    assert!(is_agent(&terminal("t1", None, true, 2)));

    let mut titled = terminal("t1", None, false, 0);
    titled.title = "claude".into();
    titled.argv = vec!["claude".into()];
    assert!(
        !is_agent(&titled),
        "a title or argv never makes a terminal an agent"
    );

    let mut finished = terminal("t1", None, true, 3);
    finished.state = "finished".into();
    assert!(is_agent(&finished), "a reported terminal stays listed");
}

#[test]
fn the_row_word_follows_the_documented_table() {
    let mut blocked = terminal("t1", Some("blocked"), true, 1);
    assert_eq!(row(&blocked, true).word, "blocked");
    blocked.status.as_mut().unwrap().state = "error".into();
    assert_eq!(row(&blocked, true).word, "blocked");

    let working = terminal("t1", Some("working"), true, 0);
    assert_eq!(row(&working, true).word, "working");

    let done = terminal("t1", Some("done"), true, 1);
    assert_eq!(row(&done, false).word, "done");
    assert_eq!(row(&done, true).word, "idle");

    let cleared = terminal("t1", None, true, 1);
    assert_eq!(row(&cleared, false).word, "done");
    assert_eq!(row(&cleared, true).word, "idle");

    let idle = terminal("t1", Some("idle"), true, 0);
    assert_eq!(row(&idle, false).word, "idle", "turn 0 is idle, not done");

    let other = terminal("t1", Some("waiting"), true, 0);
    assert_eq!(row(&other, true).word, "waiting");
    assert_eq!(row(&other, true).priority, Priority::Idle);
}

#[test]
fn a_missing_seen_label_is_not_a_zero_label() {
    let mut record = terminal("t1", Some("done"), true, 2);
    assert!(unseen(2, &record.labels, false));
    assert!(!unseen(2, &record.labels, true));

    record.labels = seen(1);
    assert!(
        unseen(2, &record.labels, true),
        "the label is authoritative"
    );
    record.labels = seen(2);
    assert!(!unseen(2, &record.labels, false));
    record.labels = seen(3);
    assert!(!unseen(2, &record.labels, false));

    record.labels = BTreeMap::from([(SEEN_LABEL.into(), "not a number".into())]);
    assert!(
        !unseen(2, &record.labels, true),
        "an invalid label falls back to the local flag (seen)"
    );
    assert!(
        unseen(2, &record.labels, false),
        "an invalid label falls back to the local flag (unseen)"
    );
}

#[test]
fn pinned_requires_a_present_non_empty_label() {
    let mut record = terminal("t1", Some("idle"), true, 0);
    assert!(!pinned(&record.labels));
    record.labels = BTreeMap::from([(PINNED_LABEL.into(), String::new())]);
    assert!(!pinned(&record.labels));
    record.labels = BTreeMap::from([(PINNED_LABEL.into(), "1".into())]);
    assert!(pinned(&record.labels));
    assert!(row(&record, true).pinned);
}

#[test]
fn the_title_prefers_status_then_core_then_the_pane_name() {
    let mut record = terminal("t1", Some("working"), true, 0);
    record.title = "core title".into();
    record.status.as_mut().unwrap().title = "status title".into();
    assert_eq!(row(&record, true).title, "status title");
    record.status.as_mut().unwrap().title.clear();
    assert_eq!(row(&record, true).title, "core title");
    record.title.clear();
    assert_eq!(row(&record, true).title, "pane");
}

fn model(terminals: &[Terminal]) -> Model {
    Model::from_snapshot(&Snapshot {
        seq: 1,
        terminals: terminals.to_vec(),
        channel: Some(3),
    })
}

fn workspace_with(addresses: &[&str]) -> Workspace {
    let mut workspace = Workspace::new();
    for (index, address) in addresses.iter().enumerate() {
        let kind = if index == 0 {
            StartKind::Initial
        } else {
            StartKind::Split(crate::workspace::model::Axis::Columns)
        };
        let request = workspace.start_request(kind);
        workspace.insert_terminal(&request, (*address).into(), (*address).into());
    }
    workspace
}

#[test]
fn rows_are_ordered_pinned_first_then_by_priority_and_workspace() {
    let workspace = workspace_with(&["t1", "t2", "t3", "t4"]);
    let mut t1 = terminal("t1", Some("idle"), true, 0);
    let t2 = terminal("t2", Some("working"), true, 0);
    let mut t3 = terminal("t3", Some("done"), true, 1);
    let t4 = terminal("t4", Some("blocked"), true, 0);
    t1.labels = seen(0);
    t3.labels = BTreeMap::from([(PINNED_LABEL.into(), "1".into())]);
    let first_model = model(&[t1, t2, t3.clone(), t4.clone()]);

    let mut rows = workspace_rows(&workspace, &first_model, &BTreeMap::new());
    sort_rows(&mut rows);
    let order: Vec<&str> = rows.iter().map(|row| row.address.as_str()).collect();
    assert_eq!(
        order,
        ["t3", "t4", "t2", "t1"],
        "pinned, blocked, working, idle"
    );

    // Inside the pinned group the same priority order applies.
    let mut t2 = terminal("t2", Some("working"), true, 0);
    t2.labels = BTreeMap::from([(PINNED_LABEL.into(), "1".into())]);
    let mut t1 = terminal("t1", Some("idle"), true, 0);
    t1.labels = seen(0);
    let pinned_model = model(&[t1, t2, t3, t4]);
    let mut rows = workspace_rows(&workspace, &pinned_model, &BTreeMap::new());
    sort_rows(&mut rows);
    let order: Vec<&str> = rows.iter().map(|row| row.address.as_str()).collect();
    assert_eq!(order, ["t2", "t3", "t4", "t1"]);
}

#[test]
fn workspace_order_then_address_breaks_ties() {
    let workspace = workspace_with(&["t2", "t1"]);
    let model = model(&[
        terminal("t1", Some("idle"), true, 0),
        terminal("t2", Some("idle"), true, 0),
    ]);
    let mut rows = workspace_rows(&workspace, &model, &BTreeMap::new());
    sort_rows(&mut rows);
    assert_eq!(
        rows.iter()
            .map(|row| row.address.as_str())
            .collect::<Vec<_>>(),
        ["t2", "t1"],
        "layout order comes before the address"
    );

    // Same order key: the address decides deterministically.
    let mut rows = vec![
        row(&terminal("t2", Some("idle"), true, 0), true),
        row(&terminal("t1", Some("idle"), true, 0), true),
    ];
    for row in &mut rows {
        row.order = (0, 0, 0);
    }
    sort_rows(&mut rows);
    assert_eq!(rows[0].address, "t1");
    assert_eq!(rows[1].address, "t2");
}

#[test]
fn visible_panes_follow_tab_zoom_focus_and_overlays() {
    let mut workspace = workspace_with(&["t1", "t2"]);
    let first = workspace.panes().next().unwrap().id;
    let second = workspace.panes().nth(1).unwrap().id;

    assert_eq!(
        visible_panes(&workspace, true, Overlay::None),
        BTreeSet::from([first, second]),
        "the whole active tab is visible"
    );
    assert!(visible_panes(&workspace, false, Overlay::None).is_empty());

    workspace.toggle_zoom();
    assert_eq!(
        visible_panes(&workspace, true, Overlay::None),
        BTreeSet::from([second]),
        "zoom hides the other panes"
    );

    for overlay in [Overlay::Help, Overlay::Prompt, Overlay::Sidebar] {
        assert!(
            visible_panes(&workspace, true, overlay).is_empty(),
            "{overlay:?} covers the panes"
        );
    }
    assert_eq!(
        visible_panes(&workspace, true, Overlay::Peek(first)),
        BTreeSet::from([first]),
        "a peek looks only at its target"
    );

    // Another tab's panes are not visible.
    let request = workspace.start_request(StartKind::Tab);
    let third = workspace.insert_terminal(&request, "t3".into(), "t3".into());
    assert_eq!(
        visible_panes(&workspace, true, Overlay::None),
        BTreeSet::from([third])
    );
}

#[test]
fn seen_writes_only_raise_once_per_turn() {
    let mut record = terminal("t1", Some("done"), true, 2);
    let rows = vec![row(&record, false)];
    let visible = BTreeSet::from([PaneId(1)]);

    assert_eq!(
        seen_writes(&rows, &visible, &BTreeMap::new()),
        vec![SeenWrite {
            address: "t1".into(),
            turn: 2
        }],
        "a missing label writes the current turn"
    );
    assert!(
        seen_writes(&rows, &BTreeSet::new(), &BTreeMap::new()).is_empty(),
        "a hidden pane never writes"
    );

    record.labels = seen(2);
    let rows = vec![row(&record, true)];
    assert!(
        seen_writes(&rows, &visible, &BTreeMap::new()).is_empty(),
        "an already seen turn writes nothing"
    );

    record.labels = seen(1);
    let rows = vec![row(&record, false)];
    assert_eq!(seen_writes(&rows, &visible, &BTreeMap::new()).len(), 1);
    let attempted = BTreeMap::from([("t1".to_owned(), 2u64)]);
    assert!(
        seen_writes(&rows, &visible, &attempted).is_empty(),
        "focus away and back at the same turn does not write again"
    );

    record.turn = 3;
    let rows = vec![row(&record, false)];
    assert_eq!(
        seen_writes(&rows, &visible, &attempted)[0].turn,
        3,
        "a new turn writes"
    );

    record.labels = seen(4);
    let rows = vec![row(&record, true)];
    assert!(
        seen_writes(&rows, &visible, &BTreeMap::new()).is_empty(),
        "a seen label ahead of the turn is never lowered"
    );
}

#[test]
fn a_missing_label_at_turn_zero_writes_but_unread_does_not() {
    let record = terminal("t1", Some("idle"), true, 0);
    let rows = vec![row(&record, true)];
    let visible = BTreeSet::from([PaneId(1)]);
    assert_eq!(
        seen_writes(&rows, &visible, &BTreeMap::new()),
        vec![SeenWrite {
            address: "t1".into(),
            turn: 0
        }],
        "a missing label is not a label of 0"
    );
    assert_eq!(unread_value(0), None);
    assert_eq!(unread_value(1), Some(0));
    assert_eq!(unread_value(3), Some(2));
}

#[test]
fn local_seen_is_cleared_by_a_hidden_busy_to_idle_transition() {
    let working = terminal("t1", Some("working"), true, 0);
    assert!(update_local_seen(
        None,
        working.status.as_ref(),
        false,
        true
    ));
    assert!(update_local_seen(
        Some("working"),
        working.status.as_ref(),
        true,
        false
    ));

    let idle = terminal("t1", Some("done"), true, 1);
    assert!(
        !update_local_seen(Some("working"), idle.status.as_ref(), false, true),
        "a hidden finish is unseen"
    );
    assert!(
        update_local_seen(Some("working"), idle.status.as_ref(), true, false),
        "looking marks the turn seen"
    );
    assert!(
        !update_local_seen(Some("blocked"), None, false, true),
        "leaving blocked while hidden is also unseen"
    );
    assert!(
        !update_local_seen(Some("idle"), working.status.as_ref(), false, false),
        "an unrelated event does not mark a hidden idle agent seen"
    );
}

#[test]
fn alerts_fire_on_blocked_and_hidden_finishes_only() {
    assert_eq!(
        transition(Some("working"), "blocked", true),
        Some(Alert::NeedsYou)
    );
    assert_eq!(
        transition(Some("idle"), "blocked", false),
        Some(Alert::NeedsYou),
        "error shows as blocked and alerts from idle"
    );
    assert_eq!(
        transition(Some("working"), "done", false),
        Some(Alert::Finished)
    );
    assert_eq!(
        transition(Some("blocked"), "idle", false),
        Some(Alert::Finished)
    );
    assert_eq!(transition(Some("working"), "done", true), None);
    assert_eq!(transition(Some("done"), "done", false), None);
    assert_eq!(transition(Some("blocked"), "blocked", true), None);
    assert_eq!(
        transition(None, "blocked", false),
        Some(Alert::NeedsYou),
        "an agent that first appears blocked needs the person"
    );
    assert_eq!(transition(None, "working", false), None);
    assert_eq!(transition(None, "done", false), None);
    assert_eq!(transition(Some("idle"), "working", false), None);
}

#[test]
fn attention_counts_blocked_always_and_done_only_unseen() {
    let rows = vec![
        row(&terminal("t1", Some("blocked"), true, 1), false),
        row(&terminal("t2", Some("done"), true, 1), false),
        row(&terminal("t3", Some("done"), true, 1), true),
        row(&terminal("t4", Some("working"), true, 0), true),
    ];
    let attention = attention(&rows);
    assert_eq!(attention.blocked, 1);
    assert_eq!(attention.done, 1);
    assert_eq!(attention.text().as_deref(), Some("1 blocked · 1 done"));
    assert_eq!(Attention::default().text(), None);
}

#[test]
fn a_jump_cycles_and_starts_at_the_right_end() {
    let rows: Vec<AgentRow> = ["t1", "t2", "t3"]
        .iter()
        .map(|address| row(&terminal(address, Some("idle"), true, 0), true))
        .collect();
    assert_eq!(jump_target(&rows, None, Jump::Next), Some(PaneId(1)));
    assert_eq!(jump_target(&rows, None, Jump::Previous), Some(PaneId(1)));
    assert_eq!(
        jump_target(&rows, Some(PaneId(1)), Jump::Next),
        Some(PaneId(1)),
        "one row cycles to itself"
    );

    let mut rows = rows;
    rows[1].pane = PaneId(2);
    rows[2].pane = PaneId(3);
    assert_eq!(
        jump_target(&rows, Some(PaneId(1)), Jump::Next),
        Some(PaneId(2))
    );
    assert_eq!(
        jump_target(&rows, Some(PaneId(3)), Jump::Next),
        Some(PaneId(1))
    );
    assert_eq!(
        jump_target(&rows, Some(PaneId(1)), Jump::Previous),
        Some(PaneId(3))
    );
    assert_eq!(
        jump_target(&rows, Some(PaneId(99)), Jump::Next),
        Some(PaneId(1)),
        "a non-agent focus starts at the first row"
    );
    assert_eq!(
        jump_target(&rows, Some(PaneId(99)), Jump::Previous),
        Some(PaneId(3))
    );
    assert_eq!(jump_target(&[], None, Jump::Next), None);
}

#[test]
fn selection_survives_reordering_and_replaces_a_gone_pane() {
    let mut rows: Vec<AgentRow> = ["t1", "t2", "t3"]
        .iter()
        .map(|address| row(&terminal(address, Some("idle"), true, 0), true))
        .collect();
    rows[0].pane = PaneId(1);
    rows[1].pane = PaneId(2);
    rows[2].pane = PaneId(3);
    assert_eq!(
        selection_after(&rows, Some(PaneId(2)), Some(1)),
        Some(PaneId(2))
    );
    let without = vec![rows[0].clone(), rows[2].clone()];
    assert_eq!(
        selection_after(&without, Some(PaneId(2)), Some(1)),
        Some(PaneId(3)),
        "the row that took the old index is selected"
    );
    assert_eq!(
        selection_after(&without, Some(PaneId(2)), Some(9)),
        Some(PaneId(3)),
        "or the last row"
    );
    assert_eq!(selection_after(&[], Some(PaneId(2)), Some(1)), None);
}

#[test]
fn scroll_keeps_the_selection_visible() {
    assert_eq!(scroll_to(0, Some(10), 5, 20), 6);
    assert_eq!(scroll_to(6, Some(2), 5, 20), 2);
    assert_eq!(scroll_to(15, Some(0), 5, 20), 0);
    assert_eq!(
        scroll_to(3, Some(3), 5, 4),
        0,
        "short content does not scroll"
    );
    assert_eq!(scroll_to(0, Some(0), 0, 20), 0);
}

#[test]
fn label_maps_have_the_exact_shared_shapes() {
    assert_eq!(
        seen_labels(7),
        BTreeMap::from([(SEEN_LABEL.to_owned(), "7".to_owned())])
    );
    assert_eq!(
        pin_labels(true),
        BTreeMap::from([(PINNED_LABEL.to_owned(), "1".to_owned())])
    );
    assert_eq!(
        pin_labels(false),
        BTreeMap::from([(PINNED_LABEL.to_owned(), String::new())])
    );
    let value = serde_json::to_value(seen_labels(7)).unwrap();
    assert_eq!(value, serde_json::json!({"herdr.seen": "7"}));
}
