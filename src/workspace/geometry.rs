//! Pure pane geometry: the space rail, the bottom line, the narrow pane-nav
//! and the split rectangles.
//!
//! The workspace area is what is left between the chrome rows. Every split
//! first gives its children a minimum, then distributes the remaining cells by
//! weight; gaps shrink before panes do. Rectangles include the pane border;
//! the terminal grid is [`inner`]. On a wide terminal the rail takes the left
//! edge; on a narrow one the single pane-nav row sits above the pane area and
//! only the focused pane is drawn. One source of rectangles serves drawing,
//! hit testing, attach sizes and resize gaps.

use super::model::{Axis, Direction, Layout, PaneId, Tab};
use ratatui::layout::Rect;

/// The status bar's height.
pub const STATUS_HEIGHT: u16 = 1;
/// The narrow pane-nav's height.
pub const PANE_NAV_HEIGHT: u16 = 1;
/// The gap between siblings, when there is room.
pub const GAP: u16 = 1;
/// A pane's smallest outer size (a border and one cell on each axis).
pub const MIN_PANE: u16 = 3;

/// Where everything in the active tab goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rects {
    /// The space rail's full-height column on a wide terminal; zero on a
    /// narrow one (where it is an on-demand drawer instead).
    pub rail: Rect,
    /// The narrow pane-nav row; zero when the rail is visible or zoomed.
    pub pane_nav: Rect,
    /// The one-row bottom line, spanning the rail and the panes.
    pub status: Rect,
    /// The pane area.
    pub workspace: Rect,
    /// Outer pane rectangles (borders included), in tree order.
    pub panes: Vec<(PaneId, Rect)>,
}

impl Rects {
    pub fn pane(&self, pane: PaneId) -> Option<Rect> {
        self.panes
            .iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, rect)| *rect)
    }

    /// The pane under a screen cell; borders count, gaps do not.
    pub fn at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.panes
            .iter()
            .rev()
            .find(|(_, rect)| contains(*rect, x, y))
            .map(|(id, _)| *id)
    }
}

/// The chrome and the pane area for a viewport. The bottom line is always one
/// row across the whole viewport. A wide terminal puts the rail on the left
/// and starts the panes at row zero; a narrow one puts the pane-nav above the
/// panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bars {
    pub rail: Rect,
    pub pane_nav: Rect,
    pub status: Rect,
    pub workspace: Rect,
}

pub fn bars(viewport: Rect) -> Bars {
    let rail_width = super::sidebar::width(viewport);
    let wide = rail_width > 0;
    let status_height = STATUS_HEIGHT.min(viewport.height);
    let body_height = viewport.height.saturating_sub(status_height);
    let status = Rect::new(
        viewport.x,
        viewport.y.saturating_add(body_height),
        viewport.width,
        status_height,
    );
    let rail = if wide {
        Rect::new(viewport.x, viewport.y, rail_width, body_height)
    } else {
        Rect::new(viewport.x, viewport.y, 0, 0)
    };
    let pane_x = viewport.x.saturating_add(rail_width);
    let pane_width = viewport.width.saturating_sub(rail_width);
    let nav_height = if !wide && body_height > 1 {
        PANE_NAV_HEIGHT.min(body_height)
    } else {
        0
    };
    let pane_nav = if nav_height > 0 {
        Rect::new(pane_x, viewport.y, pane_width, nav_height)
    } else {
        Rect::new(pane_x, viewport.y, 0, 0)
    };
    let workspace = Rect::new(
        pane_x,
        viewport.y.saturating_add(nav_height),
        pane_width,
        body_height.saturating_sub(nav_height),
    );
    Bars {
        rail,
        pane_nav,
        status,
        workspace,
    }
}

/// The layout of a tab, honouring zoom and the narrow drawing rule.
pub fn compute(tab: Option<&Tab>, viewport: Rect) -> Rects {
    let bars = bars(viewport);
    let wide = bars.rail.width > 0;
    let mut pane_nav = bars.pane_nav;
    let mut workspace = bars.workspace;
    let mut panes = Vec::new();
    if let Some(tab) = tab
        && let Some(layout) = &tab.layout
    {
        match tab.zoomed {
            Some(zoomed) if layout.contains(zoomed) => {
                if !wide && pane_nav.height > 0 {
                    // Zoom may hide the pane-nav; the focused pane takes its
                    // row too.
                    workspace = Rect::new(
                        workspace.x,
                        pane_nav.y,
                        workspace.width,
                        workspace.height.saturating_add(pane_nav.height),
                    );
                    pane_nav = Rect::new(pane_nav.x, pane_nav.y, 0, 0);
                }
                panes.push((zoomed, workspace));
            }
            _ if wide => place(layout, workspace, &mut panes),
            _ => {
                // Narrow: only the focused pane is drawn; the rest keep
                // running and keep their last grid size.
                let drawn = tab
                    .focused
                    .filter(|pane| layout.contains(*pane))
                    .or_else(|| layout.first_leaf());
                if let Some(pane) = drawn {
                    panes.push((pane, workspace));
                }
            }
        }
    }
    Rects {
        rail: bars.rail,
        pane_nav,
        status: bars.status,
        workspace,
        panes,
    }
}

/// The full layout, ignoring zoom and the narrow drawing rule. Focus and move
/// directions use this so a zoomed or hidden pane can still be reached.
pub fn compute_without_zoom(tab: Option<&Tab>, viewport: Rect) -> Rects {
    let bars = bars(viewport);
    let mut panes = Vec::new();
    if let Some(tab) = tab
        && let Some(layout) = &tab.layout
    {
        place(layout, bars.workspace, &mut panes);
    }
    Rects {
        rail: bars.rail,
        pane_nav: bars.pane_nav,
        status: bars.status,
        workspace: bars.workspace,
        panes,
    }
}

/// A pane's number in split-tree order, starting at one. It is stable while
/// focus moves and while a narrow window draws only the focused pane.
pub fn pane_number(tab: Option<&Tab>, pane: PaneId) -> Option<usize> {
    tab?.panes()
        .iter()
        .position(|candidate| *candidate == pane)
        .map(|index| index + 1)
}

/// A draggable split boundary: one gap cell column or row between two
/// children of the split at `path`, child `index` before and `index + 1`
/// after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub axis: Axis,
    /// Child indices from the tab root to the split that owns the gap.
    pub path: Vec<usize>,
    /// The child before the gap in that split.
    pub index: usize,
    pub rect: Rect,
}

/// Every split boundary with a visible gap, in tree order. Zoom is ignored:
/// the boundaries of a zoomed layout are not on screen.
pub fn gaps(tab: Option<&Tab>, viewport: Rect) -> Vec<Gap> {
    let mut out = Vec::new();
    let workspace = bars(viewport).workspace;
    if let Some(tab) = tab
        && let Some(layout) = &tab.layout
    {
        match tab.zoomed {
            Some(zoomed) if layout.contains(zoomed) => {}
            _ => collect_gaps(layout, workspace, &mut Vec::new(), &mut out),
        }
    }
    out
}

/// The split boundary under a screen cell, if any.
pub fn gap_at(tab: Option<&Tab>, viewport: Rect, x: u16, y: u16) -> Option<Gap> {
    gaps(tab, viewport)
        .into_iter()
        .find(|gap| contains(gap.rect, x, y))
}

fn collect_gaps(layout: &Layout, area: Rect, path: &mut Vec<usize>, out: &mut Vec<Gap>) {
    let Layout::Split { axis, children } = layout else {
        return;
    };
    let length = match axis {
        Axis::Columns => area.width,
        Axis::Rows => area.height,
    };
    let weights: Vec<u32> = children.iter().map(|child| child.size).collect();
    let (spans, gap) = distribute(length, &weights);
    let mut at = 0u16;
    for (index, child) in children.iter().enumerate() {
        let span = spans.get(index).copied().unwrap_or(0);
        let rect = match axis {
            Axis::Columns => Rect::new(area.x.saturating_add(at), area.y, span, area.height),
            Axis::Rows => Rect::new(area.x, area.y.saturating_add(at), area.width, span),
        };
        if gap > 0 && index + 1 < children.len() {
            let boundary = match axis {
                Axis::Columns => Rect::new(
                    area.x.saturating_add(at).saturating_add(span),
                    area.y,
                    gap,
                    area.height,
                ),
                Axis::Rows => Rect::new(
                    area.x,
                    area.y.saturating_add(at).saturating_add(span),
                    area.width,
                    gap,
                ),
            };
            out.push(Gap {
                axis: *axis,
                path: path.clone(),
                index,
                rect: boundary,
            });
        }
        path.push(index);
        collect_gaps(&child.layout, rect, path, out);
        path.pop();
        at = at.saturating_add(span).saturating_add(gap);
    }
}

/// A pane's terminal grid: the rectangle inside its border, when it has room.
pub fn inner(rect: Rect) -> Option<Rect> {
    if rect.width > 2 && rect.height > 2 {
        Some(Rect::new(
            rect.x.saturating_add(1),
            rect.y.saturating_add(1),
            rect.width - 2,
            rect.height - 2,
        ))
    } else {
        None
    }
}

/// The size to attach a terminal at. Even a pane too small to draw keeps a
/// 1x1 terminal so its program still has a valid size.
pub fn attach_size(rect: Rect) -> (u16, u16) {
    inner(rect)
        .map(|inner| (inner.width.max(1), inner.height.max(1)))
        .unwrap_or((1, 1))
}

/// One keyboard resize step: the split boundary to move and its direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResizeStep {
    /// Child indices from the tab root to the split that owns the boundary.
    pub path: Vec<usize>,
    /// The child before the boundary in that split.
    pub index: usize,
    /// `+1` moves the boundary right/down, `-1` left/up.
    pub delta: i16,
}

/// The split boundary the focused pane should move for a keyboard resize.
/// `forward` is the pressed direction (right or down). The deepest split on
/// the pane's route that runs along `axis` wins, so a nested grid moves its
/// local boundary before an outer one. When the pane has a sibling on the
/// pressed side, that boundary is used; otherwise the opposite side is, so
/// every key still acts on the focused pane when possible.
pub fn resize_target(tab: &Tab, focused: PaneId, axis: Axis, forward: bool) -> Option<ResizeStep> {
    if tab.zoomed.is_some() {
        return None;
    }
    let root = tab.layout.as_ref()?;
    let mut route = Vec::new();
    if !route_to(root, focused, &mut route) || route.is_empty() {
        return None;
    }
    for depth in (0..route.len()).rev() {
        let Some(Layout::Split {
            axis: split_axis,
            children,
        }) = walk(root, &route[..depth])
        else {
            continue;
        };
        if *split_axis != axis {
            continue;
        }
        let child = route[depth];
        let preferred = if forward {
            (child + 1 < children.len()).then_some(child)
        } else {
            child.checked_sub(1)
        };
        let fallback = if forward {
            child.checked_sub(1)
        } else {
            (child + 1 < children.len()).then_some(child)
        };
        if let Some(index) = preferred.or(fallback) {
            return Some(ResizeStep {
                path: route[..depth].to_vec(),
                index,
                delta: if forward { 1 } else { -1 },
            });
        }
    }
    None
}

fn route_to(layout: &Layout, target: PaneId, path: &mut Vec<usize>) -> bool {
    match layout {
        Layout::Leaf(id) => *id == target,
        Layout::Split { children, .. } => {
            for (index, child) in children.iter().enumerate() {
                path.push(index);
                if route_to(&child.layout, target, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
    }
}

fn walk<'a>(layout: &'a Layout, path: &[usize]) -> Option<&'a Layout> {
    let mut node = layout;
    for &index in path {
        let Layout::Split { children, .. } = node else {
            return None;
        };
        node = &children.get(index)?.layout;
    }
    Some(node)
}

/// The nearest pane whose centre is in `direction` from `from`. Centres make
/// the choice stable across uneven splits; ties prefer the pane that is more
/// directly in line, then the lower id.
pub fn neighbor(rects: &Rects, from: PaneId, direction: Direction) -> Option<PaneId> {
    let source = rects.pane(from)?;
    let (sx, sy) = centre(source);
    rects
        .panes
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, rect)| {
            let (x, y) = centre(*rect);
            let (ahead, primary, sideways) = match direction {
                Direction::Left => (x < sx, sx - x, (sy - y).abs()),
                Direction::Right => (x > sx, x - sx, (sy - y).abs()),
                Direction::Up => (y < sy, sy - y, (sx - x).abs()),
                Direction::Down => (y > sy, y - sy, (sx - x).abs()),
            };
            ahead.then_some((*id, primary * primary + sideways * sideways, sideways))
        })
        .min_by_key(|(id, distance, sideways)| (*distance, *sideways, *id))
        .map(|(id, _, _)| id)
}

fn centre(rect: Rect) -> (i64, i64) {
    (
        2 * i64::from(rect.x) + i64::from(rect.width),
        2 * i64::from(rect.y) + i64::from(rect.height),
    )
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    let x = u32::from(x);
    let y = u32::from(y);
    x >= u32::from(rect.x)
        && x < u32::from(rect.x) + u32::from(rect.width)
        && y >= u32::from(rect.y)
        && y < u32::from(rect.y) + u32::from(rect.height)
}

fn place(layout: &Layout, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match layout {
        Layout::Leaf(id) => out.push((*id, area)),
        Layout::Split { axis, children } => {
            let length = match axis {
                Axis::Columns => area.width,
                Axis::Rows => area.height,
            };
            let weights: Vec<u32> = children.iter().map(|child| child.size).collect();
            let (spans, gap) = distribute(length, &weights);
            let mut at = 0u16;
            for (index, child) in children.iter().enumerate() {
                let span = spans.get(index).copied().unwrap_or(0);
                let rect = match axis {
                    Axis::Columns => {
                        Rect::new(area.x.saturating_add(at), area.y, span, area.height)
                    }
                    Axis::Rows => Rect::new(area.x, area.y.saturating_add(at), area.width, span),
                };
                place(&child.layout, rect, out);
                at = at.saturating_add(span);
                if index + 1 < children.len() {
                    at = at.saturating_add(gap);
                }
            }
        }
    }
}

/// Splits `length` into one span per weight, with `gap` between siblings.
/// Every child gets the minimum when it fits; the rest is proportional to the
/// weights, with the leftover cells going to the largest fractional parts.
/// A zero weight is a child pinned at the minimum: that is how a drag stores
/// an exact minimum after [`MIN_PANE`] cells, and it keeps later resizes at
/// the same boundary.
pub(super) fn distribute(length: u16, weights: &[u32]) -> (Vec<u16>, u16) {
    let count = weights.len();
    if count == 0 {
        return (Vec::new(), 0);
    }
    if count == 1 {
        return (vec![length], 0);
    }
    let max_gap = length.saturating_sub(count as u16);
    let gap = GAP.min(max_gap / (count as u16 - 1));
    let available = u32::from(length.saturating_sub(gap * (count as u16 - 1)));
    let count = count as u32;
    let minimum = u32::from(MIN_PANE).min(available / count);
    let rest = available - minimum * count;
    let total: u32 = weights.iter().copied().sum::<u32>().max(1);
    if weights.iter().all(|weight| *weight == 0) {
        // No ratio to preserve: share the remainder evenly.
        let base = rest / count;
        let extra = rest % count;
        let spans = (0..count)
            .map(|index| {
                let add = if index < extra { 1 } else { 0 };
                (minimum + base + add) as u16
            })
            .collect();
        return (spans, gap);
    }
    let mut spans: Vec<u32> = Vec::with_capacity(count as usize);
    let mut fractions: Vec<(usize, u32)> = Vec::with_capacity(count as usize);
    for (index, weight) in weights.iter().copied().enumerate() {
        let share = rest * weight / total;
        spans.push(minimum + share);
        fractions.push((index, rest * weight % total));
    }
    let mut assigned: u32 = spans.iter().sum();
    let mut leftover = available - assigned;
    fractions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (index, _) in fractions {
        if leftover == 0 {
            break;
        }
        spans[index] += 1;
        assigned += 1;
        leftover -= 1;
    }
    debug_assert_eq!(assigned, available);
    (spans.into_iter().map(|span| span as u16).collect(), gap)
}

#[cfg(test)]
mod tests;
