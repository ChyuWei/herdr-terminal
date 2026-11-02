//! Drawing one [`Screen`] into a ratatui frame.
//!
//! This is a pure conversion: it never touches core, the VT or the terminal.
//! The buffer is filled cell by cell; wide characters mark the following
//! spacer cell as skipped so the glyph keeps both columns.

use crate::vt::{Cell, Color, Cursor, Screen};
use ratatui::Frame;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::{Color as RatColor, Modifier, Style};

pub fn draw(frame: &mut Frame, screen: &Screen) {
    let area = frame.area();
    draw_buffer(frame.buffer_mut(), area, screen);
    if screen.cursor.visible && screen.cursor.x < area.width && screen.cursor.y < area.height {
        frame.set_cursor_position((screen.cursor.x, screen.cursor.y));
    }
}

/// Fills `buffer` with the screen, clipped to `area`.
pub fn draw_buffer(buffer: &mut Buffer, area: Rect, screen: &Screen) {
    let cols = screen.cols.min(area.width);
    let rows = screen.rows.min(area.height);
    for y in 0..rows {
        for x in 0..cols {
            let Some(cell) = screen.cell(x, y) else {
                continue;
            };
            let target = &mut buffer[(x, y)];
            if cell.width == 0 {
                // Covered by the wide character to the left.
                target.set_symbol("").set_diff_option(CellDiffOption::Skip);
                continue;
            }
            target.set_symbol(&cell.text);
            target.set_style(style(cell));
            if cell.width == 2 && x + 1 < cols {
                buffer[(x + 1, y)]
                    .set_symbol("")
                    .set_diff_option(CellDiffOption::Skip);
            }
        }
    }
}

fn style(cell: &Cell) -> Style {
    // A cell without a color of its own is left to the outer terminal, so a
    // light or themed terminal is not painted over with the VT's defaults.
    // (Program-set OSC 10/11 defaults are not propagated in M1; M6 owns themes.)
    let mut style = Style::default().fg(color(cell.fg)).bg(color(cell.bg));
    if let Some(underline) = cell.underline_color {
        style = style.underline_color(color(underline));
    }
    let attrs = cell.attrs;
    let mut modifier = Modifier::empty();
    if attrs.bold {
        modifier |= Modifier::BOLD;
    }
    if attrs.dim {
        modifier |= Modifier::DIM;
    }
    if attrs.italic {
        modifier |= Modifier::ITALIC;
    }
    if attrs.underline {
        modifier |= Modifier::UNDERLINED;
    }
    if attrs.blink {
        modifier |= Modifier::SLOW_BLINK;
    }
    if attrs.reverse {
        modifier |= Modifier::REVERSED;
    }
    if attrs.hidden {
        modifier |= Modifier::HIDDEN;
    }
    if attrs.strike {
        modifier |= Modifier::CROSSED_OUT;
    }
    // ratatui has no overline modifier; the snapshot keeps it for later.
    style.add_modifier(modifier)
}

/// A cell color. `Default` means the cell has no color of its own, so the
/// outer terminal's theme applies.
fn color(color: Color) -> RatColor {
    match color {
        Color::Rgb(r, g, b) => RatColor::Rgb(r, g, b),
        Color::Default => RatColor::Reset,
    }
}

/// The crossterm cursor shape for a cursor. Hollow blocks have no crossterm
/// equivalent and are drawn as a solid block.
pub fn cursor_shape(cursor: &Cursor) -> crossterm::cursor::SetCursorStyle {
    use crate::vt::CursorShape;
    use crossterm::cursor::SetCursorStyle as S;
    match (cursor.shape, cursor.blinking) {
        (CursorShape::Block, true) => S::BlinkingBlock,
        (CursorShape::Block | CursorShape::Hollow, false) => S::SteadyBlock,
        (CursorShape::Hollow, true) => S::BlinkingBlock,
        (CursorShape::Bar, true) => S::BlinkingBar,
        (CursorShape::Bar, false) => S::SteadyBar,
        (CursorShape::Underline, true) => S::BlinkingUnderScore,
        (CursorShape::Underline, false) => S::SteadyUnderScore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::{Attrs, CursorShape};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cell(text: &str, width: u8) -> Cell {
        Cell {
            text: text.into(),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: Attrs::default(),
            width,
        }
    }

    fn draw_screen(screen: &Screen) -> ratatui::buffer::Buffer {
        let area = Rect::new(0, 0, screen.cols, screen.rows);
        let mut buffer = Buffer::empty(area);
        draw_buffer(&mut buffer, area, screen);
        buffer
    }

    #[test]
    fn text_and_wide_cells_reach_the_buffer() {
        let mut cells = vec![cell(" ", 1); 6];
        cells[0] = cell("你", 2);
        cells[1] = cell("", 0);
        cells[2] = cell("a", 1);
        let screen = Screen {
            cols: 6,
            rows: 1,
            cells,
            ..Screen::default()
        };
        let buffer = draw_screen(&screen);
        assert_eq!(buffer[(0, 0)].symbol(), "你");
        assert_eq!(buffer[(1, 0)].diff_option, CellDiffOption::Skip);
        assert_eq!(buffer[(2, 0)].symbol(), "a");
    }

    #[test]
    fn a_visible_cursor_is_placed_and_a_hidden_one_is_not() {
        let screen = Screen {
            cols: 3,
            rows: 1,
            cells: vec![cell(" ", 1); 3],
            cursor: Cursor {
                x: 2,
                y: 0,
                visible: true,
                ..Cursor::default()
            },
            ..Screen::default()
        };
        let backend = TestBackend::new(3, 1);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &screen))
            .expect("draw frame");
        terminal
            .backend_mut()
            .assert_cursor_position(ratatui::layout::Position::new(2, 0));
    }

    #[test]
    fn cursor_shapes_map_to_decscusr() {
        use crossterm::cursor::SetCursorStyle as S;
        let cursor = |shape, blinking| Cursor {
            shape,
            blinking,
            ..Cursor::default()
        };
        assert_eq!(
            cursor_shape(&cursor(CursorShape::Block, true)),
            S::BlinkingBlock
        );
        assert_eq!(
            cursor_shape(&cursor(CursorShape::Underline, false)),
            S::SteadyUnderScore
        );
        assert_eq!(
            cursor_shape(&cursor(CursorShape::Bar, true)),
            S::BlinkingBar
        );
    }
}
