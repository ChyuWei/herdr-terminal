//! Local settings: a small JSON file read at startup.
//!
//! The file is `$XDG_CONFIG_HOME/herdr-terminal/config.json`, falling back to
//! `$HOME/.config/herdr-terminal/config.json` when XDG_CONFIG_HOME is unset or
//! empty. It holds presentation preferences only; the workspace and layout stay
//! in core's document and the per-screen view. Every field is optional. A bad
//! field is reported and replaced by its default, so a broken config can never
//! stop the TUI from starting.

use crate::theme::ThemeChoice;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// The largest scrollback line limit accepted from config.
pub const MAX_SCROLLBACK: usize = 1_000_000;
/// The default bounded scrollback, in lines.
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// What clipboard path a copy uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardMode {
    /// OSC 52 to the outer terminal only.
    #[default]
    Osc52,
    /// A native helper, falling back to OSC 52 when it is unavailable.
    Native,
    /// Both OSC 52 and the native helper.
    Both,
    /// Copying is disabled.
    Off,
}

impl ClipboardMode {
    pub fn parse(value: &str) -> Option<ClipboardMode> {
        match value {
            "osc52" => Some(ClipboardMode::Osc52),
            "native" => Some(ClipboardMode::Native),
            "both" => Some(ClipboardMode::Both),
            "off" => Some(ClipboardMode::Off),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ClipboardMode::Osc52 => "osc52",
            ClipboardMode::Native => "native",
            ClipboardMode::Both => "both",
            ClipboardMode::Off => "off",
        }
    }
}

/// The effective local settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub theme: ThemeChoice,
    /// Lines of local scrollback requested per pane. Ghostty keeps at least
    /// its unpruuable current page, so 0 still retains a bounded few hundred
    /// rows rather than none.
    pub scrollback: usize,
    /// Whether the workspace asks the outer terminal for mouse events.
    pub mouse: bool,
    pub clipboard: ClipboardMode,
    /// Raw prefix bindings from config; validated by the key map.
    pub keys: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: ThemeChoice::Default,
            scrollback: DEFAULT_SCROLLBACK,
            mouse: true,
            clipboard: ClipboardMode::Osc52,
            keys: BTreeMap::new(),
        }
    }
}

/// A config and everything that had to be ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

impl Loaded {
    /// A one-line notice for the status bar, or nothing.
    pub fn notice(&self) -> Option<String> {
        (!self.warnings.is_empty()).then(|| self.warnings.join("; "))
    }
}

/// The default config path for this user.
pub fn path() -> Option<PathBuf> {
    path_in(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Chooses the config path from the two environment values. An empty value
/// means unset: `XDG_CONFIG_HOME=""` falls back to `$HOME/.config`, as the
/// XDG base-directory specification requires. The pure form keeps the choice
/// testable without changing the process environment.
fn path_in(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(base) = xdg.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(base).join("herdr-terminal/config.json"));
    }
    home.filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config/herdr-terminal/config.json"))
}

/// Loads the user's config. A missing file is the normal default; a broken one
/// still yields defaults and warnings.
pub fn load() -> Loaded {
    match path() {
        Some(path) => load_from(&path),
        None => Loaded::default(),
    }
}

pub fn load_from(path: &Path) -> Loaded {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded::default();
        }
        Err(error) => {
            return Loaded {
                config: Config::default(),
                warnings: vec![format!("could not read {}: {error}", path.display())],
            };
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return Loaded {
                config: Config::default(),
                warnings: vec![format!("{} is not valid JSON: {error}", path.display())],
            };
        }
    };
    parse(&value)
}

/// Parses a config value, collecting a warning per ignored field.
pub fn parse(value: &Value) -> Loaded {
    let mut loaded = Loaded::default();
    let Some(object) = value.as_object() else {
        loaded
            .warnings
            .push("config is not a JSON object; using defaults".into());
        return loaded;
    };
    for (key, value) in object {
        match key.as_str() {
            "theme" => match value.as_str().and_then(ThemeChoice::parse) {
                Some(theme) => loaded.config.theme = theme,
                None => loaded.warnings.push(format!(
                    "theme must be \"default\", \"dark\", \"light\", \"catppuccin\", \
                     \"catppuccin-latte\" or \"terminal\", not {value}"
                )),
            },
            "scrollback" => match value.as_u64() {
                Some(lines) if lines <= MAX_SCROLLBACK as u64 => {
                    loaded.config.scrollback = lines as usize;
                }
                Some(lines) => loaded.warnings.push(format!(
                    "scrollback {lines} is above the {MAX_SCROLLBACK} line maximum; using {DEFAULT_SCROLLBACK}"
                )),
                None => loaded
                    .warnings
                    .push(format!("scrollback must be a whole number of lines, not {value}")),
            },
            "mouse" => match value.as_bool() {
                Some(on) => loaded.config.mouse = on,
                None => loaded
                    .warnings
                    .push(format!("mouse must be true or false, not {value}")),
            },
            "clipboard" => match value.as_str().and_then(ClipboardMode::parse) {
                Some(mode) => loaded.config.clipboard = mode,
                None => loaded.warnings.push(format!(
                    "clipboard must be \"osc52\", \"native\", \"both\" or \"off\", not {value}"
                )),
            },
            "keys" => match value.as_object() {
                Some(keys) => {
                    for (action, binding) in keys {
                        match binding.as_str() {
                            Some(binding) if !binding.is_empty() => {
                                loaded
                                    .config
                                    .keys
                                    .insert(action.clone(), binding.to_owned());
                            }
                            _ => loaded.warnings.push(format!(
                                "key binding for {action:?} must be a non-empty string"
                            )),
                        }
                    }
                }
                None => loaded
                    .warnings
                    .push(format!("keys must be an object of action to key, not {value}")),
            },
            other => loaded
                .warnings
                .push(format!("unknown setting {other:?} ignored")),
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_are_a_usable_local_setup() {
        let config = Config::default();
        assert_eq!(config.theme, ThemeChoice::Default);
        assert_eq!(config.scrollback, 10_000);
        assert!(config.mouse);
        assert_eq!(config.clipboard, ClipboardMode::Osc52);
        assert!(config.keys.is_empty());
        assert_eq!(Loaded::default().notice(), None);
    }

    #[test]
    fn every_supported_field_is_parsed() {
        let loaded = parse(&json!({
            "theme": "dark",
            "scrollback": 500,
            "mouse": false,
            "clipboard": "both",
            "keys": {"new_tab": "b", "palette": ":"},
        }));
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let config = loaded.config;
        assert_eq!(config.theme, ThemeChoice::Dark);
        assert_eq!(config.scrollback, 500);
        assert!(!config.mouse);
        assert_eq!(config.clipboard, ClipboardMode::Both);
        assert_eq!(config.keys["new_tab"], "b");
        assert_eq!(config.keys["palette"], ":");
    }

    #[test]
    fn bad_values_warn_and_keep_their_default() {
        let loaded = parse(&json!({
            "theme": "neon",
            "scrollback": -1,
            "mouse": "yes",
            "clipboard": "clipboard",
            "keys": ["new_tab"],
            "extra": true,
        }));
        let notice = loaded.notice();
        let config = loaded.config;
        assert_eq!(config.theme, ThemeChoice::Default);
        assert_eq!(config.scrollback, 10_000);
        assert!(config.mouse);
        assert_eq!(config.clipboard, ClipboardMode::Osc52);
        assert!(config.keys.is_empty());
        assert_eq!(loaded.warnings.len(), 6, "{:?}", loaded.warnings);
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("unknown setting"))
        );
        assert!(notice.expect("notice").contains("neon"));
    }

    #[test]
    fn a_scrollback_above_the_cap_falls_back_instead_of_unbounding_memory() {
        let loaded = parse(&json!({"scrollback": MAX_SCROLLBACK + 1}));
        assert_eq!(loaded.config.scrollback, DEFAULT_SCROLLBACK);
        assert!(loaded.warnings[0].contains("maximum"));
    }

    #[test]
    fn zero_disables_scrollback_and_is_not_an_error() {
        let loaded = parse(&json!({"scrollback": 0}));
        assert_eq!(loaded.config.scrollback, 0);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn a_missing_file_is_the_default_config() {
        let dir = scratch();
        let loaded = load_from(&dir.join("missing.json"));
        assert_eq!(loaded, Loaded::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_malformed_file_warns_but_never_fails() {
        let dir = scratch();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, b"{not json").unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.warnings[0].contains("not valid JSON"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_xdg_config_home_falls_back_to_home() {
        let home = OsString::from("/home/person");
        assert_eq!(
            path_in(Some(OsString::new()), Some(home.clone())),
            Some(PathBuf::from(
                "/home/person/.config/herdr-terminal/config.json"
            ))
        );
        assert_eq!(
            path_in(Some(OsString::from("/xdg")), Some(home)),
            Some(PathBuf::from("/xdg/herdr-terminal/config.json"))
        );
        assert_eq!(path_in(None, Some(OsString::new())), None);
        assert_eq!(path_in(None, None), None);
    }

    #[test]
    fn an_empty_xdg_config_home_loads_the_home_config() {
        let root = scratch();
        let dir = root.join(".config/herdr-terminal");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), br#"{"mouse":false}"#).unwrap();
        let path = path_in(Some(OsString::new()), Some(root.clone().into_os_string()))
            .expect("the home fallback path");
        let loaded = load_from(&path);
        assert!(!loaded.config.mouse);
        assert!(loaded.warnings.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    fn scratch() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        PathBuf::from(format!(
            "/var/tmp/ht-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
