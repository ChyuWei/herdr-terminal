//! The command palette: the existing command table, a filter and a selection.
//!
//! No command semantics live here. The palette only names commands that
//! [`super::commands::Workspace::dispatch`] already implements, with their
//! current prefix key hint from the configured [`KeyMap`]. Filtering is a
//! case-insensitive substring match over the label and the action name.

use super::agents::Jump;
use super::commands::Command;
use super::keys::KeyMap;
use super::model::{Axis, CloseKind, Direction};

/// One palette row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub command: Command,
    pub label: &'static str,
    /// The current prefix key, or `None` for a command with no binding.
    pub hint: Option<char>,
}

/// Every palette command in display order.
pub fn entries(keys: &KeyMap) -> Vec<Entry> {
    table()
        .into_iter()
        .map(|(command, label)| Entry {
            command,
            label,
            hint: keys.hint(&command),
        })
        .collect()
}

fn table() -> Vec<(Command, &'static str)> {
    vec![
        (Command::Help, "help"),
        (Command::NewTab, "new tab"),
        (Command::NewSpace, "new space"),
        (Command::NextTab, "next tab"),
        (Command::PrevTab, "previous tab"),
        (Command::NextSpace, "next space"),
        (Command::PrevSpace, "previous space"),
        (Command::Split(Axis::Columns), "split left/right"),
        (Command::Split(Axis::Rows), "split top/bottom"),
        (Command::Focus(Direction::Left), "focus left"),
        (Command::Focus(Direction::Down), "focus down"),
        (Command::Focus(Direction::Up), "focus up"),
        (Command::Focus(Direction::Right), "focus right"),
        (Command::Move(Direction::Left), "move pane left"),
        (Command::Move(Direction::Down), "move pane down"),
        (Command::Move(Direction::Up), "move pane up"),
        (Command::Move(Direction::Right), "move pane right"),
        (Command::Zoom, "toggle zoom"),
        (Command::Close(CloseKind::Close), "close pane"),
        (Command::Close(CloseKind::Keep), "keep program and close"),
        (Command::Close(CloseKind::Kill), "kill program and close"),
        (Command::RenameTab, "rename tab"),
        (Command::RenameSpace, "rename space"),
        (Command::Agents, "agent sidebar"),
        (Command::Terminals, "terminal manager"),
        (Command::Machines, "machine manager"),
        (Command::Peek, "peek agent"),
        (Command::JumpAgent(Jump::Next), "jump to next agent"),
        (Command::JumpAgent(Jump::Previous), "jump to previous agent"),
        (Command::Palette, "command palette"),
        (Command::Search, "search scrollback"),
        (Command::Select, "selection mode"),
        (Command::Copy, "copy selection"),
        (Command::Detach, "detach client"),
    ]
}

/// The entries matching `query`. Empty query matches all; matching is a
/// case-insensitive substring over label and action name.
pub fn filter(entries: &[Entry], query: &str) -> Vec<Entry> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .copied()
        .filter(|entry| {
            entry.label.to_ascii_lowercase().contains(&query)
                || action_name(entry.command)
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .collect()
}

/// The current selection clamped into the filtered rows, if any.
pub fn selected(rows: &[Entry], selected: usize) -> Option<Entry> {
    rows.get(selected.min(rows.len().saturating_sub(1)))
        .copied()
}

/// A stable action name, used for matching and as the palette's subtitle.
pub fn action_name(command: Command) -> &'static str {
    match command {
        Command::Help => "help",
        Command::NewTab => "new_tab",
        Command::NewSpace => "new_space",
        Command::NextTab => "next_tab",
        Command::PrevTab => "prev_tab",
        Command::NextSpace => "next_space",
        Command::PrevSpace => "prev_space",
        Command::SelectTab(_) => "select_tab",
        Command::Split(Axis::Columns) => "split_columns",
        Command::Split(Axis::Rows) => "split_rows",
        Command::Focus(Direction::Left) => "focus_left",
        Command::Focus(Direction::Down) => "focus_down",
        Command::Focus(Direction::Up) => "focus_up",
        Command::Focus(Direction::Right) => "focus_right",
        Command::Move(Direction::Left) => "move_left",
        Command::Move(Direction::Down) => "move_down",
        Command::Move(Direction::Up) => "move_up",
        Command::Move(Direction::Right) => "move_right",
        Command::Zoom => "zoom",
        Command::Close(CloseKind::Close) => "close",
        Command::Close(CloseKind::Keep) => "keep",
        Command::Close(CloseKind::Kill) => "kill",
        Command::RenameTab => "rename_tab",
        Command::RenameSpace => "rename_space",
        Command::Agents => "agents",
        Command::Terminals => "terminals",
        Command::Machines => "machines",
        Command::Peek => "peek",
        Command::JumpAgent(Jump::Next) => "jump_next",
        Command::JumpAgent(Jump::Previous) => "jump_prev",
        Command::Palette => "palette",
        Command::Search => "search",
        Command::Select => "select",
        Command::Copy => "copy",
        Command::Detach => "detach",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_entries() -> Vec<Entry> {
        entries(&KeyMap::default())
    }

    #[test]
    fn the_palette_names_the_existing_commands_with_their_keys() {
        let rows = default_entries();
        assert!(rows.len() >= 30, "{rows:?}");
        let new_tab = rows
            .iter()
            .find(|entry| entry.command == Command::NewTab)
            .expect("new tab");
        assert_eq!(new_tab.label, "new tab");
        assert_eq!(new_tab.hint, Some('c'));
        let palette = rows
            .iter()
            .find(|entry| entry.command == Command::Palette)
            .expect("palette");
        assert_eq!(palette.hint, Some(':'));
        for row in &rows {
            assert!(!row.label.is_empty());
            assert!(!action_name(row.command).is_empty());
        }
    }

    #[test]
    fn an_empty_query_lists_everything_and_filtering_is_case_insensitive() {
        let rows = default_entries();
        assert_eq!(filter(&rows, "").len(), rows.len());
        assert_eq!(filter(&rows, "   ").len(), rows.len());
        let split = filter(&rows, "SPLIT");
        assert_eq!(split.len(), 2, "{split:?}");
        assert!(
            split
                .iter()
                .all(|entry| matches!(entry.command, Command::Split(_)))
        );
        let name = filter(&rows, "prev_tab");
        assert_eq!(name.len(), 1);
        assert_eq!(name[0].command, Command::PrevTab);
        assert!(filter(&rows, "no such command").is_empty());
    }

    #[test]
    fn selection_stays_inside_the_rows() {
        let rows = filter(&default_entries(), "split");
        assert_eq!(
            selected(&rows, 0).map(|entry| entry.command),
            Some(Command::Split(Axis::Columns))
        );
        assert_eq!(
            selected(&rows, 1).map(|entry| entry.command),
            Some(Command::Split(Axis::Rows))
        );
        assert_eq!(
            selected(&rows, 99).map(|entry| entry.command),
            Some(Command::Split(Axis::Rows))
        );
        assert_eq!(selected(&[], 0), None);
    }

    #[test]
    fn every_palette_command_has_a_distinct_action_name() {
        let rows = default_entries();
        let mut names: Vec<&str> = rows
            .iter()
            .map(|entry| action_name(entry.command))
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), rows.len());
    }
}
