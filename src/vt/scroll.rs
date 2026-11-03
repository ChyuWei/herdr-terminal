//! Local scrollback: viewport scrolling and the scrollbar, backed by the
//! Ghostty terminal's own history. No cells are duplicated here; the render
//! state already reports the viewport, so a snapshot after scrolling is the
//! historical screen.

use super::{Result, Vt, check};
use ghostty_sys as sys;
use std::ffi::c_void;

/// The terminal's scrollable area, in rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

impl Scrollbar {
    /// The viewport is on the live screen, not in history.
    pub fn at_bottom(self) -> bool {
        self.total == 0 || self.offset.saturating_add(self.len) >= self.total
    }

    /// Rows of history above the viewport.
    pub fn above(self) -> u64 {
        self.offset
    }
}

impl Vt {
    /// Scrolls the viewport by `delta` rows; negative is toward older output.
    pub fn scroll_lines(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let mut value = sys::GhosttyTerminalScrollViewportValue::default();
        value.delta = delta;
        self.scroll(sys::GHOSTTY_SCROLL_VIEWPORT_DELTA, value);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll(
            sys::GHOSTTY_SCROLL_VIEWPORT_TOP,
            sys::GhosttyTerminalScrollViewportValue::default(),
        );
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll(
            sys::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            sys::GhosttyTerminalScrollViewportValue::default(),
        );
    }

    fn scroll(
        &mut self,
        tag: sys::GhosttyTerminalScrollViewportTag,
        value: sys::GhosttyTerminalScrollViewportValue,
    ) {
        let behavior = sys::GhosttyTerminalScrollViewport { tag, value };
        // SAFETY: the terminal is live and the tagged union is initialized.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.terminal, behavior) };
    }

    /// The scrollbar state, or `None` when the terminal has no scroll area.
    pub fn scrollbar(&self) -> Option<Scrollbar> {
        let mut state = sys::GhosttyTerminalScrollbar::default();
        // SAFETY: the out pointer is valid for this data kind.
        let ok = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                &mut state as *mut _ as *mut c_void,
            )
        } == sys::GHOSTTY_SUCCESS;
        ok.then_some(Scrollbar {
            total: state.total,
            offset: state.offset,
            len: state.len,
        })
    }

    /// Whether the viewport is on the live bottom.
    pub fn at_bottom(&self) -> bool {
        self.scrollbar().is_none_or(Scrollbar::at_bottom)
    }

    /// Rows of history kept so far.
    pub fn scrollback_rows(&self) -> usize {
        let mut rows: usize = 0;
        // SAFETY: the out pointer is valid for this data kind.
        let ok = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                &mut rows as *mut _ as *mut c_void,
            )
        } == sys::GHOSTTY_SUCCESS;
        if ok { rows } else { 0 }
    }

    /// Changes the history cap at runtime. Lowering it prunes eligible
    /// history immediately. Ghostty prunes complete pages, so the retained
    /// rows can exceed the cap by roughly one page; the cap still bounds the
    /// history and memory.
    pub fn set_scrollback_limit(&mut self, lines: usize) -> Result<()> {
        // SAFETY: the input pointer is a valid size_t for this option.
        let result = unsafe {
            sys::ghostty_terminal_set(
                self.terminal,
                sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                &lines as *const usize as *const c_void,
            )
        };
        check(result, "set_scrollback_max_lines")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_lines(vt: &mut Vt, count: usize) {
        for index in 0..count {
            vt.feed(format!("L{index:03}\r\n").as_bytes());
        }
    }

    /// Ghostty prunes only complete pages: the documented overshoot is dozens
    /// to about a hundred rows, but a page's capacity grows with the terminal
    /// width, so this bound is deliberately generous. Staying under it still
    /// proves the cap is enforced rather than unbounded.
    const PAGE_SLACK: usize = 1_024;

    #[test]
    fn scrolling_moves_the_snapshot_into_history_and_back() {
        let mut vt = Vt::new_with_scrollback(10, 4, 100).expect("VT");
        feed_lines(&mut vt, 20);
        let bottom = vt.snapshot().expect("snapshot").lines().join("\n");
        assert!(bottom.contains("L019"), "{bottom:?}");
        assert!(vt.at_bottom());
        let bar = vt.scrollbar().expect("scrollbar");
        assert_eq!(bar.len, 4);
        assert!(bar.total >= 20, "{bar:?}");

        vt.scroll_lines(-2);
        assert!(!vt.at_bottom());
        let up = vt.snapshot().expect("snapshot").lines().join("\n");
        assert!(up.contains("L017"), "{up:?}");
        assert!(!up.contains("L019"), "{up:?}");

        vt.scroll_to_bottom();
        assert!(vt.at_bottom());
        let back = vt.snapshot().expect("snapshot").lines().join("\n");
        assert!(back.contains("L019"), "{back:?}");
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut vt = Vt::new_with_scrollback(10, 4, 100).expect("VT");
        feed_lines(&mut vt, 30);
        vt.scroll_lines(-10_000);
        let top = vt.snapshot().expect("snapshot").lines().join("\n");
        assert!(top.contains("L000"), "{top:?}");
        vt.scroll_lines(5_000);
        assert!(vt.at_bottom());
        vt.scroll_lines(5);
        assert!(vt.at_bottom());
    }

    #[test]
    fn the_scrollback_cap_bounds_history() {
        let mut vt = Vt::new_with_scrollback(80, 4, 5).expect("VT");
        feed_lines(&mut vt, 20_000);
        let rows = vt.scrollback_rows();
        assert!(
            rows <= 5 + PAGE_SLACK,
            "kept {rows} rows above the viewport for a 5-line cap"
        );
        let bar = vt.scrollbar().expect("scrollbar");
        assert!(bar.total <= (5 + PAGE_SLACK + 4) as u64, "{bar:?}");
        assert!(!vt.snapshot().expect("snapshot").lines()[0].contains("L000"));

        let mut unbounded = Vt::new_with_scrollback(80, 4, 100_000).expect("VT");
        feed_lines(&mut unbounded, 20_000);
        assert!(
            unbounded.scrollback_rows() > rows + 5_000,
            "the cap keeps far less than an uncapped terminal"
        );
    }

    #[test]
    fn zero_history_keeps_only_the_unprunable_page() {
        let mut vt = Vt::new_with_scrollback(80, 4, 0).expect("VT");
        feed_lines(&mut vt, 20_000);
        let rows = vt.scrollback_rows();
        assert!(
            rows <= PAGE_SLACK,
            "kept {rows} rows above the viewport with history disabled"
        );
        let bar = vt.scrollbar().expect("scrollbar");
        assert!(bar.total <= (PAGE_SLACK + 4) as u64, "{bar:?}");
    }

    #[test]
    fn lowering_the_cap_prunes_history() {
        let mut vt = Vt::new_with_scrollback(80, 4, 100).expect("VT");
        feed_lines(&mut vt, 20_000);
        assert!(vt.scrollback_rows() > 10);
        vt.set_scrollback_limit(2).expect("lower cap");
        let lowered = vt.scrollback_rows();
        assert!(
            lowered <= 2 + PAGE_SLACK,
            "kept {lowered} rows above the viewport for a 2-line cap"
        );
        feed_lines(&mut vt, 20_000);
        let rows = vt.scrollback_rows();
        assert!(
            rows <= 2 + PAGE_SLACK,
            "kept {rows} rows above the viewport for a 2-line cap after more output"
        );
    }
}
