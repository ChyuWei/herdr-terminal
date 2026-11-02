//! The per-screen local view file and the `HERDR_TERMINAL_SCREEN` id.
//!
//! Path: `$XDG_STATE_HOME/herdr-terminal/view-<screen>.json`, falling back to
//! `$HOME/.local/state`. Writes are atomic (temp file in the same directory,
//! then rename). A missing file is normal; a malformed one loads defaults with
//! a warning and never fails the workspace.

use super::model::{LocalView, Workspace};
use crate::core::{Error, Result};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub const SCREEN_ENV: &str = "HERDR_TERMINAL_SCREEN";
pub const DEFAULT_SCREEN: &str = "default";

/// Reads and validates the screen id, rejecting unsafe or empty values with a
/// usage error.
pub fn screen_id() -> Result<String> {
    let value = std::env::var(SCREEN_ENV).unwrap_or_else(|_| DEFAULT_SCREEN.to_owned());
    validate_screen(&value)?;
    Ok(value)
}

pub fn validate_screen(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "{SCREEN_ENV} must be 1-64 characters from [A-Za-z0-9._-], not {value:?}"
        )))
    }
}

/// The view file for a screen under a state directory.
pub fn view_path(state_home: &Path, screen: &str) -> PathBuf {
    state_home
        .join("herdr-terminal")
        .join(format!("view-{screen}.json"))
}

fn state_home() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("state"))
}

/// The loaded local view and its file.
pub struct ViewStore {
    path: Option<PathBuf>,
    saved: LocalView,
    warning: Option<String>,
    save_error: Option<String>,
}

impl ViewStore {
    pub fn load(screen: &str) -> ViewStore {
        let Some(home) = state_home() else {
            return ViewStore {
                path: None,
                saved: LocalView::default(),
                warning: Some(format!(
                    "no XDG_STATE_HOME or HOME: the local view for screen {screen:?} is not saved"
                )),
                save_error: None,
            };
        };
        let path = view_path(&home, screen);
        let (saved, warning) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => match LocalView::parse(&value) {
                    Ok(view) => (view, None),
                    Err(error) => (
                        LocalView::default(),
                        Some(format!(
                            "local view {} is malformed ({error}); using defaults",
                            path.display()
                        )),
                    ),
                },
                Err(error) => (
                    LocalView::default(),
                    Some(format!(
                        "local view {} is malformed ({error}); using defaults",
                        path.display()
                    )),
                ),
            },
            Err(error) if error.kind() == ErrorKind::NotFound => (LocalView::default(), None),
            Err(error) => (
                LocalView::default(),
                Some(format!(
                    "local view {} could not be read ({error}); using defaults",
                    path.display()
                )),
            ),
        };
        ViewStore {
            path: Some(path),
            saved,
            warning,
            save_error: None,
        }
    }

    /// The load warning, once.
    pub fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }

    pub fn apply(&self, workspace: &mut Workspace) {
        self.saved.apply(workspace);
    }

    /// Saves only when focus/zoom/selection changed; returns a new error to
    /// report, or nothing.
    pub fn save_if_changed(&mut self, workspace: &Workspace) -> Option<String> {
        let current = LocalView::capture(workspace);
        if current == self.saved {
            return None;
        }
        self.store(&current)
    }

    /// Saves unconditionally (detach), even when nothing changed.
    pub fn save_now(&mut self, workspace: &Workspace) -> Option<String> {
        let current = LocalView::capture(workspace);
        self.store(&current)
    }

    fn store(&mut self, view: &LocalView) -> Option<String> {
        let Some(path) = self.path.clone() else {
            return self.fail("no local view path");
        };
        match write_view(&path, view) {
            Ok(()) => {
                self.saved = view.clone();
                self.save_error = None;
                None
            }
            Err(error) => {
                let message = format!("could not save the local view {}: {error}", path.display());
                if self.save_error.as_deref() == Some(message.as_str()) {
                    None
                } else {
                    self.save_error = Some(message.clone());
                    Some(message)
                }
            }
        }
    }

    fn fail(&mut self, message: &str) -> Option<String> {
        if self.save_error.as_deref() == Some(message) {
            None
        } else {
            self.save_error = Some(message.to_owned());
            Some(message.to_owned())
        }
    }
}

fn write_view(path: &Path, view: &LocalView) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "view.json".into());
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let data = view.to_value().to_string();
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(data.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&temporary);
    // Windows cannot rename onto an existing file. Only retry there; on Unix
    // the first failure is real and must not destroy the old view.
    if cfg!(windows) {
        let _ = std::fs::remove_file(path);
        return std::fs::rename(&temporary, path);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_ids_are_restricted() {
        for valid in ["default", "one", "a.b_c-d", "x", &"a".repeat(64)] {
            assert!(validate_screen(valid).is_ok(), "{valid:?}");
        }
        for invalid in ["", "a/b", "a b", "..\\", "é", &"a".repeat(65)] {
            let error = validate_screen(invalid).unwrap_err();
            assert!(matches!(error, Error::Usage(_)), "{invalid:?}: {error}");
        }
    }

    #[test]
    fn two_screens_use_different_view_files() {
        let home = Path::new("/var/tmp/state");
        assert_ne!(
            view_path(home, "one"),
            view_path(home, "two"),
            "screen ids must not share a view"
        );
        assert!(view_path(home, "one").ends_with("herdr-terminal/view-one.json"));
    }
}
