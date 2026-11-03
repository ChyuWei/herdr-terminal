//! A safe wrapper around Ghostty's VT library.
//!
//! [`Vt`] owns a terminal, a render state and the input encoders. Feeding it
//! bytes updates the terminal; [`Vt::snapshot`] copies the visible grid,
//! cursor and title out as plain data; [`Vt::encode_key`] and
//! [`Vt::encode_mouse`] turn crossterm events into the bytes the program
//! expects, honoring its kitty keyboard and mouse modes.
//!
//! Nothing here talks to core. The screen loop feeds attach bytes in and sends
//! the encoded input back as DATA.

mod keys;
mod mouse;
mod screen;
mod scroll;
mod search;
mod selection;

pub use keys::is_detach;
pub use screen::{Attrs, Cell, Color, Cursor, CursorShape, Screen};
pub use scroll::Scrollbar;
pub use search::SearchProgress;

use ghostty_sys as sys;
use std::ffi::{c_char, c_void};
use std::fmt;
use std::ptr;

/// A VT operation failed. The only failures are allocation and invalid
/// arguments, so the message names the operation that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VtError {
    what: &'static str,
}

impl fmt::Display for VtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VT: {} failed", self.what)
    }
}

impl std::error::Error for VtError {}

type Result<T> = std::result::Result<T, VtError>;

/// The win32-input-mode watcher's place in the program's output stream.
///
/// Only a DECSET/DECRST request in ground state may change the mode; bytes
/// inside OSC, DCS, SOS, PM or APC payloads are skipped, so a title that
/// happens to contain the marker text cannot turn win32 input on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Ground,
    Escape,
    Csi,
    Osc,
    String,
    StringEscape,
}

/// The parameter bytes of the request the watcher recognizes.
const WIN32_MODE: &[u8] = b"?9001";

/// A Ghostty terminal with the state needed to render and encode input.
///
/// It is not `Send`: it is used from the thread that owns the screen.
pub struct Vt {
    terminal: sys::GhosttyTerminal,
    render: sys::GhosttyRenderState,
    rows: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
    key_encoder: sys::GhosttyKeyEncoder,
    mouse_encoder: sys::GhosttyMouseEncoder,
    /// The local scrollback search, when one is open.
    search: Option<search::Search>,
    /// The local cell selection, when one is active.
    selection: Option<selection::Selection>,
    cols: u16,
    rows_count: u16,
    buttons: u8,
    win32: bool,
    scan: ScanState,
    csi: [u8; 8],
    csi_len: usize,
}

/// DEC private mode 2027, grapheme clustering. The C header defines its
/// packed value with a function-like macro, which bindgen cannot emit, so the
/// DEC mode number is spelled out here. Bit 15 clear means a private mode.
const GRAPHEME_CLUSTER_MODE: u16 = 2027;

/// Bytes of memory allowed per configured scrollback line. Ghostty also
/// enforces a byte limit; its default is tiny (10 KB) and would prune a large
/// history long before the line limit. This estimate keeps the line limit in
/// charge while still bounding memory.
const SCROLLBACK_BYTES_PER_LINE: usize = 2 * 1024;
/// The absolute scrollback memory bound, even when config asks for a million
/// lines.
const MAX_SCROLLBACK_BYTES: usize = 64 * 1024 * 1024;

/// The byte budget paired with a line budget.
fn scrollback_bytes(max_lines: usize) -> usize {
    max_lines
        .saturating_mul(SCROLLBACK_BYTES_PER_LINE)
        .min(MAX_SCROLLBACK_BYTES)
}

impl Vt {
    /// Creates a terminal of at least one column and one row, with no local
    /// scrollback. The workspace uses [`Vt::new_with_scrollback`].
    pub fn new(cols: u16, rows: u16) -> Result<Vt> {
        Vt::new_with_scrollback(cols, rows, 0)
    }

    /// Creates a terminal with a bounded local scrollback of `max_lines`
    /// lines (0 keeps only the viewport). History lives in the VT; nothing is
    /// duplicated by this client.
    pub fn new_with_scrollback(cols: u16, rows: u16, max_lines: usize) -> Result<Vt> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // SAFETY: every pointer is an out-parameter for a fresh handle, and
        // the dimensions are non-zero as the API requires.
        unsafe {
            let mut terminal = ptr::null_mut();
            check(
                sys::ghostty_terminal_new(ptr::null(), &mut terminal, cols, rows),
                "terminal_new",
            )?;
            if terminal.is_null() {
                return Err(VtError {
                    what: "terminal_new",
                });
            }
            // Grapheme clustering is the terminal's reset default, like core,
            // Ghostty and the app: emoji variation sequences and combining
            // marks form one cell cluster instead of splitting into cells.
            let mode = sys::GhosttyTerminalModeConfig {
                mode: GRAPHEME_CLUSTER_MODE,
                value: true,
            };
            if sys::ghostty_terminal_set(
                terminal,
                sys::GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                &mode as *const _ as *const c_void,
            ) != sys::GHOSTTY_SUCCESS
            {
                sys::ghostty_terminal_free(terminal);
                return Err(VtError {
                    what: "set_mode_default",
                });
            }
            let lines = max_lines;
            let bytes = scrollback_bytes(max_lines);
            // The line limit is the user-visible one; the byte limit bounds
            // memory for programs that use many styles or wide cells.
            let result = sys::ghostty_terminal_set(
                terminal,
                sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                &lines as *const usize as *const c_void,
            );
            if result == sys::GHOSTTY_SUCCESS {
                // SAFETY: the input pointer is a valid size_t for this option.
                sys::ghostty_terminal_set(
                    terminal,
                    sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                    &bytes as *const usize as *const c_void,
                );
            }
            if result != sys::GHOSTTY_SUCCESS {
                sys::ghostty_terminal_free(terminal);
                return Err(VtError {
                    what: "set_scrollback_max_lines",
                });
            }
            let mut render = ptr::null_mut();
            if let Err(error) = check(
                sys::ghostty_render_state_new(ptr::null(), &mut render),
                "render_state_new",
            ) {
                sys::ghostty_terminal_free(terminal);
                return Err(error);
            }
            let mut row_iterator = ptr::null_mut();
            if let Err(error) = check(
                sys::ghostty_render_state_row_iterator_new(ptr::null(), &mut row_iterator),
                "row_iterator_new",
            ) {
                sys::ghostty_render_state_free(render);
                sys::ghostty_terminal_free(terminal);
                return Err(error);
            }
            let mut cells = ptr::null_mut();
            if let Err(error) = check(
                sys::ghostty_render_state_row_cells_new(ptr::null(), &mut cells),
                "row_cells_new",
            ) {
                sys::ghostty_render_state_row_iterator_free(row_iterator);
                sys::ghostty_render_state_free(render);
                sys::ghostty_terminal_free(terminal);
                return Err(error);
            }
            let mut key_encoder = ptr::null_mut();
            if let Err(error) = check(
                sys::ghostty_key_encoder_new(ptr::null(), &mut key_encoder),
                "key_encoder_new",
            ) {
                sys::ghostty_render_state_row_cells_free(cells);
                sys::ghostty_render_state_row_iterator_free(row_iterator);
                sys::ghostty_render_state_free(render);
                sys::ghostty_terminal_free(terminal);
                return Err(error);
            }
            let mut mouse_encoder = ptr::null_mut();
            if let Err(error) = check(
                sys::ghostty_mouse_encoder_new(ptr::null(), &mut mouse_encoder),
                "mouse_encoder_new",
            ) {
                sys::ghostty_key_encoder_free(key_encoder);
                sys::ghostty_render_state_row_cells_free(cells);
                sys::ghostty_render_state_row_iterator_free(row_iterator);
                sys::ghostty_render_state_free(render);
                sys::ghostty_terminal_free(terminal);
                return Err(error);
            }
            Ok(Vt {
                terminal,
                render,
                rows: row_iterator,
                cells,
                key_encoder,
                mouse_encoder,
                search: None,
                selection: None,
                cols,
                rows_count: rows,
                buttons: 0,
                win32: false,
                scan: ScanState::Ground,
                csi: [0; 8],
                csi_len: 0,
            })
        }
    }

    /// Feeds program output (or a first paint) into the VT.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.watch_win32(bytes);
        // SAFETY: the terminal is live and the slice is valid for its length.
        unsafe { sys::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    /// Ghostty does not track win32-input-mode, so watch the program's output
    /// for the request itself. The state persists across feeds, so a request
    /// split across two chunks is still seen, and payloads of OSC/DCS/APC
    /// strings are skipped rather than mistaken for a request.
    fn watch_win32(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.scan {
                ScanState::Ground => {
                    if byte == 0x1b {
                        self.scan = ScanState::Escape;
                    }
                }
                ScanState::Escape => match byte {
                    b'[' => {
                        self.scan = ScanState::Csi;
                        self.csi_len = 0;
                    }
                    b']' => self.scan = ScanState::Osc,
                    b'P' | b'X' | b'^' | b'_' => self.scan = ScanState::String,
                    0x1b => {}
                    _ => self.scan = ScanState::Ground,
                },
                ScanState::Csi => match byte {
                    0x1b => self.scan = ScanState::Escape,
                    0x40..=0x7e => {
                        if (byte == b'h' || byte == b'l')
                            && self.csi_len == WIN32_MODE.len()
                            && &self.csi[..self.csi_len] == WIN32_MODE
                        {
                            self.win32 = byte == b'h';
                        }
                        self.scan = ScanState::Ground;
                    }
                    0x20..=0x3f => {
                        if self.csi_len < self.csi.len() {
                            self.csi[self.csi_len] = byte;
                        }
                        // Longer than the request we recognize; never match.
                        self.csi_len = self.csi_len.saturating_add(1);
                    }
                    _ => {}
                },
                ScanState::Osc => match byte {
                    0x07 => self.scan = ScanState::Ground,
                    0x1b => self.scan = ScanState::StringEscape,
                    _ => {}
                },
                ScanState::String => {
                    if byte == 0x1b {
                        self.scan = ScanState::StringEscape;
                    }
                }
                ScanState::StringEscape => match byte {
                    b'\\' => self.scan = ScanState::Ground,
                    0x1b => {}
                    _ => self.scan = ScanState::String,
                },
            }
        }
    }

    /// Changes the grid size. The next snapshot reflects the new size.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // SAFETY: the terminal is live; cell pixel sizes are irrelevant to a
        // cell-grid renderer but must be non-zero.
        let result = unsafe { sys::ghostty_terminal_resize(self.terminal, cols, rows, 1, 1) };
        check(result, "terminal_resize")?;
        self.cols = cols;
        self.rows_count = rows;
        Ok(())
    }

    /// The size the VT was last given.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows_count)
    }

    /// Copies the visible grid, cursor and title out of the render state.
    pub fn snapshot(&mut self) -> Result<Screen> {
        // SAFETY: the terminal and render state are live and only used here.
        unsafe {
            check(
                sys::ghostty_render_state_update(self.render, self.terminal),
                "render_state_update",
            )?;

            let mut colors = sys::GhosttyRenderStateColors {
                size: std::mem::size_of::<sys::GhosttyRenderStateColors>(),
                ..Default::default()
            };
            let colors_ok = sys::ghostty_render_state_get(
                self.render,
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS;

            let mut cols: u16 = 0;
            let mut rows: u16 = 0;
            sys::ghostty_render_state_get(
                self.render,
                sys::GHOSTTY_RENDER_STATE_DATA_COLS,
                &mut cols as *mut _ as *mut c_void,
            );
            sys::ghostty_render_state_get(
                self.render,
                sys::GHOSTTY_RENDER_STATE_DATA_ROWS,
                &mut rows as *mut _ as *mut c_void,
            );
            let cols = cols.max(1);
            let rows = rows.max(1);
            self.cols = cols;
            self.rows_count = rows;

            let default_fg = if colors_ok {
                color_of(colors.foreground)
            } else {
                Color::Default
            };
            let default_bg = if colors_ok {
                color_of(colors.background)
            } else {
                Color::Default
            };
            let palette = &colors.palette;

            let mut out = Vec::with_capacity(cols as usize * rows as usize);
            let got_iterator = sys::ghostty_render_state_get(
                self.render,
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.rows as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS
                && !self.rows.is_null();
            if got_iterator {
                let iterator = self.rows;
                let mut y = 0u16;
                while sys::ghostty_render_state_row_iterator_next(iterator) {
                    sys::ghostty_render_state_row_get(
                        iterator,
                        sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                        &mut self.cells as *mut _ as *mut c_void,
                    );
                    let mut x = 0u16;
                    while sys::ghostty_render_state_row_cells_next(self.cells) {
                        if x < cols {
                            out.push(self.read_cell(palette));
                        }
                        x = x.saturating_add(1);
                    }
                    // A resize can leave the iterator with fewer cells than
                    // the reported width; fill the rest so the grid is exact.
                    while x < cols {
                        out.push(Cell::blank(default_fg, default_bg));
                        x += 1;
                    }
                    y = y.saturating_add(1);
                    if y >= rows {
                        break;
                    }
                }
                while out.len() < cols as usize * rows as usize {
                    out.push(Cell::blank(default_fg, default_bg));
                }
            } else {
                out.resize(
                    cols as usize * rows as usize,
                    Cell::blank(default_fg, default_bg),
                );
            }

            let mut cursor = sys::GhosttyRenderStateCursor {
                size: std::mem::size_of::<sys::GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            let cursor_ok = sys::ghostty_render_state_get(
                self.render,
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cursor as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS;
            let cursor = if cursor_ok && cursor.viewport_has_value {
                Cursor {
                    x: if cursor.wide_tail {
                        cursor.viewport_x.saturating_sub(1)
                    } else {
                        cursor.viewport_x
                    },
                    y: cursor.viewport_y,
                    visible: cursor.visible,
                    blinking: cursor.blinking,
                    shape: match cursor.visual_style {
                        sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Bar,
                        sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                            CursorShape::Underline
                        }
                        sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                            CursorShape::Hollow
                        }
                        _ => CursorShape::Block,
                    },
                }
            } else {
                Cursor::default()
            };

            // Consume dirty state only after the whole frame has been read.
            sys::ghostty_render_state_clean(self.render);

            Ok(Screen {
                cols,
                rows,
                cells: out,
                cursor,
                title: self.title(),
                default_fg,
                default_bg,
            })
        }
    }

    /// The terminal title set by OSC 0/2.
    pub fn title(&self) -> String {
        let mut string = sys::GhosttyString::default();
        // SAFETY: the out pointer is valid; a borrowed string is copied
        // before the next mutating terminal call.
        let ok = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_TITLE,
                &mut string as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS
        };
        if !ok || string.len == 0 || string.ptr.is_null() {
            return String::new();
        }
        // SAFETY: the API guarantees ptr is valid for len bytes until the next
        // mutating call; this copies it immediately.
        let bytes = unsafe { std::slice::from_raw_parts(string.ptr, string.len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Whether the program asked for Windows win32-input-mode (DEC 9001).
    pub fn win32_input(&self) -> bool {
        self.win32
    }

    /// The kitty keyboard protocol flags the program enabled.
    pub fn kitty_keyboard(&self) -> u8 {
        let mut flags: u8 = 0;
        // SAFETY: the out pointer is a valid u8 for this data kind.
        unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                &mut flags as *mut _ as *mut c_void,
            );
        }
        flags
    }

    /// Whether the program enabled any mouse tracking mode.
    pub fn mouse_tracking(&self) -> bool {
        let mut on = false;
        // SAFETY: the out pointer is a valid bool for this data kind.
        unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                &mut on as *mut _ as *mut c_void,
            );
        }
        on
    }

    /// Reads a DEC private mode.
    pub fn mode(&self, mode: u16) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig {
            // Bit 15 clear means a DEC private mode (`?`-prefixed).
            mode: mode & 0x7fff,
            value: false,
        };
        // SAFETY: the struct is valid input/output for GHOSTTY_TERMINAL_DATA_MODE.
        let ok = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GHOSTTY_TERMINAL_DATA_MODE,
                &mut config as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS
        };
        ok && config.value
    }

    /// Encodes one key event using the program's current keyboard modes.
    pub fn encode_key(&mut self, event: &crossterm::event::KeyEvent) -> Vec<u8> {
        let key = keys::key(event);
        if self.win32_input()
            && self.kitty_keyboard() == 0
            && let Some(bytes) = keys::win32(event)
        {
            return bytes;
        }
        // SAFETY: both handles are live; the event is freed on every path.
        unsafe {
            sys::ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.terminal);
            let mut event_handle = ptr::null_mut();
            if sys::ghostty_key_event_new(ptr::null(), &mut event_handle) != sys::GHOSTTY_SUCCESS
                || event_handle.is_null()
            {
                return Vec::new();
            }
            sys::ghostty_key_event_set_action(event_handle, key.action);
            sys::ghostty_key_event_set_key(event_handle, key.key);
            sys::ghostty_key_event_set_mods(event_handle, key.mods);
            if let Some(text) = &key.text {
                sys::ghostty_key_event_set_utf8(
                    event_handle,
                    text.as_ptr() as *const c_char,
                    text.len(),
                );
            }
            if let Some(codepoint) = key.unshifted {
                sys::ghostty_key_event_set_unshifted_codepoint(event_handle, codepoint);
            }
            let out = encode_buffer(|buf, cap, len| {
                sys::ghostty_key_encoder_encode(self.key_encoder, event_handle, buf, cap, len)
            });
            sys::ghostty_key_event_free(event_handle);
            out
        }
    }

    /// Encodes one mouse event using the program's current tracking mode and
    /// format. Empty when the program does not want the event.
    pub fn encode_mouse(&mut self, event: &crossterm::event::MouseEvent) -> Vec<u8> {
        let Some(mouse) = mouse::mouse(event) else {
            return Vec::new();
        };
        let bit = button_bit(mouse.button);
        if mouse.holds_button {
            self.buttons |= bit;
        }
        if mouse.releases_button {
            self.buttons &= !bit;
        }
        // SAFETY: the encoders are live; the event is freed on every path.
        unsafe {
            sys::ghostty_mouse_encoder_setopt_from_terminal(self.mouse_encoder, self.terminal);
            let size = sys::GhosttyMouseEncoderSize {
                size: std::mem::size_of::<sys::GhosttyMouseEncoderSize>(),
                screen_width: self.cols as u32,
                screen_height: self.rows_count as u32,
                cell_width: 1,
                cell_height: 1,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            };
            sys::ghostty_mouse_encoder_setopt(
                self.mouse_encoder,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                &size as *const _ as *const c_void,
            );
            let any = self.buttons != 0;
            sys::ghostty_mouse_encoder_setopt(
                self.mouse_encoder,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &any as *const bool as *const c_void,
            );

            let mut event_handle = ptr::null_mut();
            if sys::ghostty_mouse_event_new(ptr::null(), &mut event_handle) != sys::GHOSTTY_SUCCESS
                || event_handle.is_null()
            {
                return Vec::new();
            }
            sys::ghostty_mouse_event_set_action(event_handle, mouse.action);
            if let Some(button) = mouse.button {
                sys::ghostty_mouse_event_set_button(event_handle, button);
            } else {
                sys::ghostty_mouse_event_clear_button(event_handle);
            }
            sys::ghostty_mouse_event_set_mods(event_handle, mouse.mods);
            sys::ghostty_mouse_event_set_position(
                event_handle,
                sys::GhosttyMousePosition {
                    x: mouse.column as f32,
                    y: mouse.row as f32,
                },
            );
            let out = encode_buffer(|buf, cap, len| {
                sys::ghostty_mouse_encoder_encode(self.mouse_encoder, event_handle, buf, cap, len)
            });
            sys::ghostty_mouse_event_free(event_handle);
            out
        }
    }

    /// Reads one cell from the current row cells iterator.
    ///
    /// # Safety
    ///
    /// The cells iterator must be positioned on a cell from a render state
    /// that has not been updated since.
    unsafe fn read_cell(&self, palette: &[sys::GhosttyColorRgb; 256]) -> Cell {
        unsafe {
            let cells = self.cells;
            let mut raw: sys::GhosttyCell = 0;
            sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                &mut raw as *mut _ as *mut c_void,
            );
            let mut wide: sys::GhosttyCellWide = sys::GHOSTTY_CELL_WIDE_NARROW;
            sys::ghostty_cell_get(
                raw,
                sys::GHOSTTY_CELL_DATA_WIDE,
                &mut wide as *mut _ as *mut c_void,
            );

            let mut style = sys::GhosttyStyle {
                size: std::mem::size_of::<sys::GhosttyStyle>(),
                ..Default::default()
            };
            sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut style as *mut _ as *mut c_void,
            );

            let mut fg = sys::GhosttyColorRgb::default();
            let fg = if sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                &mut fg as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS
            {
                color_of(fg)
            } else {
                Color::Default
            };
            let mut bg = sys::GhosttyColorRgb::default();
            let bg = if sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                &mut bg as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS
            {
                color_of(bg)
            } else {
                Color::Default
            };

            let spacer = wide == sys::GHOSTTY_CELL_WIDE_SPACER_TAIL
                || wide == sys::GHOSTTY_CELL_WIDE_SPACER_HEAD;
            let mut text = String::new();
            if !spacer && !style.invisible {
                let mut len: u32 = 0;
                sys::ghostty_render_state_row_cells_get(
                    cells,
                    sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    &mut len as *mut _ as *mut c_void,
                );
                if len > 0 {
                    let mut codepoints = vec![0u32; len as usize];
                    sys::ghostty_render_state_row_cells_get(
                        cells,
                        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                        codepoints.as_mut_ptr() as *mut c_void,
                    );
                    text = codepoints.into_iter().filter_map(char::from_u32).collect();
                }
            }
            if !spacer && text.is_empty() {
                text.push(' ');
            }

            let underline_color = if style.underline != 0 {
                style_color(style.underline_color, palette)
            } else {
                None
            };

            Cell {
                text,
                fg,
                bg,
                underline_color,
                attrs: Attrs {
                    bold: style.bold,
                    dim: style.faint,
                    italic: style.italic,
                    underline: style.underline != 0,
                    blink: style.blink,
                    reverse: style.inverse,
                    hidden: style.invisible,
                    strike: style.strikethrough,
                    overline: style.overline,
                },
                width: match wide {
                    sys::GHOSTTY_CELL_WIDE_WIDE => 2,
                    sys::GHOSTTY_CELL_WIDE_SPACER_TAIL | sys::GHOSTTY_CELL_WIDE_SPACER_HEAD => 0,
                    _ => 1,
                },
            }
        }
    }
}

impl Drop for Vt {
    fn drop(&mut self) {
        // The search and selection borrow this terminal; release them first so
        // their tracked state is detached while the terminal is still alive.
        self.search = None;
        self.selection = None;
        // SAFETY: each handle was created in `new` and is freed once, in the
        // reverse order of creation.
        unsafe {
            sys::ghostty_mouse_encoder_free(self.mouse_encoder);
            sys::ghostty_key_encoder_free(self.key_encoder);
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.rows);
            sys::ghostty_render_state_free(self.render);
            sys::ghostty_terminal_free(self.terminal);
        }
    }
}

fn check(result: sys::GhosttyResult, what: &'static str) -> Result<()> {
    if result == sys::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(VtError { what })
    }
}

fn color_of(rgb: sys::GhosttyColorRgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

fn style_color(
    color: sys::GhosttyStyleColor,
    palette: &[sys::GhosttyColorRgb; 256],
) -> Option<Color> {
    match color.tag {
        sys::GHOSTTY_STYLE_COLOR_PALETTE => {
            // SAFETY: the union's palette field is active for this tag.
            let index = unsafe { color.value.palette } as usize;
            palette.get(index).copied().map(color_of)
        }
        sys::GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: the union's rgb field is active for this tag.
            let rgb = unsafe { color.value.rgb };
            Some(color_of(rgb))
        }
        _ => None,
    }
}

fn button_bit(button: Option<sys::GhosttyMouseButton>) -> u8 {
    match button {
        Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT) => 1 << 0,
        Some(sys::GHOSTTY_MOUSE_BUTTON_MIDDLE) => 1 << 1,
        Some(sys::GHOSTTY_MOUSE_BUTTON_RIGHT) => 1 << 2,
        Some(sys::GHOSTTY_MOUSE_BUTTON_FOUR) => 1 << 3,
        Some(sys::GHOSTTY_MOUSE_BUTTON_FIVE) => 1 << 4,
        Some(sys::GHOSTTY_MOUSE_BUTTON_SIX) => 1 << 5,
        Some(sys::GHOSTTY_MOUSE_BUTTON_SEVEN) => 1 << 6,
        _ => 0,
    }
}

/// Runs an encoder twice: once to learn the size, once into a buffer of it.
fn encode_buffer(
    mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> sys::GhosttyResult,
) -> Vec<u8> {
    let mut len = 0usize;
    let result = encode(ptr::null_mut(), 0, &mut len);
    if result == sys::GHOSTTY_SUCCESS {
        return Vec::new();
    }
    if result != sys::GHOSTTY_OUT_OF_SPACE || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u8; len];
    let result = encode(buffer.as_mut_ptr() as *mut c_char, buffer.len(), &mut len);
    if result == sys::GHOSTTY_SUCCESS {
        buffer.truncate(len);
        buffer
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
