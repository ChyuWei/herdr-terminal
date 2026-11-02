//! Rediscovering the screen's terminal after a client restart.
//!
//! M2 has one screen, so a small local pointer is enough: the core identity
//! and the terminal address, under `XDG_STATE_HOME/herdr-terminal/`. A
//! pointer that matches the current core's snapshot wins; otherwise one live
//! terminal carrying `herdr.client=herdr-terminal` is adopted. Several such
//! terminals are ambiguous and are reported rather than guessed, so a client
//! restart can never spawn a duplicate of a program that is still running.
//!
//! The workspace document replaces this in M4; this file is deliberately not
//! a document and is not shared between machines or screens.

use crate::core::Terminal;
use crate::model::Model;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The label every HerdR Terminal terminal carries.
pub const CLIENT_LABEL: &str = "herdr.client";
pub const CLIENT_NAME: &str = "herdr-terminal";

/// What a previous client left for the next one: which core, which terminal.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Pointer {
    /// The core's stable id (`hello.core.id`); empty when it did not say.
    #[serde(default)]
    pub core: String,
    pub terminal: String,
}

impl Pointer {
    /// The default pointer path for this user.
    pub fn path() -> Option<PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })?;
        Some(state.join(CLIENT_NAME).join("screen.json"))
    }

    /// The pointer for this core and terminal, if one was written.
    pub fn load() -> Option<Pointer> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Option<Pointer> {
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Records this terminal for the next client. A failed write only costs
    /// the pointer path; the labels still identify the terminal.
    pub fn save(&self) {
        if let Some(path) = Self::path() {
            let _ = self.save_to(&path);
        }
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        // Write beside the target, then move it over, so a crash cannot leave
        // half a pointer behind.
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        let _ = fs::remove_file(path);
        fs::rename(&temporary, path)
    }

    /// Removes the pointer if it still names this core and terminal.
    pub fn clear(&self) {
        if let Some(path) = Self::path() {
            Self::clear_at(&path, self);
        }
    }

    pub fn clear_at(path: &Path, expected: &Pointer) {
        if Self::load_from(path).as_ref() == Some(expected) {
            let _ = fs::remove_file(path);
        }
    }
}

/// What a fresh client should do with the terminals core reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// No terminal to adopt: start one.
    New,
    /// Reattach to this terminal.
    Existing(String),
    /// Several live terminals carry this client's label; do not guess.
    Ambiguous(Vec<String>),
}

/// Chooses the terminal a fresh client should reattach to.
pub fn choose(model: &Model, pointer: Option<&Pointer>, core_id: &str) -> Choice {
    if let Some(pointer) = pointer
        && core_matches(&pointer.core, core_id)
        && model.terminal(&pointer.terminal).is_some()
    {
        return Choice::Existing(pointer.terminal.clone());
    }
    let mut candidates: Vec<String> = model
        .terminals()
        .filter(|terminal| terminal.is_live())
        .filter(|terminal| is_ours(terminal))
        .map(|terminal| terminal.address.clone())
        .collect();
    candidates.sort();
    match candidates.len() {
        0 => Choice::New,
        1 => Choice::Existing(candidates.remove(0)),
        _ => Choice::Ambiguous(candidates),
    }
}

fn core_matches(pointer: &str, core_id: &str) -> bool {
    pointer.is_empty() || core_id.is_empty() || pointer == core_id
}

fn is_ours(terminal: &Terminal) -> bool {
    terminal.labels.get(CLIENT_LABEL).map(String::as_str) == Some(CLIENT_NAME)
}

/// The pointer for this core and address, ready to save or clear.
pub fn pointer(core_id: &str, address: &str) -> Pointer {
    Pointer {
        core: core_id.to_owned(),
        terminal: address.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Snapshot;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn terminal(address: &str, state: &str, labelled: bool) -> Terminal {
        let mut labels = std::collections::BTreeMap::new();
        if labelled {
            labels.insert(CLIENT_LABEL.into(), CLIENT_NAME.into());
        }
        Terminal {
            address: address.into(),
            state: state.into(),
            labels,
            ..Terminal::default()
        }
    }

    fn model_of(terminals: Vec<Terminal>) -> Model {
        Model::from_snapshot(&Snapshot {
            seq: 1,
            terminals,
            channel: Some(3),
        })
    }

    fn scratch() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        PathBuf::from(format!(
            "/var/tmp/ht-pointer-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn a_matching_pointer_wins_even_when_other_terminals_exist() {
        let model = model_of(vec![
            terminal("t1", "live", true),
            terminal("t2", "live", true),
        ]);
        assert_eq!(
            choose(&model, Some(&pointer("core-a", "t2")), "core-a"),
            Choice::Existing("t2".into())
        );
        // Another core's pointer is ignored: the labels decide.
        let model = model_of(vec![terminal("t1", "live", true)]);
        assert_eq!(
            choose(&model, Some(&pointer("core-b", "t9")), "core-a"),
            Choice::Existing("t1".into())
        );
    }

    #[test]
    fn one_labelled_terminal_is_adopted_and_none_means_start_one() {
        let model = model_of(vec![
            terminal("t1", "live", true),
            terminal("t2", "finished", true),
            terminal("t3", "live", false),
        ]);
        assert_eq!(choose(&model, None, "core"), Choice::Existing("t1".into()));

        let model = model_of(vec![terminal("t3", "live", false)]);
        assert_eq!(choose(&model, None, "core"), Choice::New);
    }

    #[test]
    fn several_live_candidates_are_ambiguous() {
        let model = model_of(vec![
            terminal("t2", "live", true),
            terminal("t1", "live", true),
            terminal("t3", "finished", true),
        ]);
        assert_eq!(
            choose(&model, None, "core"),
            Choice::Ambiguous(vec!["t1".into(), "t2".into()])
        );
    }

    #[test]
    fn a_pointer_round_trips_and_only_clears_its_own_terminal() {
        let path = scratch().join("screen.json");
        let saved = pointer("core", "t1");
        saved.save_to(&path).expect("save");
        assert_eq!(Pointer::load_from(&path), Some(saved.clone()));

        Pointer::clear_at(&path, &pointer("core", "t2"));
        assert_eq!(
            Pointer::load_from(&path),
            Some(saved.clone()),
            "a pointer for another terminal must not clear this one"
        );
        Pointer::clear_at(&path, &saved);
        assert_eq!(Pointer::load_from(&path), None);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_broken_pointer_is_ignored() {
        let path = scratch().join("screen.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json").unwrap();
        assert_eq!(Pointer::load_from(&path), None);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_pointer_is_json_with_a_core_and_a_terminal() {
        let value = serde_json::to_value(pointer("core", "t7")).unwrap();
        assert_eq!(value, serde_json::json!({"core": "core", "terminal": "t7"}));
        let parsed: Pointer =
            serde_json::from_value(serde_json::json!({"terminal": "t7"})).unwrap();
        assert_eq!(parsed, pointer("", "t7"));
    }
}
