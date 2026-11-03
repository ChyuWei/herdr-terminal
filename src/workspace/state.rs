//! Local UI state that is not part of the persisted workspace: the help
//! overlay, the rename prompt, the command palette, the scrollback search,
//! selection mode, the last message, the prefix and the connection state.
//! Pure and unit-testable.

use super::agents::Overlay;
use super::commands::RenameTarget;
use super::keys::KeyMap;
use super::model::PaneId;

/// A one-line editable text field, used by the palette and the search bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub text: Vec<char>,
    pub cursor: usize,
}

impl Line {
    pub fn new(text: &str) -> Line {
        let text: Vec<char> = text.chars().collect();
        Line {
            cursor: text.len(),
            text,
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    /// Applies an editing action. Commit and Cancel are not edits; they are
    /// handled by the overlay that owns the line.
    pub fn edit(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::Insert(c) => {
                if c.is_control() {
                    return false;
                }
                self.text.insert(self.cursor, c);
                self.cursor += 1;
            }
            Edit::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
            }
            Edit::Delete => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
            }
            Edit::Left => self.cursor = self.cursor.saturating_sub(1),
            Edit::Right => {
                if self.cursor < self.text.len() {
                    self.cursor += 1;
                }
            }
            Edit::Home => self.cursor = 0,
            Edit::End => self.cursor = self.text.len(),
            Edit::Commit | Edit::Cancel => return false,
        }
        true
    }

    /// The text before the cursor, for placing a cursor on screen.
    pub fn before(&self) -> String {
        self.text[..self.cursor].iter().collect()
    }
}

/// The screen's local view state.
#[derive(Clone, Debug)]
pub struct UiState {
    pub help: bool,
    pub prompt: Option<Prompt>,
    /// The modal command palette, when open.
    pub palette: Option<PaletteState>,
    /// The scrollback search bar, when open.
    pub search: Option<SearchState>,
    /// The pane whose local selection is being edited by the keyboard, when
    /// selection mode is on.
    pub selecting: Option<PaneId>,
    /// A transient message for the status bar (kept addresses, exit status).
    pub message: Option<String>,
    /// The prefix key was pressed and the next key is a command.
    pub prefix: bool,
    pub connection: Connection,
    /// The modal agent sidebar and its selection.
    pub sidebar: Sidebar,
    /// The Core-wide terminal manager and its local selection/filter.
    pub manager: ManagerState,
    /// The machine manager and its local selection/filter/prompt.
    pub machines: MachineState,
    /// The file viewer/editor overlay: open, its path prompt and its error.
    pub files: FileState,
    /// The read-only observer preview, when open.
    pub preview: Option<PreviewState>,
    /// The agent being peeked, when the peek overlay is open.
    pub peek: Option<PaneId>,
    /// The effective prefix key map, so the palette can show current hints.
    pub keymap: KeyMap,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState {
            help: false,
            prompt: None,
            palette: None,
            search: None,
            selecting: None,
            message: None,
            prefix: false,
            connection: Connection::Live,
            sidebar: Sidebar::default(),
            manager: ManagerState::default(),
            machines: MachineState::default(),
            files: FileState::default(),
            preview: None,
            peek: None,
            keymap: KeyMap::default(),
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
        } else if self.palette.is_some() {
            Overlay::Palette
        } else if self.search.is_some() {
            Overlay::Search
        } else if self.preview.is_some() {
            Overlay::Preview
        } else if let Some(pane) = self.peek {
            Overlay::Peek(pane)
        } else if self.manager.open {
            Overlay::Manager
        } else if self.machines.open {
            Overlay::Machines
        } else if self.files.open {
            Overlay::Files
        } else if self.sidebar.open {
            Overlay::Sidebar
        } else {
            Overlay::None
        }
    }

    /// Whether an overlay other than the text selection owns every key and
    /// mouse event. A selection is modal for keys but lets a fresh mouse press
    /// through, so mouse routing checks this rather than [`Self::modal`].
    pub fn overlay(&self) -> bool {
        self.help
            || self.prompt.is_some()
            || self.palette.is_some()
            || self.search.is_some()
            || self.peek.is_some()
            || self.manager.open
            || self.machines.open
            || self.files.open
            || self.preview.is_some()
            || self.sidebar.open
    }

    /// Whether an overlay currently owns every key.
    pub fn modal(&self) -> bool {
        self.overlay() || self.selecting.is_some()
    }
}

/// The command palette's editable query and selected row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub line: Line,
    pub selected: usize,
}

impl PaletteState {
    /// Keeps the selection inside `len` rows after a filter change.
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(len - 1) as isize;
        let next = (current + delta).rem_euclid(len as isize);
        self.selected = next as usize;
    }
}

/// The scrollback search bar's query and the latest counts from the local VT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchState {
    pub pane: PaneId,
    pub line: Line,
    /// Matches found so far; grows while the search catches up.
    pub total: usize,
    /// The selected match as a 0-based index in newest-to-oldest order.
    pub selected: Option<usize>,
    /// The local VT has caught up with the query it was given.
    pub complete: bool,
    /// The query changed and the search restarted.
    pub pending: bool,
}

impl SearchState {
    pub fn new(pane: PaneId) -> SearchState {
        SearchState {
            pane,
            line: Line::default(),
            total: 0,
            selected: None,
            complete: false,
            pending: false,
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

/// The terminal manager's local state. The selection is an address, not a
/// pane id: a hidden terminal has no pane. The filter and selection are never
/// part of the shared document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagerState {
    pub open: bool,
    pub selected: Option<String>,
    pub filter: Line,
    pub filtering: bool,
    pub offset: usize,
}

/// The machine manager's local state. The selection is a core machine name,
/// not a pane or an ssh target. A prompt edits a new target or a new name; a
/// confirmation holds the machine a `d` key would remove. None of it is ever
/// part of the shared document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineState {
    pub open: bool,
    pub selected: Option<String>,
    pub filter: Line,
    pub filtering: bool,
    pub offset: usize,
    pub prompt: Option<MachinePrompt>,
    /// The machine waiting for the second `d`/Enter of a removal.
    pub confirm: Option<String>,
    /// Core's refusal or a connection problem, shown inside the overlay.
    pub error: Option<String>,
}

/// The file overlay's local state. The path prompt is shown before a read;
/// an error stays visible inside the overlay while the person corrects the
/// path. The viewer, diff and buffers are runtime state, not UI state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileState {
    pub open: bool,
    /// Where the overlay's path prompt points: the focused pane's machine and
    /// cwd at the moment it opened.
    pub context: super::file::Context,
    /// The path prompt, prefilled from the focused pane's cwd.
    pub prompt: Option<Line>,
    /// Core's refusal or a local IO problem, shown inside the overlay.
    pub error: Option<String>,
}

/// The one-line machine prompt: add a target, or rename the selected machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinePrompt {
    pub kind: MachinePromptKind,
    pub line: Line,
}

/// What a machine prompt will call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachinePromptKind {
    /// `machine.add` with the typed target unchanged. Core chooses the name.
    Add,
    /// `machine.rename` for the captured machine name.
    Rename { machine: String },
}

impl MachinePrompt {
    pub fn add() -> MachinePrompt {
        MachinePrompt {
            kind: MachinePromptKind::Add,
            line: Line::default(),
        }
    }

    pub fn rename(machine: &str) -> MachinePrompt {
        MachinePrompt {
            kind: MachinePromptKind::Rename {
                machine: machine.to_owned(),
            },
            line: Line::new(machine),
        }
    }

    /// The status-bar label before the editable text.
    pub fn label(&self) -> &'static str {
        match self.kind {
            MachinePromptKind::Add => "add machine target: ",
            MachinePromptKind::Rename { .. } => "rename machine: ",
        }
    }
}

/// The address and title of the terminal being previewed. The stream and VT
/// live in the runtime; this is what the overlays and input rules read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewState {
    pub address: String,
    pub title: String,
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
        ui.search = Some(SearchState::new(PaneId(3)));
        assert_eq!(ui.agent_overlay(), Overlay::Search);
        ui.palette = Some(PaletteState::default());
        assert_eq!(ui.agent_overlay(), Overlay::Palette);
        ui.prompt = Some(prompt());
        assert_eq!(ui.agent_overlay(), Overlay::Prompt);
        ui.help = true;
        assert_eq!(ui.agent_overlay(), Overlay::Help);
    }

    #[test]
    fn a_line_edits_at_the_cursor_and_ignores_commit() {
        let mut line = Line::new("ab");
        assert!(line.edit(Edit::Home));
        assert!(line.edit(Edit::Insert('X')));
        assert_eq!(line.text(), "Xab");
        assert_eq!(line.before(), "X");
        assert!(line.edit(Edit::End));
        assert!(line.edit(Edit::Backspace));
        assert_eq!(line.text(), "Xa");
        assert!(line.edit(Edit::Delete));
        assert_eq!(line.text(), "Xa");
        assert!(!line.edit(Edit::Commit));
        assert!(!line.edit(Edit::Cancel));
        assert!(!line.edit(Edit::Insert('\u{2}')));
    }

    #[test]
    fn the_palette_selection_clamps_and_wraps() {
        let mut palette = PaletteState::default();
        palette.move_by(1, 0);
        assert_eq!(palette.selected, 0);
        palette.move_by(1, 3);
        assert_eq!(palette.selected, 1);
        palette.move_by(-2, 3);
        assert_eq!(palette.selected, 2);
        palette.selected = 9;
        palette.clamp(3);
        assert_eq!(palette.selected, 2);
        palette.clamp(0);
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn a_modal_ui_is_not_forwarding_keys() {
        let mut ui = UiState::default();
        assert!(!ui.modal());
        ui.selecting = Some(PaneId(2));
        assert!(ui.modal());
        ui.selecting = None;
        ui.search = Some(SearchState::new(PaneId(2)));
        assert!(ui.modal());
    }
}
