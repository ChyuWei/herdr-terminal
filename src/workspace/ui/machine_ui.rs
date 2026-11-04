//! The machine manager's geometry. The panel itself is drawn with the other
//! overlays in `ui/overlays.rs`; this module keeps the list height shared by
//! the drawing and scrolling code.

use super::manager_rect;
use ratatui::layout::Rect;

/// The machine manager uses the same wide panel as the terminal manager.
pub(super) fn machine_rect(area: Rect) -> Rect {
    manager_rect(area)
}

/// How many machine rows fit under the count line and the error/help line.
pub fn machine_list_height(area: Rect) -> usize {
    machine_rect(area)
        .height
        .saturating_sub(2)
        .saturating_sub(2) as usize
}
