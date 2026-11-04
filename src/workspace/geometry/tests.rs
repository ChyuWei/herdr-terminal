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
fn a_wide_terminal_puts_the_rail_beside_the_panes_and_one_status_row_under_all() {
    let tab = make_tab(leaf(1));
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(rects.rail, Rect::new(0, 0, 30, 39));
    assert_eq!(
        rects.pane_nav,
        Rect::new(30, 0, 0, 0),
        "no pane-nav when wide"
    );
    assert_eq!(rects.status, Rect::new(0, 39, 120, 1));
    assert_eq!(rects.workspace, Rect::new(30, 0, 90, 39));
    assert_eq!(rects.panes, vec![(PaneId(1), Rect::new(30, 0, 90, 39))]);
    assert_eq!(
        rects.pane(PaneId(1)).and_then(inner),
        Some(Rect::new(31, 1, 88, 37))
    );
    assert_eq!(attach_size(Rect::new(30, 0, 90, 39)), (88, 37));
}

#[test]
fn the_wide_threshold_is_exactly_ninety_columns() {
    let tab = make_tab(leaf(1));
    let narrow = compute(Some(&tab), Rect::new(0, 0, 89, 24));
    assert_eq!(narrow.rail, Rect::new(0, 0, 0, 0));
    assert_eq!(narrow.pane_nav, Rect::new(0, 0, 89, 1));
    assert_eq!(narrow.workspace, Rect::new(0, 1, 89, 22));

    let wide = compute(Some(&tab), Rect::new(0, 0, 90, 24));
    assert_eq!(wide.rail, Rect::new(0, 0, 22, 23));
    assert_eq!(wide.pane_nav, Rect::new(22, 0, 0, 0));
    assert_eq!(wide.workspace, Rect::new(22, 0, 68, 23));
    assert_eq!(wide.status, Rect::new(0, 23, 90, 1));
}

#[test]
fn a_narrow_terminal_draws_only_the_focused_pane_and_maps_the_rest() {
    let mut tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2), leaf(3)]));
    tab.focused = Some(PaneId(2));
    let rects = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(rects.rail.width, 0);
    assert_eq!(rects.status, Rect::new(0, 23, 80, 1));
    assert_eq!(rects.pane_nav, Rect::new(0, 0, 80, 1));
    assert_eq!(rects.workspace, Rect::new(0, 1, 80, 22));
    assert_eq!(
        rects.panes,
        vec![(PaneId(2), Rect::new(0, 1, 80, 22))],
        "only the focused pane is drawn"
    );
    // The full layout is still what focus and move directions use.
    let full = compute_without_zoom(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(full.panes.len(), 3);
    assert_eq!(pane_number(Some(&tab), PaneId(2)), Some(2));
    assert_eq!(pane_number(Some(&tab), PaneId(3)), Some(3));
    assert_eq!(pane_number(Some(&tab), PaneId(9)), None);
}

#[test]
fn a_zoom_takes_the_workspace_and_hides_the_narrow_pane_nav() {
    let mut tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    tab.zoomed = Some(PaneId(2));
    let narrow = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(narrow.pane_nav, Rect::new(0, 0, 0, 0));
    assert_eq!(narrow.workspace, Rect::new(0, 0, 80, 23));
    assert_eq!(narrow.panes, vec![(PaneId(2), Rect::new(0, 0, 80, 23))]);

    let wide = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(wide.panes, vec![(PaneId(2), Rect::new(30, 0, 90, 39))]);
    let full = compute_without_zoom(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(full.panes.len(), 2);
    assert_eq!(neighbor(&full, PaneId(2), Direction::Left), Some(PaneId(1)));
}

#[test]
fn the_narrow_drawer_does_not_shrink_the_panes() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let before = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    let drawer = crate::workspace::sidebar::drawer_rect(Rect::new(0, 0, 80, 24));
    assert_eq!(
        drawer,
        Rect::new(0, 0, 20, 23),
        "the drawer ends above the status row"
    );
    let after = compute(Some(&tab), Rect::new(0, 0, 80, 24));
    assert_eq!(
        before, after,
        "opening the drawer draws over the panes; it never changes them"
    );
}

#[test]
fn columns_and_rows_share_cells_with_a_gap() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(30, 0, 45, 39)),
            (PaneId(2), Rect::new(76, 0, 44, 39)),
        ]
    );

    let tab = make_tab(split(Axis::Rows, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(30, 0, 90, 19)),
            (PaneId(2), Rect::new(30, 20, 90, 19)),
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
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(
        rects.panes,
        vec![
            (PaneId(1), Rect::new(30, 0, 45, 19)),
            (PaneId(2), Rect::new(76, 0, 44, 19)),
            (PaneId(3), Rect::new(30, 20, 45, 19)),
            (PaneId(4), Rect::new(76, 20, 44, 19)),
        ]
    );
    assert_eq!(
        inner(Rect::new(30, 0, 45, 19)),
        Some(Rect::new(31, 1, 43, 17))
    );
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
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(rects.panes[0].1.width, 24);
    assert_eq!(rects.panes[1].1.width, 65);
    assert_eq!(rects.panes[0].1.width + 1 + rects.panes[1].1.width, 90);
}

#[test]
fn tiny_viewports_clamp_gaps_before_panes() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let viewport = Rect::new(0, 0, 5, 3);
    let rects = compute(Some(&tab), viewport);
    assert_eq!(rects.pane_nav, Rect::new(0, 0, 5, 1));
    assert_eq!(rects.status, Rect::new(0, 2, 5, 1));
    assert_eq!(rects.workspace, Rect::new(0, 1, 5, 1));
    assert_eq!(rects.panes, vec![(PaneId(1), Rect::new(0, 1, 5, 1))]);
    let full = compute_without_zoom(Some(&tab), viewport);
    assert_eq!(full.panes.len(), 2);
    assert_eq!(full.panes[0].1, Rect::new(0, 1, 2, 1));
    assert_eq!(full.panes[1].1, Rect::new(3, 1, 2, 1));
    assert_eq!(inner(full.panes[0].1), None);
    assert_eq!(attach_size(full.panes[0].1), (1, 1));

    let rects = compute(Some(&tab), Rect::new(0, 0, 1, 1));
    assert_eq!(rects.status, Rect::new(0, 0, 1, 1));
    assert_eq!(rects.workspace, Rect::new(0, 0, 1, 0));
    assert_eq!(rects.pane_nav, Rect::new(0, 0, 0, 0));
    assert!(
        rects
            .panes
            .iter()
            .all(|(_, rect)| rect.width == 0 || rect.height == 0)
    );
}

#[test]
fn hit_testing_includes_borders_but_not_gaps_or_chrome() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let rects = compute(Some(&tab), Rect::new(0, 0, 120, 40));
    assert_eq!(rects.at(30, 0), Some(PaneId(1)), "top-left border");
    assert_eq!(rects.at(74, 38), Some(PaneId(1)), "bottom-right border");
    assert_eq!(rects.at(75, 10), None, "the gap");
    assert_eq!(rects.at(76, 0), Some(PaneId(2)));
    assert_eq!(rects.at(119, 38), Some(PaneId(2)));
    assert_eq!(rects.at(30, 39), None, "the bottom line");
    assert_eq!(rects.at(120, 10), None);
    assert_eq!(rects.at(0, 10), None, "the rail is not a pane");
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
    let rects = compute_without_zoom(Some(&tab), Rect::new(0, 0, 120, 40));
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
fn a_tab_without_a_layout_has_only_chrome() {
    let tab = Tab::new(TabId(1), "empty".into());
    let rects = compute(Some(&tab), Rect::new(2, 3, 20, 8));
    assert_eq!(rects.rail, Rect::new(2, 3, 0, 0));
    assert_eq!(rects.pane_nav, Rect::new(2, 3, 20, 1));
    assert_eq!(rects.status, Rect::new(2, 10, 20, 1));
    assert_eq!(rects.workspace, Rect::new(2, 4, 20, 6));
    assert!(rects.panes.is_empty());
    assert!(compute(None, Rect::new(0, 0, 20, 8)).panes.is_empty());
}

#[test]
fn split_boundaries_are_hit_testable_on_the_gap_cell() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    let viewport = Rect::new(0, 0, 120, 40);
    let boundaries = gaps(Some(&tab), viewport);
    assert_eq!(boundaries.len(), 1, "{boundaries:?}");
    assert_eq!(boundaries[0].axis, Axis::Columns);
    assert_eq!(boundaries[0].path, Vec::<usize>::new());
    assert_eq!(boundaries[0].index, 0);
    assert_eq!(boundaries[0].rect, Rect::new(75, 0, 1, 39));
    assert_eq!(
        gap_at(Some(&tab), viewport, 75, 10),
        Some(boundaries[0].clone())
    );
    assert_eq!(gap_at(Some(&tab), viewport, 74, 10), None);
    assert_eq!(gap_at(Some(&tab), viewport, 76, 10), None);
    assert_eq!(gap_at(Some(&tab), viewport, 75, 39), None, "bottom line");

    let nested = make_tab(split(
        Axis::Rows,
        vec![split(Axis::Columns, vec![leaf(1), leaf(2)]), leaf(3)],
    ));
    let nested_gaps = gaps(Some(&nested), viewport);
    assert_eq!(nested_gaps.len(), 2, "{nested_gaps:?}");
    assert_eq!(nested_gaps[1].axis, Axis::Columns);
    assert_eq!(nested_gaps[1].path, vec![0]);
    assert_eq!(nested_gaps[1].index, 0);
    assert_eq!(nested_gaps[1].rect.y, 0);
    assert_eq!(nested_gaps[1].rect.height, 19);
}

#[test]
fn a_zoomed_pane_has_no_draggable_boundaries() {
    let mut tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    tab.zoomed = Some(PaneId(2));
    assert!(gaps(Some(&tab), Rect::new(0, 0, 120, 40)).is_empty());
}

#[test]
fn pane_numbers_keep_split_tree_order_when_the_view_is_narrow() {
    let tab = make_tab(split(
        Axis::Rows,
        vec![split(Axis::Columns, vec![leaf(4), leaf(5)]), leaf(6)],
    ));
    assert_eq!(pane_number(Some(&tab), PaneId(4)), Some(1));
    assert_eq!(pane_number(Some(&tab), PaneId(5)), Some(2));
    assert_eq!(pane_number(Some(&tab), PaneId(6)), Some(3));
    assert_eq!(pane_number(None, PaneId(4)), None);
}

#[test]
fn keyboard_resize_picks_the_deepest_matching_split_on_the_focused_route() {
    let tab = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    assert_eq!(
        resize_target(&tab, PaneId(1), Axis::Columns, true),
        Some(ResizeStep {
            path: Vec::new(),
            index: 0,
            delta: 1
        })
    );
    assert_eq!(
        resize_target(&tab, PaneId(1), Axis::Columns, false),
        Some(ResizeStep {
            path: Vec::new(),
            index: 0,
            delta: -1
        }),
        "the only boundary is on the right, so it moves left"
    );
    assert_eq!(
        resize_target(&tab, PaneId(2), Axis::Columns, false),
        Some(ResizeStep {
            path: Vec::new(),
            index: 0,
            delta: -1
        })
    );
    assert_eq!(
        resize_target(&tab, PaneId(1), Axis::Rows, true),
        None,
        "there is no row split"
    );

    // A nested grid moves its local boundary before the outer one.
    let nested = make_tab(split(
        Axis::Rows,
        vec![split(Axis::Columns, vec![leaf(1), leaf(2)]), leaf(3)],
    ));
    assert_eq!(
        resize_target(&nested, PaneId(1), Axis::Columns, true),
        Some(ResizeStep {
            path: vec![0],
            index: 0,
            delta: 1
        })
    );
    assert_eq!(
        resize_target(&nested, PaneId(1), Axis::Rows, true),
        Some(ResizeStep {
            path: Vec::new(),
            index: 0,
            delta: 1
        })
    );
    assert_eq!(
        resize_target(&nested, PaneId(3), Axis::Rows, false),
        Some(ResizeStep {
            path: Vec::new(),
            index: 0,
            delta: -1
        })
    );

    let mut zoomed = make_tab(split(Axis::Columns, vec![leaf(1), leaf(2)]));
    zoomed.zoomed = Some(PaneId(1));
    assert_eq!(
        resize_target(&zoomed, PaneId(1), Axis::Columns, true),
        None,
        "a zoomed tab has no visible boundaries"
    );
}
