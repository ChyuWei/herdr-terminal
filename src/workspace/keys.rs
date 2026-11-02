//! Key bindings after the prefix. Separate from command semantics so M6 can
//! make them configurable.
//!
//! The proposed M3 table binds both `k`/`K` for focus/move up and for
//! keep/kill. Focus and move keep the conventional `h/j/k/l` (and
//! `H/J/K/L`) family; keep and kill move to `e` ("eject", leave the program
//! running) and `X` (end the terminal). This is the only deviation from the
//! proposed table, and the help overlay lists the keys that actually work.

use super::commands::Command;
use super::model::{Axis, CloseKind, Direction};

/// The byte that opens the prefix, also sent literally by pressing it twice.
pub const PREFIX_BYTE: u8 = 0x02;

/// The control character that starts the prefix (Ctrl-b).
pub const PREFIX: char = 'b';

/// The command for a key pressed after the prefix. The character is the one
/// crossterm reports, so shifted letters and punctuation arrive already
/// shifted. `None` consumes the key without sending it to the program.
pub fn command(key: char) -> Option<Command> {
    match key {
        '?' => Some(Command::Help),
        'c' => Some(Command::NewTab),
        'C' => Some(Command::NewSpace),
        'n' => Some(Command::NextTab),
        'p' => Some(Command::PrevTab),
        'N' => Some(Command::NextSpace),
        'P' => Some(Command::PrevSpace),
        '1'..='9' => Some(Command::SelectTab(key as u8 - b'0')),
        'v' => Some(Command::Split(Axis::Columns)),
        's' => Some(Command::Split(Axis::Rows)),
        'h' => Some(Command::Focus(Direction::Left)),
        'j' => Some(Command::Focus(Direction::Down)),
        'k' => Some(Command::Focus(Direction::Up)),
        'l' => Some(Command::Focus(Direction::Right)),
        'H' => Some(Command::Move(Direction::Left)),
        'J' => Some(Command::Move(Direction::Down)),
        'K' => Some(Command::Move(Direction::Up)),
        'L' => Some(Command::Move(Direction::Right)),
        'z' => Some(Command::Zoom),
        'x' => Some(Command::Close(CloseKind::Close)),
        'e' => Some(Command::Close(CloseKind::Keep)),
        'X' => Some(Command::Close(CloseKind::Kill)),
        ',' => Some(Command::RenameTab),
        '.' => Some(Command::RenameSpace),
        'q' => Some(Command::Detach),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_command_has_a_key() {
        let bindings = [
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
            ('q', Command::Detach),
        ];
        for (key, expected) in bindings {
            assert_eq!(command(key), Some(expected), "key {key:?}");
        }
        for digit in '1'..='9' {
            assert_eq!(command(digit), Some(Command::SelectTab(digit as u8 - b'0')));
        }
    }

    #[test]
    fn unbound_keys_are_consumed_without_a_command() {
        for key in ['a', 'b', 'd', '0', ' ', '!', 'Z'] {
            assert_eq!(command(key), None, "key {key:?}");
        }
    }
}
