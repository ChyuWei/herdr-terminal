//! What one rendered frame of a terminal looks like.
//!
//! These are plain data: the VT wrapper fills them in from Ghostty's render
//! state, and the UI turns them into ratatui cells. Nothing here talks to
//! core or to a terminal.

/// A cell color. Palette entries are resolved to RGB from the terminal's
/// active palette; `Default` means "the terminal's default", which the UI
/// paints with [`Screen::default_fg`] / [`Screen::default_bg`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Rgb(u8, u8, u8),
}

/// Text attributes a terminal cell can carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strike: bool,
    pub overline: bool,
}

/// One terminal cell.
///
/// `width` is 1 for an ordinary cell, 2 for a wide character (whose text
/// occupies this cell and the next), and 0 for a wide character's spacer.
/// A spacer is never drawn; the UI skips it so the wide glyph keeps its
/// second column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub underline_color: Option<Color>,
    pub attrs: Attrs,
    pub width: u8,
}

impl Cell {
    /// A blank cell with the given colors.
    pub fn blank(fg: Color, bg: Color) -> Cell {
        Cell {
            text: " ".into(),
            fg,
            bg,
            underline_color: None,
            attrs: Attrs::default(),
            width: 1,
        }
    }
}

/// The visual shape of the cursor (DECSCUSR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
    Hollow,
}

/// The cursor's position in the viewport and how it should be drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    /// Visible both by mode (DECTCEM) and inside the viewport.
    pub visible: bool,
    pub blinking: bool,
    pub shape: CursorShape,
}

/// One full frame: the visible grid, the cursor, and the terminal title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    /// `rows * cols` cells in row-major order.
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
    pub title: String,
    /// The terminal's current default foreground and background.
    pub default_fg: Color,
    pub default_bg: Color,
}

impl Screen {
    pub fn cell(&self, x: u16, y: u16) -> Option<&Cell> {
        if x >= self.cols || y >= self.rows {
            return None;
        }
        self.cells.get(y as usize * self.cols as usize + x as usize)
    }

    /// The row's text with trailing blanks removed. Wide characters appear
    /// once; their spacer cells contribute nothing.
    pub fn line(&self, y: u16) -> String {
        let mut line = String::new();
        for x in 0..self.cols {
            if let Some(cell) = self.cell(x, y)
                && cell.width != 0
            {
                line.push_str(&cell.text);
            }
        }
        line.trim_end().to_owned()
    }

    /// Every row's [`Screen::line`], for golden tests and debugging.
    pub fn lines(&self) -> Vec<String> {
        (0..self.rows).map(|y| self.line(y)).collect()
    }
}
