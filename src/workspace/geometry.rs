//! Pure pane geometry: the tab bar, the status bar and the split rectangles.
//!
//! The workspace area is what is left between the bars. Every split first
//! gives its children a minimum, then distributes the remaining cells by
//! weight; gaps shrink before panes do. Rectangles include the pane border;
//! the terminal grid is [`inner`].

use super::model::{Axis, Direction, Layout, PaneId, Space, Tab, TabId};
use ratatui::layout::Rect;
use ratatui::text::Span;

/// The tab bar's height.
pub const TAB_BAR_HEIGHT: u16 = 1;
/// The status bar's height.
pub const STATUS_HEIGHT: u16 = 1;
/// The gap between siblings, when there is room.
pub const GAP: u16 = 1;
/// A pane's smallest outer size (a border and one cell on each axis).
pub const MIN_PANE: u16 = 3;

/// Where everything in the active tab goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rects {
    pub tab_bar: Rect,
    pub status: Rect,
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

/// The tab bar, status bar and workspace area for a viewport.
pub fn bars(viewport: Rect) -> (Rect, Rect, Rect) {
    let tab_height = TAB_BAR_HEIGHT.min(viewport.height);
    let status_height = if viewport.height > tab_height {
        STATUS_HEIGHT.min(viewport.height - tab_height)
    } else {
        0
    };
    let tab_bar = Rect::new(viewport.x, viewport.y, viewport.width, tab_height);
    let status = Rect::new(
        viewport.x,
        viewport
            .y
            .saturating_add(viewport.height)
            .saturating_sub(status_height),
        viewport.width,
        status_height,
    );
    let workspace = Rect::new(
        viewport.x,
        viewport.y.saturating_add(tab_height),
        viewport.width,
        viewport
            .height
            .saturating_sub(tab_height)
            .saturating_sub(status_height),
    );
    (tab_bar, status, workspace)
}

/// The layout of a tab, honouring zoom.
pub fn compute(tab: Option<&Tab>, viewport: Rect) -> Rects {
    let (tab_bar, status, workspace) = bars(viewport);
    let mut panes = Vec::new();
    if let Some(tab) = tab
        && let Some(layout) = &tab.layout
    {
        match tab.zoomed {
            Some(zoomed) if layout.contains(zoomed) => panes.push((zoomed, workspace)),
            _ => place(layout, workspace, &mut panes),
        }
    }
    Rects {
        tab_bar,
        status,
        workspace,
        panes,
    }
}

/// The full layout, ignoring zoom. Focus and move directions use this so a
/// zoomed pane can still be left with the keyboard.
pub fn compute_without_zoom(tab: Option<&Tab>, viewport: Rect) -> Rects {
    let (tab_bar, status, workspace) = bars(viewport);
    let mut panes = Vec::new();
    if let Some(tab) = tab
        && let Some(layout) = &tab.layout
    {
        place(layout, workspace, &mut panes);
    }
    Rects {
        tab_bar,
        status,
        workspace,
        panes,
    }
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
    let (_, _, workspace) = bars(viewport);
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

/// The clickable tab rectangles in the tab bar, in the same order and widths
/// as the drawn tab bar. The space name's own header is not a tab.
pub fn tab_spans(space: &Space, area: Rect) -> Vec<(TabId, Rect)> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let end = area.x.saturating_add(area.width);
    let mut x = area.x.saturating_add(width(&format!(" {} ", space.name)));
    let mut spans = Vec::new();
    for (index, tab) in space.tabs.iter().enumerate() {
        let text = format!(" {} {} ", index + 1, tab.name);
        let span = width(&text);
        if x >= end {
            break;
        }
        let visible = span.min(end - x);
        spans.push((tab.id, Rect::new(x, area.y, visible, area.height)));
        x = x.saturating_add(span);
    }
    spans
}

/// The tab under a screen cell in the tab bar.
pub fn tab_at(space: &Space, area: Rect, x: u16, y: u16) -> Option<TabId> {
    tab_spans(space, area)
        .into_iter()
        .find(|(_, rect)| contains(*rect, x, y))
        .map(|(id, _)| id)
}

fn width(text: &str) -> u16 {
    u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
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
