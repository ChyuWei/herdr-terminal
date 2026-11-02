//! Local UI state that is not part of the persisted workspace: the help
//! overlay, the rename prompt, the last message, the prefix and the
//! connection state. Pure and unit-testable.

use super::commands::RenameTarget;

/// The screen's local view state.
#[derive(Clone, Debug)]
pub struct UiState {
    pub help: bool,
    pub prompt: Option<Prompt>,
    /// A transient message for the status bar (kept addresses, exit status).
    pub message: Option<String>,
    /// The prefix key was pressed and the next key is a command.
    pub prefix: bool,
    pub connection: Connection,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState {
            help: false,
            prompt: None,
            message: None,
            prefix: false,
            connection: Connection::Live,
        }
    }
}

/// Whether the client currently has a usable core connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Live,
    Reconnecting {
        attempt: u32,
    },
}

/// A one-line rename prompt. The ids are captured when it opens, so a
/// background selection change cannot rename the wrong object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub target: RenameTarget,
    pub text: Vec<char>,
    pub cursor: usize,
}

/// One editing action from the input layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Insert(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    /// Enter: close the prompt and commit the trimmed text.
    Commit,
    /// Escape: close the prompt and change nothing.
    Cancel,
}

/// What the prompt did with an edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptEdit {
    /// The text or cursor changed; redraw.
    Changed,
    /// The prompt is done; the runtime applies the name if non-empty.
    Commit(String),
    /// The prompt is done; no change.
    Cancel,
}

impl Prompt {
    pub fn new(target: RenameTarget, current: &str) -> Prompt {
        let text: Vec<char> = current.chars().collect();
        Prompt {
            target,
            cursor: text.len(),
            text,
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    /// The label before the text: "rename tab: " or "rename space: ".
    pub fn label(&self) -> &'static str {
        match self.target {
            RenameTarget::Tab { .. } => "rename tab: ",
            RenameTarget::Space(_) => "rename space: ",
        }
    }

    pub fn edit(&mut self, edit: Edit) -> PromptEdit {
        match edit {
            Edit::Insert(c) => {
                if c.is_control() {
                    return PromptEdit::Changed;
                }
                self.text.insert(self.cursor, c);
                self.cursor += 1;
                PromptEdit::Changed
            }
            Edit::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
                PromptEdit::Changed
            }
            Edit::Delete => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
                PromptEdit::Changed
            }
            Edit::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                PromptEdit::Changed
            }
            Edit::Right => {
                if self.cursor < self.text.len() {
                    self.cursor += 1;
                }
                PromptEdit::Changed
            }
            Edit::Home => {
                self.cursor = 0;
                PromptEdit::Changed
            }
            Edit::End => {
                self.cursor = self.text.len();
                PromptEdit::Changed
            }
            Edit::Commit => PromptEdit::Commit(self.text().trim().to_owned()),
            Edit::Cancel => PromptEdit::Cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> Prompt {
        Prompt::new(
            RenameTarget::Tab {
                space: super::super::model::SpaceId(1),
                tab: super::super::model::TabId(2),
            },
            "tab 1",
        )
    }

    #[test]
    fn editing_inserts_and_deletes_at_the_cursor() {
        let mut prompt = prompt();
        prompt.edit(Edit::Home);
        prompt.edit(Edit::Insert('X'));
        assert_eq!(prompt.text(), "Xtab 1");
        prompt.edit(Edit::End);
        prompt.edit(Edit::Backspace);
        assert_eq!(prompt.text(), "Xtab ");
        prompt.edit(Edit::Delete);
        assert_eq!(prompt.text(), "Xtab ");
        prompt.edit(Edit::Left);
        prompt.edit(Edit::Delete);
        assert_eq!(prompt.text(), "Xtab");
        assert_eq!(prompt.cursor, 4);
    }

    #[test]
    fn arrows_and_home_end_stay_in_bounds() {
        let mut prompt = prompt();
        for _ in 0..20 {
            prompt.edit(Edit::Left);
        }
        assert_eq!(prompt.cursor, 0);
        prompt.edit(Edit::Right);
        assert_eq!(prompt.cursor, 1);
        prompt.edit(Edit::End);
        assert_eq!(prompt.cursor, 5);
        prompt.edit(Edit::Right);
        assert_eq!(prompt.cursor, 5);
        prompt.edit(Edit::Home);
        assert_eq!(prompt.cursor, 0);
    }

    #[test]
    fn enter_commits_trimmed_text_and_escape_cancels() {
        let mut prompt = prompt();
        prompt.edit(Edit::Home);
        prompt.edit(Edit::Insert(' '));
        assert_eq!(
            prompt.edit(Edit::Commit),
            PromptEdit::Commit("tab 1".into())
        );
        assert_eq!(prompt.edit(Edit::Cancel), PromptEdit::Cancel);
    }

    #[test]
    fn control_characters_are_not_typed_into_the_prompt() {
        let mut prompt = prompt();
        let before = prompt.text();
        prompt.edit(Edit::Insert('\u{2}'));
        assert_eq!(prompt.text(), before);
    }

    #[test]
    fn the_label_names_the_target() {
        assert_eq!(prompt().label(), "rename tab: ");
        let space = Prompt::new(RenameTarget::Space(super::super::model::SpaceId(1)), "s");
        assert_eq!(space.label(), "rename space: ");
    }
}
