//! Resize mode: `prefix r`, then h/l width and j/k height. It reuses the
//! weighted split resize the mouse drag uses, so a keyboard resize and a
//! drag converge on the same layout weights.

use super::Screen;
use crate::core::Result;
use crate::workspace::geometry;
use crate::workspace::model::Axis;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

impl Screen<'_> {
    /// Enters resize mode when the active tab has something to resize.
    pub(super) fn enter_resize(&mut self) {
        let Some(tab) = self.workspace.active_tab() else {
            self.ui.message = Some("nothing to resize".into());
            self.dirty = true;
            return;
        };
        if tab.zoomed.is_some() {
            self.ui.message = Some("resize is unavailable while zoomed (leave zoom first)".into());
            self.dirty = true;
            return;
        }
        let panes = tab.panes().len();
        if panes == 0 {
            self.ui.message = Some("nothing to resize".into());
            self.dirty = true;
            return;
        }
        if panes == 1 {
            self.ui.message = Some("nothing to resize: this tab has one pane".into());
            self.dirty = true;
            return;
        }
        self.clear_modes();
        self.ui.prefix = false;
        self.ui.resize = true;
        self.ui.message = None;
        self.dirty = true;
    }

    /// One key in resize mode. Esc/q leave; the four directions move the
    /// nearest boundary (Shift for five cells); any other key leaves without
    /// acting and never reaches the program.
    pub(super) fn resize_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if !plain {
            self.leave_resize();
            return Ok(());
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let amount = if shift { 5 } else { 1 };
        let (axis, forward) = match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                self.leave_resize();
                return Ok(());
            }
            KeyCode::Char('h') => (Axis::Columns, false),
            KeyCode::Char('l') => (Axis::Columns, true),
            KeyCode::Char('k') => (Axis::Rows, false),
            KeyCode::Char('j') => (Axis::Rows, true),
            KeyCode::Char('H') => (Axis::Columns, false),
            KeyCode::Char('L') => (Axis::Columns, true),
            KeyCode::Char('K') => (Axis::Rows, false),
            KeyCode::Char('J') => (Axis::Rows, true),
            _ => {
                self.leave_resize();
                return Ok(());
            }
        };
        let amount = match key.code {
            KeyCode::Char('H' | 'L' | 'K' | 'J') => 5,
            _ => amount,
        };
        self.resize_step(axis, forward, amount);
        Ok(())
    }

    fn resize_step(&mut self, axis: Axis, forward: bool, amount: i16) {
        let area = self.area();
        let step = self.workspace.active_tab().and_then(|tab| {
            self.workspace
                .focused_pane()
                .and_then(|focused| geometry::resize_target(tab, focused, axis, forward))
        });
        let Some(step) = step else {
            return;
        };
        if self
            .workspace
            .resize_gap(area, &step.path, step.index, step.delta * amount)
        {
            self.dirty = true;
        }
    }

    fn leave_resize(&mut self) {
        if self.ui.resize {
            self.ui.resize = false;
            self.dirty = true;
        }
    }
}
