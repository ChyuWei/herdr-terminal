//! The shared `herdr-terminal` document: schema, capture, restore, merge and
//! the orphan recovery that keeps a labeled terminal from being lost.

mod merge;
mod restore;
mod schema;

pub use merge::{Conflict, MergeResult, merge};
pub use restore::{TerminalInfo, pane_addresses, recover_orphans, restore};
pub use schema::{
    CLIENT_LABEL, CLIENT_NAME, DOC_NAME, DocChild, DocError, DocLayout, DocPane, DocSpace, DocTab,
    PINNED_LABEL, SEEN_LABEL, SharedDoc, identity_labels, is_attention_label, pane_ids,
};

use super::Workspace;

impl Workspace {
    /// The document value for this workspace. Focus, zoom, history and the
    /// active space/tab are not included.
    pub fn capture_doc(&self) -> SharedDoc {
        schema::capture(self)
    }
}
