//! Scrollback search through Ghostty's built-in searcher.
//!
//! The search owns a Ghostty search handle bound to this terminal. Its work is
//! split into bounded ticks so a large history never blocks the event loop:
//! each [`Vt::search_step`] makes a bounded amount of progress and feeds the
//! terminal at most once. Dropping the search (or replacing the VT) cancels it.

use super::Vt;
use ghostty_sys as sys;
use std::ffi::c_void;
use std::ptr;

/// The observable state of a running search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchProgress {
    /// Matches found on the active screen, newest first.
    pub total: usize,
    /// The selected match, 0-based from the newest one.
    pub selected: Option<usize>,
    /// The search has caught up with the terminal as of the last feed.
    pub complete: bool,
    /// The local search failed and was dropped.
    pub failed: bool,
}

/// A Ghostty search bound to one terminal.
pub(super) struct Search {
    handle: sys::GhosttySearch,
}

impl Search {
    pub(super) fn new(terminal: sys::GhosttyTerminal) -> Option<Search> {
        let mut handle = ptr::null_mut();
        // SAFETY: the terminal is live and the out pointer is valid.
        let result = unsafe { sys::ghostty_search_new(ptr::null(), &mut handle, terminal) };
        (result == sys::GHOSTTY_SUCCESS && !handle.is_null()).then_some(Search { handle })
    }

    /// Sets the literal needle. Matching is byte-exact except ASCII letters,
    /// which Ghostty compares case-insensitively.
    pub(super) fn set_needle(&mut self, needle: &str) -> bool {
        let string = sys::GhosttyString {
            ptr: needle.as_ptr(),
            len: needle.len(),
        };
        // SAFETY: the needle is copied by the search before this returns.
        let result = unsafe {
            sys::ghostty_search_set(
                self.handle,
                sys::GHOSTTY_SEARCH_OPT_NEEDLE,
                &string as *const _ as *const c_void,
            )
        };
        result == sys::GHOSTTY_SUCCESS
    }

    /// One bounded tick. Returns the status and whether the terminal is still
    /// usable.
    pub(super) fn tick(&mut self) -> (sys::GhosttySearchStatus, bool) {
        let mut status: sys::GhosttySearchStatus = sys::GHOSTTY_SEARCH_STATUS_COMPLETE;
        // SAFETY: the handle is live and the status pointer is valid.
        let result = unsafe { sys::ghostty_search_tick(self.handle, &mut status as *mut _) };
        (status, result == sys::GHOSTTY_SUCCESS)
    }

    pub(super) fn feed(&mut self) -> bool {
        // SAFETY: the handle is live and bound to the terminal.
        let result = unsafe { sys::ghostty_search_feed(self.handle) };
        result == sys::GHOSTTY_SUCCESS
    }

    pub(super) fn status(&self) -> sys::GhosttySearchStatus {
        let mut status: sys::GhosttySearchStatus = sys::GHOSTTY_SEARCH_STATUS_COMPLETE;
        // SAFETY: the handle is live and the out pointer is valid.
        unsafe {
            sys::ghostty_search_get(
                self.handle,
                sys::GHOSTTY_SEARCH_DATA_STATUS,
                &mut status as *mut _ as *mut c_void,
            )
        };
        status
    }

    pub(super) fn total(&self) -> usize {
        let mut total: usize = 0;
        // SAFETY: the handle is live and the out pointer is valid.
        let result = unsafe {
            sys::ghostty_search_get(
                self.handle,
                sys::GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
                &mut total as *mut _ as *mut c_void,
            )
        };
        if result == sys::GHOSTTY_SUCCESS {
            total
        } else {
            0
        }
    }

    pub(super) fn selected(&self) -> Option<usize> {
        let mut index: usize = 0;
        // SAFETY: the handle is live and the out pointer is valid.
        let result = unsafe {
            sys::ghostty_search_get(
                self.handle,
                sys::GHOSTTY_SEARCH_DATA_SELECTED_INDEX,
                &mut index as *mut _ as *mut c_void,
            )
        };
        (result == sys::GHOSTTY_SUCCESS).then_some(index)
    }

    /// Selects the next (older) or previous (newer) match, scrolling the
    /// viewport to it. False when there is no match.
    pub(super) fn select(&mut self, next: bool) -> bool {
        let option = if next {
            sys::GHOSTTY_SEARCH_OPT_SELECT_NEXT
        } else {
            sys::GHOSTTY_SEARCH_OPT_SELECT_PREV
        };
        // SAFETY: the select options take a null value by contract.
        let result = unsafe { sys::ghostty_search_set(self.handle, option, ptr::null()) };
        result == sys::GHOSTTY_SUCCESS
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        // SAFETY: the handle was created in `new` and is freed once. Ghostty
        // allows freeing the search before or after its terminal.
        unsafe { sys::ghostty_search_free(self.handle) };
    }
}

impl Vt {
    /// Sets the search needle, starting a fresh search when it changes. An
    /// empty needle closes the search.
    pub fn search_set(&mut self, needle: &str) -> SearchProgress {
        if needle.is_empty() {
            self.search_clear();
            return SearchProgress {
                complete: true,
                ..SearchProgress::default()
            };
        }
        if self.search.is_none() {
            self.search = Search::new(self.terminal);
        }
        let Some(search) = self.search.as_mut() else {
            return SearchProgress {
                failed: true,
                ..SearchProgress::default()
            };
        };
        if !search.set_needle(needle) {
            self.search = None;
            return SearchProgress {
                failed: true,
                ..SearchProgress::default()
            };
        }
        self.search_progress()
    }

    /// Makes bounded progress: at most `ticks` ticks and one feed. The loop
    /// keeps its event handling responsive while a large history is scanned.
    pub fn search_step(&mut self, ticks: usize) -> SearchProgress {
        let Some(search) = self.search.as_mut() else {
            return SearchProgress {
                complete: true,
                ..SearchProgress::default()
            };
        };
        let mut fed = false;
        let mut failed = false;
        for _ in 0..ticks.max(1) {
            let (status, ok) = search.tick();
            if !ok {
                failed = true;
                break;
            }
            match status {
                sys::GHOSTTY_SEARCH_STATUS_RUNNING => continue,
                sys::GHOSTTY_SEARCH_STATUS_FEED_REQUIRED => {
                    if fed {
                        break;
                    }
                    if !search.feed() {
                        failed = true;
                        break;
                    }
                    fed = true;
                }
                _ => break,
            }
        }
        if failed {
            self.search = None;
            return SearchProgress {
                failed: true,
                ..SearchProgress::default()
            };
        }
        self.search_progress()
    }

    /// Reads counts without making progress.
    pub fn search_progress(&self) -> SearchProgress {
        let Some(search) = self.search.as_ref() else {
            return SearchProgress {
                complete: true,
                ..SearchProgress::default()
            };
        };
        SearchProgress {
            total: search.total(),
            selected: search.selected(),
            complete: search.status() == sys::GHOSTTY_SEARCH_STATUS_COMPLETE,
            failed: false,
        }
    }

    /// Selects the next/older or previous/newer match.
    pub fn search_select(&mut self, next: bool) -> bool {
        self.search
            .as_mut()
            .is_some_and(|search| search.select(next))
    }

    /// Drops the search and its state. Cancellable at any point.
    pub fn search_clear(&mut self) {
        self.search = None;
    }

    pub fn search_active(&self) -> bool {
        self.search.is_some()
    }
}

/// Drives a search to completion. Tests use this; the runtime uses bounded
/// steps instead.
#[cfg(test)]
pub(crate) fn search_drive(vt: &mut Vt, limit: usize) -> SearchProgress {
    let mut progress = vt.search_progress();
    for _ in 0..limit {
        progress = vt.search_step(64);
        if progress.complete || progress.failed {
            break;
        }
    }
    progress
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt_with(text: &str) -> Vt {
        let mut vt = Vt::new_with_scrollback(40, 8, 10_000).expect("VT");
        vt.feed(text.as_bytes());
        vt
    }

    #[test]
    fn a_literal_search_is_case_insensitive_and_counts_matches() {
        let mut vt = vt_with("error one\r\nERROR two\r\nError three\r\nnothing here\r\n");
        vt.search_set("error");
        let progress = search_drive(&mut vt, 100);
        assert!(!progress.failed, "{progress:?}");
        assert!(progress.complete, "{progress:?}");
        assert_eq!(progress.total, 3, "{progress:?}");
        assert_eq!(progress.selected, None);
    }

    #[test]
    fn next_and_previous_walk_matches_newest_first_and_wrap() {
        let mut vt = vt_with("error one\r\nERROR two\r\nError three\r\n");
        vt.search_set("error");
        let progress = search_drive(&mut vt, 100);
        assert_eq!(progress.total, 3);
        assert!(vt.search_select(true));
        assert_eq!(vt.search_progress().selected, Some(0));
        assert!(vt.search_select(true));
        assert_eq!(vt.search_progress().selected, Some(1));
        assert!(vt.search_select(true));
        assert_eq!(vt.search_progress().selected, Some(2));
        assert!(vt.search_select(true), "next wraps");
        assert_eq!(vt.search_progress().selected, Some(0));
        assert!(vt.search_select(false), "previous wraps the other way");
        assert_eq!(vt.search_progress().selected, Some(2));
    }

    #[test]
    fn a_dot_is_literal_not_a_regular_expression() {
        let mut vt = vt_with("a.b\r\naxb\r\n");
        vt.search_set("a.b");
        let progress = search_drive(&mut vt, 100);
        assert_eq!(progress.total, 1, "{progress:?}");
    }

    #[test]
    fn a_search_reaches_into_scrollback_and_scrolls_to_a_match() {
        let mut vt = Vt::new_with_scrollback(20, 5, 10_000).expect("VT");
        for index in 0..80 {
            vt.feed(format!("filler {index:02}\r\n").as_bytes());
        }
        assert!(vt.at_bottom());
        vt.feed(b"needle alpha\r\n");
        for index in 0..40 {
            vt.feed(format!("later {index:02}\r\n").as_bytes());
        }
        assert!(vt.scrollback_rows() > 0);
        vt.search_set("NEEDLE alpha");
        let progress = search_drive(&mut vt, 500);
        assert!(progress.complete, "{progress:?}");
        assert_eq!(progress.total, 1, "{progress:?}");
        assert!(vt.search_select(true));
        assert!(!vt.at_bottom(), "the match is above the live bottom");
        let screen = vt.snapshot().expect("snapshot").lines().join("\n");
        assert!(screen.contains("needle alpha"), "{screen:?}");
    }

    #[test]
    fn changing_the_needle_restarts_and_an_empty_one_cancels() {
        let mut vt = vt_with("alpha\r\nbeta\r\n");
        vt.search_set("alpha");
        assert_eq!(search_drive(&mut vt, 100).total, 1);
        vt.search_set("beta");
        let progress = search_drive(&mut vt, 100);
        assert_eq!(progress.total, 1, "{progress:?}");
        vt.search_set("");
        assert!(!vt.search_active());
        assert!(vt.search_progress().complete);
        assert!(!vt.search_select(true));
    }
}
