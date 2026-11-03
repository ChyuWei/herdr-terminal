use super::*;
use crate::workspace::model::{Axis, Child, PaneId, TabId};

fn leaf(id: u64) -> Layout {
    Layout::Leaf(PaneId(id))
}

fn split(axis: Axis, children: Vec<Layout>) -> Layout {
    Layout::Split {
        axis,
        children: children.into_iter().map(Child::new).collect(),
    }
}

fn make_tab(layout: Layout) -> Tab {
    let mut tab = Tab::new(TabId(1), "tab".into());
    tab.focused = layout.first_leaf();
    tab.layout = Some(layout);
    tab
}

#[test]
fn bars_and_borders_leave_the_terminal_grid_inside_them() {
    let layout = leaf(1);
    let tab = make_tab(layout);
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(rects.tab_bar, Rect::new(0, 0, 80, 1));
    assert_eq!(rects.status, Rect::new(0, 23, 80, 1));
    assert_eq!(rects.workspace, Rect::new(0, 1, 80, 22));
    assert_eq!(rects.panes, vec![(PaneId(1), Rect::new(0, 1, 80, 22))]);
    assert_eq!(
        rects.pane(PaneId(1)).and_then(inner),
        Some(Rect::new(1, 2, 78, 20))
    );
    assert_eq!(attach_size(Rect::new(0, 1, 80, 22)), (78, 20));
}

#[test]
fn columns_and_rows_share_cells_with_a_gap() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(0, 1, 40, 22)),
            (PaneId(2), Rect::new(41, 1, 39, 22)),
        ]
    );

    let tab = make_tab(split(Axis::Rows, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(0, 1, 80, 11)),
            (PaneId(2), Rect::new(0, 13, 80, 10)),
        ]
    );
}

#[test]
fn a_nested_grid_is_stable_at_a_small_size() {
    let tab = make_tab(split(
        Axis::Rows,
        vec![
            split(Axis::Columns, vec![leaf(1), leaf(2)]),
            split(Axis::Columns, vec![leaf(3), leaf(4)]),
        ],
    ));
    let rects = compute(Some(&tab), Rect::new(0, 0, 40, 12));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(0, 1, 20, 5)),
            (PaneId(2), Rect::new(21, 1, 19, 5)),
            (PaneId(3), Rect::new(0, 7, 20, 4)),
            (PaneId(4), Rect::new(21, 7, 19, 4)),
        ]
    );
    assert_eq!(inner(Rect::new(0, 1, 20, 5)), Some(Rect::new(1, 2, 18, 3)));
}

#[test]
fn weights_change_only_the_extra_space() {
    let mut layout = split(Axis::Columns, vec![leaf(1), leaf(2)]);
    let Layout::Split { children, .. } = &mut layout else {
        unreachable!()
    };
    children[0].size = 1;
    children[1].size = 3;
    let tab = make_tab(layout);
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(rects.panes[0].1.width, 21);
    assert_eq!(rects.panes[1].1.width, 58);
    assert_eq!(rects.panes[0].1.width + 1 + rects.panes[1].1.width, 80);
}

#[test]
fn zoom_shows_only_the_focused_pane_but_directions_use_the_full_layout() {
    let mut tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    tab.zoomed = Some(PaneId(2));
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(rects.panes, vec![(PaneId(2), Rect::new(0, 1, 80, 22))]);
    let full = compute_without_zoom(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(full.panes.len(), 2);
    assert_eq!(neighbor(&full, PaneId(2), Direction::Left), Some(PaneId(1)));
}

#[test]
fn tiny_viewports_clamp_gaps_before_panes() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let viewport = Rect::new(0, 0, 5, 3);
    let rects = compute(Some(&tab), viewport);
    assert_eq!(rects.tab_bar, Rect::new(0, 0, 5, 1));
    assert_eq!(rects.status, Rect::new(0, 2, 5, 1));
    assert_eq!(rects.workspace, Rect::new(0, 1, 5, 1));
    assert_eq!(rects.panes.len(), 2);
    assert_eq!(rects.panes[0].1, Rect::new(0, 1, 2, 1));
    assert_eq!(rects.panes[1].1, Rect::new(3, 1, 2, 1));
    assert_eq!(inner(rects.panes[0].1), None);
    assert_eq!(attach_size(rects.panes[0].1), (1, 1));

    let rects = compute(Some(&tab), Rect::new(0, 0, 1, 1));
    assert_eq!(rects.workspace, Rect::new(0, 1, 1, 0));
    assert_eq!(rects.status, Rect::new(0, 1, 1, 0));
    assert!(
        rects
            .panes
            .iter()
            .all(|(_, rect)| rect.width == 0 || rect.height == 0)
    );
}

#[test]
fn hit_testing_includes_borders_but_not_gaps() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(rects.at(0, 1), Some(PaneId(1)), "top-left border");
    assert_eq!(rects.at(39, 22), Some(PaneId(1)), "bottom-right border");
    assert_eq!(rects.at(40, 10), None, "the gap");
    assert_eq!(rects.at(41, 1), Some(PaneId(2)));
    assert_eq!(rects.at(79, 22), Some(PaneId(2)));
    assert_eq!(rects.at(0, 0), None, "the tab bar");
    assert_eq!(rects.at(80, 10), None);
}

#[test]
fn directions_pick_the_nearest_aligned_pane() {
    let tab = make_tab(split(
        Axis::Rows,
        vec![
            split(Axis::Columns, vec![leaf(1), leaf(2)]),
            split(Axis::Columns, vec![leaf(3), leaf(4)]),
        ],
    ));
    let rects = compute_without_zoom(Some(&tab), Rect::new(0, 0, 40, 12));
    assert_eq!(
        neighbor(&rects, PaneId(1), Direction::Right),
        Some(PaneId(2))
    );
    assert_eq!(
        neighbor(&rects, PaneId(1), Direction::Down),
        Some(PaneId(3))
    );
    assert_eq!(neighbor(&rects, PaneId(4), Direction::Up), Some(PaneId(2)));
    assert_eq!(
        neighbor(&rects, PaneId(2), Direction::Left),
        Some(PaneId(1))
    );
    assert_eq!(neighbor(&rects, PaneId(1), Direction::Left), None);
    assert_eq!(neighbor(&rects, PaneId(1), Direction::Up), None);
    assert_eq!(neighbor(&rects, PaneId(999), Direction::Down), None);
}

#[test]
fn a_tab_without_a_layout_has_only_bars() {
    let tab = Tab::new(TabId(1), "empty".into());
    let rects = compute(Some(&tab), Rect::new(2, 3, 20, 8));
    assert_eq!(rects.tab_bar, Rect::new(2, 3, 20, 1));
    assert_eq!(rects.status, Rect::new(2, 10, 20, 1));
    assert_eq!(rects.workspace, Rect::new(2, 4, 20, 6));
    assert!(rects.panes.is_empty());
    assert!(compute(None, Rect::new(0, 0, 20, 8)).panes.is_empty());
}

#[test]
fn split_boundaries_are_hit_testable_on_the_gap_cell() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let viewport = Rect::new(0, 0, 80, 24);
    let boundaries = gaps(Some(&tab), viewport);
    assert_eq!(boundaries.len(), 1, "{boundaries:?}");
    assert_eq!(boundaries[0].axis, Axis::Columns);
    assert_eq!(boundaries[0].path, Vec::<usize>::new());
    assert_eq!(boundaries[0].index, 0);
    assert_eq!(boundaries[0].rect, Rect::new(40, 1, 1, 22));
    assert_eq!(
        gap_at(Some(&tab), viewport, 40, 10),
        Some(boundaries[0].clone())
    );
    assert_eq!(gap_at(Some(&tab), viewport, 39, 10), None);
    assert_eq!(gap_at(Some(&tab), viewport, 41, 10), None);
    assert_eq!(gap_at(Some(&tab), viewport, 40, 0), None, "tab bar");

    let nested = make_tab(split(
        Axis::Rows,
        vec![split(Axis::Columns, vec![leaf(1), leaf(2)]), leaf(3)],
    ));
    let nested_gaps = gaps(Some(&nested), viewport);
    assert_eq!(nested_gaps.len(), 2, "{nested_gaps:?}");
    assert_eq!(nested_gaps[1].axis, Axis::Columns);
    assert_eq!(nested_gaps[1].path, vec![0]);
    assert_eq!(nested_gaps[1].index, 0);
    assert_eq!(nested_gaps[1].rect.y, 1);
    assert_eq!(nested_gaps[1].rect.height, 11);
}

#[test]
fn a_zoomed_pane_has_no_draggable_boundaries() {
    let mut tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    tab.zoomed = Some(PaneId(2));
    assert!(gaps(Some(&tab), Rect::new(0, 0, 80, 24)).is_empty());
}

#[test]
fn tab_spans_match_the_drawn_tab_bar_and_hit_the_right_tab() {
    let mut space = Space {
        id: super::super::model::SpaceId(1),
        name: "space 1".into(),
        folder: String::new(),
        tabs: Vec::new(),
        active: TabId(1),
    };
    space.tabs.push(Tab::new(TabId(1), "tab 1".into()));
    space.tabs.push(Tab::new(TabId(2), "tab 2".into()));
    space.active = TabId(2);
    let area = Rect::new(0, 0, 80, 1);
    let spans = tab_spans(&space, area);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0], (TabId(1), Rect::new(9, 0, 9, 1)));
    assert_eq!(spans[1], (TabId(2), Rect::new(18, 0, 9, 1)));
    assert_eq!(tab_at(&space, area, 9, 0), Some(TabId(1)));
    assert_eq!(tab_at(&space, area, 17, 0), Some(TabId(1)));
    assert_eq!(tab_at(&space, area, 18, 0), Some(TabId(2)));
    assert_eq!(
        tab_at(&space, area, 8, 0),
        None,
        "the space name is not a tab"
    );
    assert_eq!(tab_at(&space, area, 27, 0), None, "past the last tab");

    let narrow = tab_spans(&space, Rect::new(0, 0, 20, 1));
    assert_eq!(narrow.len(), 2);
    assert_eq!(
        narrow[1].1.width, 2,
        "a clipped tab keeps its visible cells"
    );
}
