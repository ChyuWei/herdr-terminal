//! State for the shared-document CAS loop. Pure: the runtime performs the
//! `doc.get`/`doc.set` calls and this type remembers what it last read or
//! wrote, what still needs writing, and when to retry.
//!
//! The retry deadline throttles repeated stale refetches; it is not a
//! background fetch. A single failed `doc.get` can still hold the UI for up to
//! `DOC_TIMEOUT` because the call is synchronous, and moving fetches off the
//! UI thread is later work.

use super::model::SharedDoc;
use crate::core::Error;
use std::time::{Duration, Instant};

/// A document call is bounded so a slow core cannot freeze the UI for the
/// 10 s default.
pub const DOC_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait after a failed or conflicting document call.
pub const DOC_RETRY: Duration = Duration::from_secs(1);
/// The most CAS rounds one loop performs before leaving the rest for later.
pub const MAX_WRITE_ROUNDS: u32 = 3;

/// Whether a failed document call lost the CAS race.
pub fn is_conflict(error: &Error) -> bool {
    matches!(error, Error::Rpc { error, .. } if error.kind() == Some("conflict"))
}

#[derive(Clone, Debug)]
pub struct DocState {
    /// The document as last read or successfully written, with its version.
    pub base: SharedDoc,
    pub version: u64,
    /// A local shared mutation has not reached core yet.
    pub dirty: bool,
    /// A doc event, `missed` or a reconnect says the document may be newer.
    pub stale: bool,
    /// The document disappeared; write the local capture at version 0 even if
    /// it equals the base.
    pub force: bool,
    pub retry_at: Option<Instant>,
    pub last_written: Option<u64>,
}

impl DocState {
    pub fn new(base: SharedDoc, version: u64) -> DocState {
        DocState {
            base,
            version,
            dirty: false,
            stale: false,
            force: false,
            retry_at: None,
            last_written: Some(version),
        }
    }

    /// A doc event at `version`. Our own just-written version (and older
    /// versions) are ignored; a newer one asks for a refetch.
    pub fn note_event(&mut self, version: u64) {
        if Some(version) == self.last_written || version <= self.version {
            return;
        }
        self.stale = true;
    }

    pub fn written(&mut self, base: SharedDoc, version: u64) {
        self.base = base;
        self.version = version;
        self.dirty = false;
        self.stale = false;
        self.force = false;
        self.retry_at = None;
        self.last_written = Some(version);
    }

    pub fn retry_later(&mut self, now: Instant) {
        self.retry_at = Some(now + DOC_RETRY);
    }

    pub fn due(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|at| at <= now)
    }

    /// Whether a stale document should be fetched now. A failed fetch's
    /// deadline throttles repeated stale refetches; doc events keep `stale`
    /// set but do not clear the deadline.
    pub fn refresh_due(&self, now: Instant) -> bool {
        self.stale && self.due(now)
    }

    /// A `doc.get` succeeded, so any earlier retry deadline is satisfied. The
    /// caller clears this before a possible write, so a fresh write failure's
    /// deadline is not overwritten.
    pub fn fetched(&mut self) {
        self.retry_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str) -> SharedDoc {
        SharedDoc {
            spaces: vec![super::super::model::DocSpace {
                id: 1,
                name: name.into(),
                folder: String::new(),
                tabs: vec![super::super::model::DocTab {
                    id: 2,
                    name: "tab".into(),
                    layout: None,
                }],
            }],
        }
    }

    #[test]
    fn our_own_doc_event_is_not_a_refetch() {
        let mut state = DocState::new(doc("a"), 4);
        state.note_event(4);
        assert!(!state.stale);
        state.note_event(3);
        assert!(!state.stale);
        state.note_event(5);
        assert!(state.stale, "a newer version must be fetched");
        state.written(doc("b"), 5);
        state.note_event(5);
        assert!(!state.stale, "our just-written version is ignored");
    }

    #[test]
    fn retries_wait_for_their_deadline() {
        let now = Instant::now();
        let mut state = DocState::new(doc("a"), 1);
        assert!(state.due(now));
        state.retry_later(now);
        assert!(!state.due(now));
        assert!(state.due(now + DOC_RETRY));
    }

    #[test]
    fn a_stale_document_refreshes_only_after_its_deadline() {
        let now = Instant::now();
        let mut state = DocState::new(doc("a"), 1);
        assert!(!state.refresh_due(now), "nothing to fetch");
        state.note_event(2);
        assert!(state.refresh_due(now), "a stale document fetches");
        state.retry_later(now);
        assert!(!state.refresh_due(now), "the failed fetch cools down");
        assert!(state.refresh_due(now + DOC_RETRY));
    }

    #[test]
    fn a_successful_fetch_clears_the_retry_deadline() {
        let now = Instant::now();
        let mut state = DocState::new(doc("a"), 1);
        state.note_event(2);
        state.retry_later(now);
        assert!(!state.refresh_due(now));
        state.fetched();
        assert!(state.retry_at.is_none());
        assert!(state.refresh_due(now), "the next loop may fetch at once");
    }

    #[test]
    fn a_newer_doc_event_does_not_clear_the_retry_deadline() {
        let now = Instant::now();
        let mut state = DocState::new(doc("a"), 1);
        state.retry_later(now);
        state.note_event(2);
        assert!(state.stale, "the event still marks the document stale");
        assert!(
            !state.refresh_due(now),
            "but it must not bypass the cooldown"
        );
        assert!(state.refresh_due(now + DOC_RETRY));
    }
}
