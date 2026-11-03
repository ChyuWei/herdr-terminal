//! Local cell selection with tracked endpoints and plain-text extraction.
//!
//! Endpoints are Ghostty tracked grid references, so a selection follows its
//! cells while output scrolls and history is pruned. Formatting is Ghostty's
//! plain formatter with soft wraps unwrapped and trailing padding trimmed:
//! the copied text never contains the blank cells that pad a row.

use super::Vt;
use ghostty_sys as sys;
use std::ptr;

/// A selection's two tracked endpoints. Both are inclusive.
pub(super) struct Selection {
    anchor: sys::GhosttyTrackedGridRef,
    end: sys::GhosttyTrackedGridRef,
}

impl Selection {
    fn new(terminal: sys::GhosttyTerminal, x: u16, y: u16) -> Option<Selection> {
        let mut anchor = ptr::null_mut();
        // SAFETY: the terminal is live and the out pointer is valid.
        let result =
            unsafe { sys::ghostty_terminal_grid_ref_track(terminal, point(x, y), &mut anchor) };
        if result != sys::GHOSTTY_SUCCESS || anchor.is_null() {
            return None;
        }
        let mut end = ptr::null_mut();
        let result =
            unsafe { sys::ghostty_terminal_grid_ref_track(terminal, point(x, y), &mut end) };
        if result != sys::GHOSTTY_SUCCESS || end.is_null() {
            // SAFETY: the anchor was created above and is freed once here.
            unsafe { sys::ghostty_tracked_grid_ref_free(anchor) };
            return None;
        }
        Some(Selection { anchor, end })
    }

    fn extend(&mut self, terminal: sys::GhosttyTerminal, x: u16, y: u16) -> bool {
        // SAFETY: the terminal owns the reference and is live.
        let result = unsafe { sys::ghostty_tracked_grid_ref_set(self.end, terminal, point(x, y)) };
        result == sys::GHOSTTY_SUCCESS
    }

    fn end_point(&self) -> Option<(u16, u32)> {
        let mut coordinate = sys::GhosttyPointCoordinate::default();
        // SAFETY: the tracked handle is live and the out pointer is valid.
        let result = unsafe {
            sys::ghostty_tracked_grid_ref_point(
                self.end,
                sys::GHOSTTY_POINT_TAG_VIEWPORT,
                &mut coordinate,
            )
        };
        (result == sys::GHOSTTY_SUCCESS).then_some((coordinate.x, coordinate.y))
    }

    fn snapshot(&self) -> Option<sys::GhosttySelection> {
        let mut start = sys::GhosttyGridRef {
            size: std::mem::size_of::<sys::GhosttyGridRef>(),
            ..sys::GhosttyGridRef::default()
        };
        if unsafe { sys::ghostty_tracked_grid_ref_snapshot(self.anchor, &mut start) }
            != sys::GHOSTTY_SUCCESS
        {
            return None;
        }
        let mut end = sys::GhosttyGridRef {
            size: std::mem::size_of::<sys::GhosttyGridRef>(),
            ..sys::GhosttyGridRef::default()
        };
        if unsafe { sys::ghostty_tracked_grid_ref_snapshot(self.end, &mut end) }
            != sys::GHOSTTY_SUCCESS
        {
            return None;
        }
        Some(sys::GhosttySelection {
            size: std::mem::size_of::<sys::GhosttySelection>(),
            start,
            end,
            rectangle: false,
        })
    }

    fn text(&self, terminal: sys::GhosttyTerminal) -> Option<String> {
        let selection = self.snapshot()?;
        let options = sys::GhosttyTerminalSelectionFormatOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectionFormatOptions>(),
            emit: sys::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: true,
            trim: true,
            selection: &selection,
        };
        let mut written = 0usize;
        // SAFETY: the terminal is live; the null buffer asks for the size.
        let result = unsafe {
            sys::ghostty_terminal_selection_format_buf(
                terminal,
                options,
                ptr::null_mut(),
                0,
                &mut written,
            )
        };
        if result != sys::GHOSTTY_OUT_OF_SPACE || written == 0 {
            return None;
        }
        let mut buffer = vec![0u8; written];
        // SAFETY: the buffer is `written` bytes and the terminal is live.
        let result = unsafe {
            sys::ghostty_terminal_selection_format_buf(
                terminal,
                options,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        if result != sys::GHOSTTY_SUCCESS {
            return None;
        }
        buffer.truncate(written);
        if buffer.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&buffer).into_owned())
    }

    fn rects(&self, terminal: sys::GhosttyTerminal, cols: u16, rows: u16) -> Vec<(u16, u16, u16)> {
        let Some(selection) = self.snapshot() else {
            return Vec::new();
        };
        let Some((ax, ay)) = viewport_point(terminal, &selection.start) else {
            return Vec::new();
        };
        let Some((bx, by)) = viewport_point(terminal, &selection.end) else {
            return Vec::new();
        };
        let (start, end) = if (ay, ax) <= (by, bx) {
            ((ax, ay), (bx, by))
        } else {
            ((bx, by), (ax, ay))
        };
        if start.1 >= u32::from(rows) || end.1 >= u32::from(rows) {
            return Vec::new();
        }
        let mut rects = Vec::new();
        for y in start.1..=end.1 {
            let x0 = if y == start.1 { start.0 } else { 0 };
            let x1 = if y == end.1 {
                end.0
            } else {
                cols.saturating_sub(1)
            };
            if x1 >= x0 {
                rects.push((x0, y as u16, x1 - x0 + 1));
            }
        }
        rects
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        // SAFETY: both handles were created together and are freed once.
        unsafe {
            sys::ghostty_tracked_grid_ref_free(self.end);
            sys::ghostty_tracked_grid_ref_free(self.anchor);
        }
    }
}

fn point(x: u16, y: u16) -> sys::GhosttyPoint {
    sys::GhosttyPoint {
        tag: sys::GHOSTTY_POINT_TAG_VIEWPORT,
        value: sys::GhosttyPointValue {
            coordinate: sys::GhosttyPointCoordinate { x, y: u32::from(y) },
        },
    }
}

fn viewport_point(
    terminal: sys::GhosttyTerminal,
    reference: &sys::GhosttyGridRef,
) -> Option<(u16, u32)> {
    let mut coordinate = sys::GhosttyPointCoordinate::default();
    // SAFETY: the reference came from this terminal and the out pointer is valid.
    let result = unsafe {
        sys::ghostty_terminal_point_from_grid_ref(
            terminal,
            reference,
            sys::GHOSTTY_POINT_TAG_VIEWPORT,
            &mut coordinate,
        )
    };
    (result == sys::GHOSTTY_SUCCESS).then_some((coordinate.x, coordinate.y))
}

impl Vt {
    /// Begins a selection at a viewport cell. Replaces any previous one.
    pub fn start_selection(&mut self, x: u16, y: u16) -> bool {
        if x >= self.cols || y >= self.rows_count {
            return false;
        }
        let selection = Selection::new(self.terminal, x, y);
        let started = selection.is_some();
        self.selection = selection;
        started
    }

    /// Moves the selection's end to a viewport cell.
    pub fn extend_selection(&mut self, x: u16, y: u16) -> bool {
        if x >= self.cols || y >= self.rows_count {
            return false;
        }
        self.selection
            .as_mut()
            .is_some_and(|selection| selection.extend(self.terminal, x, y))
    }

    /// Extends the selection end by viewport cells, scrolling the history when
    /// the end moves past the top or bottom row. Returns whether it moved.
    pub fn move_selection(&mut self, dx: i16, dy: i16) -> bool {
        let Some((x, y)) = self.selection.as_ref().and_then(Selection::end_point) else {
            return false;
        };
        let mut nx = i32::from(x) + i32::from(dx);
        let mut ny = i64::from(y) + i64::from(dy);
        let rows = i64::from(self.rows_count.max(1));
        if ny < 0 {
            // Scrolling up moves the tracked cell down by the same amount.
            self.scroll_lines(ny as isize);
            let Some((_, moved)) = self.selection.as_ref().and_then(Selection::end_point) else {
                return false;
            };
            ny += i64::from(moved);
        } else if ny >= rows {
            let over = ny - (rows - 1);
            self.scroll_lines(over as isize);
            let Some((_, moved)) = self.selection.as_ref().and_then(Selection::end_point) else {
                return false;
            };
            ny = i64::from(moved) + over;
        }
        ny = ny.clamp(0, rows - 1);
        nx = nx.clamp(0, i32::from(self.cols.saturating_sub(1)));
        self.selection
            .as_mut()
            .is_some_and(|selection| selection.extend(self.terminal, nx as u16, ny as u16))
    }

    /// The selection text with wraps unwrapped and trailing padding trimmed.
    /// `None` when nothing is selected or the endpoints lost their cells.
    pub fn selection_text(&mut self) -> Option<String> {
        self.selection
            .as_ref()
            .and_then(|selection| selection.text(self.terminal))
    }

    /// Viewport rectangles to highlight: `(x, y, width)` per row.
    pub fn selection_rects(&mut self) -> Vec<(u16, u16, u16)> {
        let (cols, rows) = (self.cols, self.rows_count);
        self.selection
            .as_ref()
            .map(|selection| selection.rects(self.terminal, cols, rows))
            .unwrap_or_default()
    }

    pub fn selection_active(&self) -> bool {
        self.selection.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt_with(text: &str) -> Vt {
        let mut vt = Vt::new_with_scrollback(20, 5, 100).expect("VT");
        vt.feed(text.as_bytes());
        vt
    }

    fn copy(vt: &mut Vt, from: (u16, u16), to: (u16, u16)) -> Option<String> {
        assert!(vt.start_selection(from.0, from.1));
        assert!(vt.extend_selection(to.0, to.1));
        vt.selection_text()
    }

    #[test]
    fn a_single_line_selection_copies_without_padding() {
        let mut vt = vt_with("hello world\r\nsecond line\r\n");
        assert_eq!(copy(&mut vt, (0, 0), (4, 0)).as_deref(), Some("hello"));
        assert_eq!(
            copy(&mut vt, (0, 0), (19, 0)).as_deref(),
            Some("hello world"),
            "trailing blank cells are trimmed"
        );
    }

    #[test]
    fn a_multi_line_selection_keeps_the_line_breaks() {
        let mut vt = vt_with("hello world\r\nsecond line\r\nthird\r\n");
        assert_eq!(
            copy(&mut vt, (6, 0), (5, 1)).as_deref(),
            Some("world\nsecond")
        );
        assert_eq!(
            copy(&mut vt, (0, 1), (4, 2)).as_deref(),
            Some("second line\nthird")
        );
    }

    #[test]
    fn wide_cells_are_copied_once() {
        let mut vt = vt_with("日本語\r\n");
        assert_eq!(copy(&mut vt, (0, 0), (5, 0)).as_deref(), Some("日本語"));
    }

    #[test]
    fn an_empty_selection_has_no_text() {
        let mut vt = vt_with("hello\r\n\r\n");
        assert_eq!(copy(&mut vt, (10, 1), (15, 1)), None);
        vt.clear_selection();
        assert!(!vt.selection_active());
        assert_eq!(vt.selection_text(), None);
    }

    #[test]
    fn highlight_rects_cover_the_selected_rows() {
        let mut vt = vt_with("hello world\r\nsecond line\r\n");
        assert!(vt.start_selection(6, 0));
        assert!(vt.extend_selection(5, 1));
        let rects = vt.selection_rects();
        assert_eq!(rects.len(), 2, "{rects:?}");
        assert_eq!(rects[0], (6, 0, 14));
        assert_eq!(rects[1], (0, 1, 6));
        assert_eq!(rects[0].0, 6);
    }

    #[test]
    fn a_selection_follows_its_cells_while_output_scrolls() {
        let mut vt = Vt::new_with_scrollback(20, 4, 100).expect("VT");
        vt.feed(b"alpha\r\nbeta\r\n");
        assert!(vt.start_selection(0, 0));
        assert!(vt.extend_selection(4, 0));
        for index in 0..20 {
            vt.feed(format!("more {index}\r\n").as_bytes());
        }
        assert_eq!(vt.selection_text().as_deref(), Some("alpha"));
        assert!(
            vt.selection_rects().is_empty(),
            "the selected row is above the viewport now"
        );
    }

    #[test]
    fn keyboard_extension_moves_and_scrolls_at_the_edges() {
        let mut vt = Vt::new_with_scrollback(20, 4, 100).expect("VT");
        for index in 0..10 {
            vt.feed(format!("line {index:02}\r\n").as_bytes());
        }
        vt.feed(b"last line");
        let cursor = vt.snapshot().expect("snapshot").cursor;
        assert!(vt.start_selection(cursor.x, cursor.y));
        assert!(vt.move_selection(0, -3), "the end reaches the top row");
        assert!(vt.at_bottom(), "moving inside the viewport does not scroll");
        let text = vt.selection_text().expect("selection text");
        assert!(text.contains("line 08"), "{text:?}");
        assert!(vt.move_selection(0, -1), "past the top scrolls history");
        assert!(!vt.at_bottom());
    }
}
