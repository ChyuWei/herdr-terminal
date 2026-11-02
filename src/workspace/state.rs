//! Local UI state that is not part of the persisted workspace: the help
//! overlay, the rename prompt, the last message, the prefix and the
//! connection state. Pure and unit-testable.

use super::agents::Overlay;
use super::commands::RenameTarget;
use super::model::PaneId;

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
    /// The modal agent sidebar and its selection.
    pub sidebar: Sidebar,
    /// The agent being peeked, when the peek overlay is open.
    pub peek: Option<PaneId>,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState {
            help: false,
            prompt: None,
            message: None,
            prefix: false,
            connection: Connection::Live,
            sidebar: Sidebar::default(),
            peek: None,
        }
    }
}

impl UiState {
    /// What covers the panes right now, for the visible-agent rules. A peek
    /// is checked before the sidebar because it counts only its target.
    pub fn agent_overlay(&self) -> Overlay {
        if self.help {
            Overlay::Help
        } else if self.prompt.is_some() {
            Overlay::Prompt
        } else if let Some(pane) = self.peek {
            Overlay::Peek(pane)
        } else if self.sidebar.open {
            Overlay::Sidebar
        } else {
            Overlay::None
        }
    }
}

/// The agent sidebar: open flag, selection by pane id, and scroll offset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sidebar {
    pub open: bool,
    pub selected: Option<PaneId>,
    pub offset: usize,
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

    #[test]
    fn the_agent_overlay_names_what_covers_the_panes() {
        let mut ui = UiState::default();
        assert_eq!(ui.agent_overlay(), Overlay::None);
        ui.sidebar.open = true;
        assert_eq!(ui.agent_overlay(), Overlay::Sidebar);
        ui.peek = Some(PaneId(7));
        assert_eq!(
            ui.agent_overlay(),
            Overlay::Peek(PaneId(7)),
            "a peek over the sidebar counts only its target"
        );
        ui.prompt = Some(prompt());
        assert_eq!(ui.agent_overlay(), Overlay::Prompt);
        ui.help = true;
        assert_eq!(ui.agent_overlay(), Overlay::Help);
    }
}
