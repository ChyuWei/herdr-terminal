//! Prefix key bindings.
//!
//! The defaults are the workspace and agent table plus the daily-use commands
//! (`:`, `f`, `[`, `y`). A small config map can move a command to another single
//! key; collisions are rejected with a warning so two commands are never
//! ambiguous.

use super::agents::Jump;
use super::commands::Command;
use super::model::{Axis, CloseKind, Direction};
use std::collections::BTreeMap;

/// The byte that opens the prefix, also sent literally by pressing it twice.
pub const PREFIX_BYTE: u8 = 0x02;

/// The control character that starts the prefix (Ctrl-b).
pub const PREFIX: char = 'b';

/// A validated prefix key map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMap {
    bindings: BTreeMap<char, Command>,
}

impl Default for KeyMap {
    fn default() -> KeyMap {
        let mut bindings = BTreeMap::new();
        for (key, command) in defaults() {
            bindings.insert(key, command);
        }
        KeyMap { bindings }
    }
}

impl KeyMap {
    /// The command for a prefix key. `1`..`9` stay reserved for tab selection.
    pub fn command(&self, key: char) -> Option<Command> {
        if let '1'..='9' = key {
            return Some(Command::SelectTab(key as u8 - b'0'));
        }
        self.bindings.get(&key).copied()
    }

    /// The key currently bound to a command, for palette hints and help.
    pub fn hint(&self, command: &Command) -> Option<char> {
        self.bindings
            .iter()
            .find(|(_, bound)| *bound == command)
            .map(|(key, _)| *key)
    }

    /// Applies `keys` from config. Returns the map and one warning per ignored
    /// binding. Invalid actions, invalid keys and collisions are ignored; the
    /// command keeps its previous key.
    pub fn from_config(overrides: &BTreeMap<String, String>) -> (KeyMap, Vec<String>) {
        let mut map = KeyMap::default();
        let mut warnings = Vec::new();
        for (action, binding) in overrides {
            let Some(command) = action_command(action) else {
                warnings.push(format!("unknown key action {action:?} ignored"));
                continue;
            };
            let mut chars = binding.chars();
            let Some(key) = chars.next() else {
                warnings.push(format!("key binding for {action:?} is empty"));
                continue;
            };
            if chars.next().is_some() {
                warnings.push(format!(
                    "key binding for {action:?} must be one character, not {binding:?}"
                ));
                continue;
            }
            if key.is_control() || key.is_ascii_digit() && key != '0' {
                warnings.push(format!(
                    "key {key:?} for {action:?} is reserved; the default is kept"
                ));
                continue;
            }
            if let Some(existing) = map.bindings.get(&key)
                && *existing != command
            {
                warnings.push(format!(
                    "key {key:?} for {action:?} is already bound to another command; ignored"
                ));
                continue;
            }
            map.bindings.retain(|_, bound| *bound != command);
            map.bindings.insert(key, command);
        }
        (map, warnings)
    }
}

/// The default command for a key, used by docs/tests and by `KeyMap::default`.
pub fn command(key: char) -> Option<Command> {
    KeyMap::default().command(key)
}

/// Every configurable action, with its default binding. Tab selection stays
/// on digits and is not part of this table.
pub fn defaults() -> Vec<(char, Command)> {
    vec![
        ('?', Command::Help),
        ('c', Command::NewTab),
        ('C', Command::NewSpace),
        ('n', Command::NextTab),
        ('p', Command::PrevTab),
        ('N', Command::NextSpace),
        ('P', Command::PrevSpace),
        ('v', Command::Split(Axis::Columns)),
        ('s', Command::Split(Axis::Rows)),
        ('h', Command::Focus(Direction::Left)),
        ('j', Command::Focus(Direction::Down)),
        ('k', Command::Focus(Direction::Up)),
        ('l', Command::Focus(Direction::Right)),
        ('H', Command::Move(Direction::Left)),
        ('J', Command::Move(Direction::Down)),
        ('K', Command::Move(Direction::Up)),
        ('L', Command::Move(Direction::Right)),
        ('z', Command::Zoom),
        ('x', Command::Close(CloseKind::Close)),
        ('e', Command::Close(CloseKind::Keep)),
        ('X', Command::Close(CloseKind::Kill)),
        (',', Command::RenameTab),
        ('.', Command::RenameSpace),
        ('a', Command::Agents),
        ('r', Command::Resize),
        ('t', Command::Terminals),
        ('m', Command::Machines),
        ('o', Command::Files),
        ('i', Command::Peek),
        ('g', Command::JumpAgent(Jump::Next)),
        ('G', Command::JumpAgent(Jump::Previous)),
        (':', Command::Palette),
        ('f', Command::Search),
        ('[', Command::Select),
        ('y', Command::Copy),
        ('q', Command::Detach),
    ]
}

/// The action name accepted in config's `keys` object.
pub fn action_command(action: &str) -> Option<Command> {
    Some(match action {
        "help" => Command::Help,
        "new_tab" => Command::NewTab,
        "new_space" => Command::NewSpace,
        "next_tab" => Command::NextTab,
        "prev_tab" => Command::PrevTab,
        "next_space" => Command::NextSpace,
        "prev_space" => Command::PrevSpace,
        "split_columns" => Command::Split(Axis::Columns),
        "split_rows" => Command::Split(Axis::Rows),
        "focus_left" => Command::Focus(Direction::Left),
        "focus_down" => Command::Focus(Direction::Down),
        "focus_up" => Command::Focus(Direction::Up),
        "focus_right" => Command::Focus(Direction::Right),
        "move_left" => Command::Move(Direction::Left),
        "move_down" => Command::Move(Direction::Down),
        "move_up" => Command::Move(Direction::Up),
        "move_right" => Command::Move(Direction::Right),
        "zoom" => Command::Zoom,
        "close" => Command::Close(CloseKind::Close),
        "keep" => Command::Close(CloseKind::Keep),
        "kill" => Command::Close(CloseKind::Kill),
        "rename_tab" => Command::RenameTab,
        "rename_space" => Command::RenameSpace,
        "agents" => Command::Agents,
        "resize" => Command::Resize,
        "terminals" => Command::Terminals,
        "machines" => Command::Machines,
        "files" => Command::Files,
        "peek" => Command::Peek,
        "jump_next" => Command::JumpAgent(Jump::Next),
        "jump_prev" => Command::JumpAgent(Jump::Previous),
        "palette" => Command::Palette,
        "search" => Command::Search,
        "select" => Command::Select,
        "copy" => Command::Copy,
        "detach" => Command::Detach,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_command_has_a_key() {
        for (key, expected) in defaults() {
            assert_eq!(command(key), Some(expected), "key {key:?}");
            assert_eq!(action_command(action_name(expected)), Some(expected));
        }
        for digit in '1'..='9' {
            assert_eq!(command(digit), Some(Command::SelectTab(digit as u8 - b'0')));
        }
    }

    fn action_name(command: Command) -> &'static str {
        [
            "help",
            "new_tab",
            "new_space",
            "next_tab",
            "prev_tab",
            "next_space",
            "prev_space",
            "split_columns",
            "split_rows",
            "focus_left",
            "focus_down",
            "focus_up",
            "focus_right",
            "move_left",
            "move_down",
            "move_up",
            "move_right",
            "zoom",
            "close",
            "keep",
            "kill",
            "rename_tab",
            "rename_space",
            "agents",
            "resize",
            "terminals",
            "machines",
            "files",
            "peek",
            "jump_next",
            "jump_prev",
            "palette",
            "search",
            "select",
            "copy",
            "detach",
        ]
        .into_iter()
        .find(|action| action_command(action) == Some(command))
        .expect("an action name")
    }

    #[test]
    fn unbound_keys_are_consumed_without_a_command() {
        for key in ['b', 'd', '0', ' ', '!', 'Z'] {
            assert_eq!(command(key), None, "key {key:?}");
        }
    }

    #[test]
    fn an_override_moves_a_command_and_frees_its_old_key() {
        let overrides = BTreeMap::from([("new_tab".to_owned(), "b".to_owned())]);
        let (keys, warnings) = KeyMap::from_config(&overrides);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(keys.command('b'), Some(Command::NewTab));
        assert_eq!(keys.command('c'), None);
        assert_eq!(keys.hint(&Command::NewTab), Some('b'));
    }

    #[test]
    fn collisions_and_reserved_keys_warn_and_keep_the_default() {
        let overrides = BTreeMap::from([
            ("new_tab".to_owned(), "v".to_owned()),
            ("help".to_owned(), "1".to_owned()),
            ("zoom".to_owned(), "zz".to_owned()),
            ("nope".to_owned(), "!".to_owned()),
        ]);
        let (keys, warnings) = KeyMap::from_config(&overrides);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(keys.command('c'), Some(Command::NewTab));
        assert_eq!(keys.command('v'), Some(Command::Split(Axis::Columns)));
        assert_eq!(keys.command('?'), Some(Command::Help));
        assert_eq!(keys.command('z'), Some(Command::Zoom));
    }

    #[test]
    fn the_new_daily_keys_are_bound_by_default() {
        assert_eq!(command(':'), Some(Command::Palette));
        assert_eq!(command('f'), Some(Command::Search));
        assert_eq!(command('['), Some(Command::Select));
        assert_eq!(command('y'), Some(Command::Copy));
    }
}
