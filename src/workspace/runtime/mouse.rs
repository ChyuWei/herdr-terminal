//! Mouse routing: chrome before panes, local actions before program reporting.
//!
//! Precedence, in order:
//! 1. A split-boundary drag resizes; a selection drag extends the selection.
//! 2. An open overlay other than a selection consumes every mouse event. A
//!    fresh press is never swallowed by selection mode: it cancels the
//!    selection and is then routed as if none existed.
//! 3. A click in the tab bar selects a tab; a click in the status bar is
//!    consumed; a click in a split gap starts a resize. Chrome never reaches a
//!    program.
//! 4. A press in a pane focuses it; a press on its border only focuses it. On
//!    the grid, a press with Shift or over a non-tracking program arms a local
//!    selection, and its release stays local even when no drag starts. The
//!    selection starts only when the pointer moves to another cell, so a press
//!    and release in one cell is a plain focus click (or, with Shift, a local
//!    click that sends nothing). If the program tracks the mouse and Shift is
//!    not held, the press, drag and release are forwarded with pane-local
//!    coordinates.
//! 5. The wheel is forwarded only to a program that tracks the mouse; a
//!    non-tracking program scrolls its local history instead. A wheel over
//!    chrome is consumed. While selecting, the wheel scrolls locally.
//!
//! Every forwarded event is converted from screen cells to the pane's inner
//! grid; the program never sees screen coordinates.

use super::{ResizeDrag, Screen, SelectionPress};
use crate::core::Result;
use crate::session;
use crate::workspace::geometry;
use crate::workspace::model::{Axis, PaneId};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

impl Screen<'_> {
    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        // The drawer's press owns the whole gesture. Its drag and release must
        // not reach the pane the drawer covered. Only a real release clears
        // this: a lost Up must not let the next Down's release through.
        if self.drawer_gesture {
            if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                self.drawer_gesture = false;
            }
            return Ok(());
        }
        // A drag that is already in progress owns the mouse. This comes before
        // the overlay rule because starting a selection makes `selecting`
        // some; its release must still end the drag.
        if self.resize_drag.is_some() {
            return self.drag_resize(mouse);
        }
        if let Some(pane) = self.selection_drag {
            return self.drag_selection(pane, mouse);
        }
        if self.ui.overlay() {
            return Ok(());
        }
        // On a narrow terminal the open rail is a modal drawer over the
        // panes: every event is consumed, and only a press on a row acts. Any
        // press starts a gesture the rest of which is swallowed; an outside
        // press also closes the drawer.
        if self.ui.sidebar.open {
            let rects = self.rects();
            if rects.rail.width == 0 {
                let drawer = self.rail_rect();
                let pressed = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
                if in_rect(drawer, mouse.column, mouse.row) {
                    if pressed {
                        self.sidebar_click(mouse.column, mouse.row);
                        // A row jump closes the drawer; its release must not
                        // reach the pane that is now uncovered.
                        self.drawer_gesture = true;
                    }
                } else if pressed {
                    self.close_sidebar();
                    self.drawer_gesture = true;
                }
                return Ok(());
            }
        }
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                // The wheel still works in selection mode: it scrolls that
                // pane locally, keeping the highlighted cells on their lines.
                self.selection_press = None;
                self.wheel(mouse)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection_press = None;
                // A fresh press leaves selection mode instead of being
                // consumed. The press then does what it would have done with
                // no selection: focus, tab, chrome or a tracking program.
                if self.ui.selecting.is_some() {
                    self.exit_selection(true);
                }
                self.press(mouse)
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_or_select(mouse),
            MouseEventKind::Up(MouseButton::Left) => {
                // A release with no movement was a focus click. When the press
                // was kept local (Shift or a non-tracking program), its release
                // stays local too, even if the drag never started.
                if self.selection_press.take().is_some() {
                    return Ok(());
                }
                self.forward_if_tracking(mouse)
            }
            _ => {
                self.selection_press = None;
                self.forward_if_tracking(mouse)
            }
        }
    }

    fn wheel(&mut self, mouse: MouseEvent) -> Result<()> {
        let (x, y) = (mouse.column, mouse.row);
        let rects = self.rects();
        let Some(pane) = rects.at(x, y) else {
            return Ok(());
        };
        // The pane border is chrome: a wheel there never reaches the program.
        let Some(inner) = rects.pane(pane).and_then(geometry::inner) else {
            return Ok(());
        };
        if !in_rect(inner, x, y) {
            return Ok(());
        }
        let tracking = self
            .links
            .get(&pane)
            .is_some_and(|link| link.vt.mouse_tracking());
        if tracking && self.ui.selecting.is_none() {
            return self.forward_to(pane, mouse);
        }
        let delta = match mouse.kind {
            MouseEventKind::ScrollUp => -3,
            _ => 3,
        };
        if let Some(link) = self.links.get_mut(&pane) {
            link.vt.scroll_lines(delta);
            self.dirty = true;
        }
        Ok(())
    }

    fn press(&mut self, mouse: MouseEvent) -> Result<()> {
        let (x, y) = (mouse.column, mouse.row);
        let rects = self.rects();
        // The rail on a wide terminal, or the summoned drawer on a narrow one.
        let wide = rects.rail.width > 0;
        if wide && in_rect(rects.rail, x, y) {
            self.sidebar_click(x, y);
            return Ok(());
        }
        if self.ui.sidebar.open {
            // A click outside the panel takes keyboard focus back.
            self.close_sidebar();
        }
        // The narrow pane-nav row maps every pane of the tab. The chips are
        // built by the same function the drawing uses, so a status word that
        // changes a chip's width can never move the mouse's target.
        if in_rect(rects.pane_nav, x, y) {
            if let Some(tab) = self.workspace.active_tab() {
                let entries = super::super::panenav::entries(&self.workspace, &self.agents.rows);
                let chips = super::super::panenav::layout(&entries, tab.focused, rects.pane_nav);
                if let Some(pane) = super::super::panenav::at(&chips, x, y)
                    && self.workspace.focus_pane(pane)
                {
                    self.dirty = true;
                }
            }
            return Ok(());
        }
        if in_rect(rects.status, x, y) {
            return Ok(());
        }
        // A split gap is only on screen in the wide, unzoomed layout; the
        // narrow view draws one pane and must not drag a boundary it hides.
        if wide && let Some(gap) = geometry::gap_at(self.workspace.active_tab(), self.area(), x, y)
        {
            let last = match gap.axis {
                Axis::Columns => x,
                Axis::Rows => y,
            };
            self.resize_drag = Some(ResizeDrag {
                axis: gap.axis,
                path: gap.path,
                index: gap.index,
                last,
                area: self.area(),
            });
            return Ok(());
        }
        let Some(pane) = rects.at(x, y) else {
            return Ok(());
        };
        if self.workspace.focus_pane(pane) {
            self.dirty = true;
        }
        let tracking = self
            .links
            .get(&pane)
            .is_some_and(|link| link.vt.mouse_tracking());
        let local = mouse.modifiers.contains(KeyModifiers::SHIFT) || !tracking;
        let Some(inner) = rects.pane(pane).and_then(geometry::inner) else {
            return Ok(());
        };
        if !local {
            // A program only ever sees events on its grid. The border is
            // chrome: a click there focuses the pane and stops.
            if !in_rect(inner, x, y) {
                return Ok(());
            }
            return self.forward_to(pane, mouse);
        }
        if !in_rect(inner, x, y) {
            // A border click focuses the pane and stops there.
            return Ok(());
        }
        // Do not start selecting yet: a press and release in one cell is only
        // a focus click. The selection starts on the first drag to another
        // cell, using this anchor.
        let (local_x, local_y) = local_cell(inner, x, y);
        self.selection_press = Some(SelectionPress {
            pane,
            origin: (x, y),
            anchor: (local_x, local_y),
        });
        Ok(())
    }

    /// A left drag with no selection yet either turns the pending press into a
    /// local selection or is forwarded to a tracking program. Dragging within
    /// the press cell stays a click.
    fn drag_or_select(&mut self, mouse: MouseEvent) -> Result<()> {
        let Some(press) = self.selection_press else {
            return self.forward_if_tracking(mouse);
        };
        if (mouse.column, mouse.row) == press.origin {
            return Ok(());
        }
        self.selection_press = None;
        let Some(inner) = self.rects().pane(press.pane).and_then(geometry::inner) else {
            return Ok(());
        };
        let (x, y) = local_cell(inner, mouse.column, mouse.row);
        let started = self
            .links
            .get_mut(&press.pane)
            .is_some_and(|link| link.vt.start_selection(press.anchor.0, press.anchor.1));
        if !started {
            return Ok(());
        }
        if let Some(link) = self.links.get_mut(&press.pane) {
            link.vt.extend_selection(x, y);
        }
        // COPY and resize are mutually exclusive: a mouse selection ends
        // resize first, so the strip always names the mode that consumes the
        // next key.
        self.clear_modes();
        self.selection_drag = Some(press.pane);
        self.ui.selecting = Some(press.pane);
        self.dirty = true;
        Ok(())
    }

    fn drag_resize(&mut self, mouse: MouseEvent) -> Result<()> {
        let Some(mut drag) = self.resize_drag.take() else {
            return Ok(());
        };
        let coordinate = match drag.axis {
            Axis::Columns => mouse.column,
            Axis::Rows => mouse.row,
        };
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                let delta = i32::from(coordinate) - i32::from(drag.last);
                if delta != 0 {
                    let delta = delta.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                    let changed = self
                        .workspace
                        .resize_gap(drag.area, &drag.path, drag.index, delta);
                    if changed {
                        drag.last = coordinate;
                        self.dirty = true;
                    }
                }
                self.resize_drag = Some(drag);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dirty = true;
            }
            _ => {
                self.resize_drag = Some(drag);
            }
        }
        Ok(())
    }

    fn drag_selection(&mut self, pane: PaneId, mouse: MouseEvent) -> Result<()> {
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(inner) = self.rects().pane(pane).and_then(geometry::inner) else {
                    return Ok(());
                };
                let (x, y) = local_cell(inner, mouse.column, mouse.row);
                if let Some(link) = self.links.get_mut(&pane)
                    && link.vt.extend_selection(x, y)
                {
                    self.dirty = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.selection_drag = None;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn forward_if_tracking(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.ui.selecting.is_some() {
            return Ok(());
        }
        let Some(pane) = self.rects().at(mouse.column, mouse.row) else {
            return Ok(());
        };
        let tracking = self
            .links
            .get(&pane)
            .is_some_and(|link| link.vt.mouse_tracking());
        if !tracking {
            return Ok(());
        }
        let Some(inner) = self.rects().pane(pane).and_then(geometry::inner) else {
            return Ok(());
        };
        if !in_rect(inner, mouse.column, mouse.row) {
            return Ok(());
        }
        self.forward_to(pane, mouse)
    }

    /// Sends a mouse event to one pane with pane-local coordinates.
    pub(super) fn forward_to(&mut self, pane: PaneId, mouse: MouseEvent) -> Result<()> {
        let Some(inner) = self.rects().pane(pane).and_then(geometry::inner) else {
            return Ok(());
        };
        let prepared = {
            let Some(link) = self.links.get_mut(&pane) else {
                return Ok(());
            };
            if link.observed {
                // An observer never types: mouse reports are input too.
                return Ok(());
            }
            if !link.vt.mouse_tracking() {
                return Ok(());
            }
            let (column, row) = local_cell(inner, mouse.column, mouse.row);
            let local = MouseEvent {
                kind: mouse.kind,
                column,
                row,
                modifiers: mouse.modifiers,
            };
            let bytes = link.vt.encode_mouse(&local);
            link.session.channel().map(|channel| (channel, bytes))
        };
        let Some((channel, bytes)) = prepared else {
            return Ok(());
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let client = self.client.clone();
        if let Err(error) = client.keys(channel, &bytes) {
            if session::is_lost(&error) {
                self.begin_reconnect();
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }
}

fn in_rect(rect: Rect, x: u16, y: u16) -> bool {
    let (x, y) = (u32::from(x), u32::from(y));
    x >= u32::from(rect.x)
        && x < u32::from(rect.x) + u32::from(rect.width)
        && y >= u32::from(rect.y)
        && y < u32::from(rect.y) + u32::from(rect.height)
}

/// Screen cell to pane-local cell, clamped onto the inner grid.
fn local_cell(inner: Rect, x: u16, y: u16) -> (u16, u16) {
    let max_x = inner.x.saturating_add(inner.width.saturating_sub(1));
    let max_y = inner.y.saturating_add(inner.height.saturating_sub(1));
    (
        x.clamp(inner.x, max_x).saturating_sub(inner.x),
        y.clamp(inner.y, max_y).saturating_sub(inner.y),
    )
}
