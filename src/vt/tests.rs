//! Unit and golden tests for the VT wrapper.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn screen(bytes: &[u8], cols: u16, rows: u16) -> Screen {
    let mut vt = Vt::new(cols, rows).expect("VT");
    vt.feed(bytes);
    vt.snapshot().expect("snapshot")
}

#[test]
fn plain_text_lands_on_the_grid_and_moves_the_cursor() {
    let screen = screen(b"hello", 10, 3);
    assert_eq!(screen.cols, 10);
    assert_eq!(screen.rows, 3);
    assert_eq!(screen.lines(), ["hello", "", ""]);
    assert_eq!(screen.cursor.x, 5);
    assert_eq!(screen.cursor.y, 0);
    assert!(screen.cursor.visible);
    assert_eq!(screen.cell(5, 0).expect("cell").text, " ");
}

#[test]
fn colors_and_attributes_are_read_from_the_cell_style() {
    let screen = screen(
        b"\x1b[1;31mA\x1b[0m\x1b[3;4;9mB\x1b[0m\x1b[38;2;1;2;3mC\x1b[48;5;196mD",
        10,
        2,
    );
    let a = screen.cell(0, 0).expect("A");
    assert_eq!(a.text, "A");
    assert!(a.attrs.bold);
    // Ghostty's default palette red (index 1).
    assert_eq!(a.fg, Color::Rgb(0xCC, 0x66, 0x66));
    assert_eq!(a.bg, Color::Default);

    let b = screen.cell(1, 0).expect("B");
    assert!(b.attrs.italic);
    assert!(b.attrs.underline);
    assert!(b.attrs.strike);
    assert!(!b.attrs.bold);

    let c = screen.cell(2, 0).expect("C");
    assert_eq!(c.fg, Color::Rgb(1, 2, 3));

    let d = screen.cell(3, 0).expect("D");
    // Palette index 196 is the cube's pure red.
    assert_eq!(d.bg, Color::Rgb(255, 0, 0));
    assert_eq!(
        d.fg,
        Color::Rgb(1, 2, 3),
        "the previous SGR fg still applies"
    );
}

#[test]
fn a_wide_character_owns_two_cells_and_its_spacer_is_marked() {
    let screen = screen("你a".as_bytes(), 6, 2);
    let wide = screen.cell(0, 0).expect("wide");
    assert_eq!(wide.text, "你");
    assert_eq!(wide.width, 2);
    let spacer = screen.cell(1, 0).expect("spacer");
    assert_eq!(spacer.width, 0);
    assert_eq!(spacer.text, "", "the spacer is not drawn");
    assert_eq!(screen.cell(2, 0).expect("a").text, "a");
    assert_eq!(screen.cursor.x, 3);
}

#[test]
fn a_grapheme_cluster_stays_in_one_cell() {
    // e followed by COMBINING ACUTE ACCENT is one user-perceived character.
    let screen = screen("e\u{301}".as_bytes(), 4, 2);
    let cell = screen.cell(0, 0).expect("cell");
    assert_eq!(cell.text, "e\u{301}");
    assert_eq!(cell.width, 1);
    assert_eq!(screen.cursor.x, 1, "the combining mark adds no cell");
}

#[test]
fn an_emoji_is_two_cells_wide() {
    let screen = screen("😀".as_bytes(), 4, 2);
    assert_eq!(screen.cell(0, 0).expect("emoji").width, 2);
    assert_eq!(screen.cell(1, 0).expect("spacer").width, 0);
    assert_eq!(screen.cursor.x, 2);
}

#[test]
fn a_variation_selector_makes_a_text_emoji_two_cells_wide() {
    let mut vt = Vt::new(6, 2).expect("VT");
    // U+270D is narrow on its own; VS16 gives it emoji presentation.
    vt.feed("✍️".as_bytes());
    let screen = vt.snapshot().expect("snapshot");
    let sign = screen.cell(0, 0).expect("cell");
    assert_eq!(sign.text, "✍️");
    assert_eq!(sign.width, 2);
    assert_eq!(screen.cell(1, 0).expect("spacer").width, 0);
    assert_eq!(screen.cursor.x, 2);

    // RIS restores the configured default, not Ghostty's built-in one.
    vt.feed(b"\x1bc");
    assert!(vt.mode(2027), "grapheme clustering is the reset default");
    vt.feed("❤️".as_bytes());
    let screen = vt.snapshot().expect("snapshot after RIS");
    let heart = screen.cell(0, 0).expect("heart");
    assert_eq!(heart.text, "❤️");
    assert_eq!(heart.width, 2);
    assert_eq!(screen.cell(1, 0).expect("spacer").width, 0);
    assert_eq!(screen.cursor.x, 2);
}

#[test]
fn resize_changes_the_grid_and_keeps_the_cursor_inside_it() {
    let mut vt = Vt::new(10, 3).expect("VT");
    vt.feed(b"abcdef");
    vt.resize(4, 2).expect("resize");
    let screen = vt.snapshot().expect("snapshot");
    assert_eq!((screen.cols, screen.rows), (4, 2));
    assert_eq!(screen.cells.len(), 8);
    assert!(screen.cursor.x < 4 && screen.cursor.y < 2);
}

#[test]
fn the_title_win32_mode_and_mouse_tracking_are_visible() {
    let mut vt = Vt::new(10, 2).expect("VT");
    vt.feed(b"\x1b]2;my title\x07");
    assert_eq!(vt.title(), "my title");
    assert!(!vt.win32_input());
    vt.feed(b"\x1b[?9001h");
    assert!(vt.win32_input());
    vt.feed(b"\x1b[?9001l");
    assert!(!vt.win32_input());
    assert!(!vt.mouse_tracking());
    vt.feed(b"\x1b[?1000h");
    assert!(vt.mouse_tracking());
    assert_eq!(vt.kitty_keyboard(), 0);
}

#[test]
fn win32_mode_requests_split_across_feeds_are_seen_in_order() {
    let mut vt = Vt::new(10, 2).expect("VT");
    // ON split over two feeds.
    vt.feed(b"\x1b[?90");
    assert!(!vt.win32_input());
    vt.feed(b"01h");
    assert!(vt.win32_input());
    // OFF then ON across the seam: the later request wins.
    vt.feed(b"\x1b[?9001");
    vt.feed(b"l\x1b[?9001h");
    assert!(vt.win32_input());
    // ON then OFF across the seam.
    vt.feed(b"\x1b[?9001h\x1b[?9001");
    vt.feed(b"l");
    assert!(!vt.win32_input());
}

#[test]
fn win32_mode_is_ignored_inside_string_payloads() {
    let mut vt = Vt::new(10, 2).expect("VT");
    // A title (OSC) that contains the request bytes must not enable the mode.
    vt.feed(b"\x1b]0;\x1b[?9001h\x07");
    assert!(!vt.win32_input());
    assert_eq!(
        vt.encode_key(&key(KeyCode::Esc, KeyModifiers::NONE)),
        b"\x1b",
        "Escape must use the normal encoding when win32 mode was not requested"
    );
    // DCS and APC payloads are skipped the same way, including ST endings.
    vt.feed(b"\x1bP\x1b[?9001h\x1b\\");
    assert!(!vt.win32_input());
    vt.feed(b"\x1b_\x1b[?9001h\x1b\\");
    assert!(!vt.win32_input());
    // A real request in ground state still works after a string.
    vt.feed(b"\x1b[?9001h");
    assert!(vt.win32_input());
    assert_eq!(
        vt.encode_key(&key(KeyCode::Esc, KeyModifiers::NONE)),
        b"\x1b[27;1;27;1;0;1_"
    );
}

#[test]
fn the_cursor_can_be_hidden_and_shaped() {
    let mut vt = Vt::new(10, 2).expect("VT");
    vt.feed(b"\x1b[?25l");
    assert!(!vt.snapshot().expect("snapshot").cursor.visible);
    vt.feed(b"\x1b[?25h\x1b[4 q");
    let cursor = vt.snapshot().expect("snapshot").cursor;
    assert!(cursor.visible);
    assert_eq!(cursor.shape, CursorShape::Underline);
}

#[test]
fn legacy_keys_encode_to_what_a_program_expects() {
    let mut vt = Vt::new(10, 2).expect("VT");
    assert_eq!(
        vt.encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE)),
        b"a"
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        [0x03]
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Enter, KeyModifiers::NONE)),
        b"\r"
    );
    assert_eq!(vt.encode_key(&key(KeyCode::Tab, KeyModifiers::NONE)), b"\t");
    assert_eq!(
        vt.encode_key(&key(KeyCode::Backspace, KeyModifiers::NONE)),
        [0x7f]
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Up, KeyModifiers::NONE)),
        b"\x1b[A"
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Left, KeyModifiers::NONE)),
        b"\x1b[D"
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Esc, KeyModifiers::NONE)),
        [0x1b]
    );
}

#[test]
fn kitty_keyboard_flags_change_the_encoding() {
    let mut vt = Vt::new(10, 2).expect("VT");
    // Disambiguate escape codes.
    vt.feed(b"\x1b[>1u");
    assert_eq!(
        vt.encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        b"\x1b[99;5u"
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE)),
        b"a"
    );
    // Disambiguate + report event types: releases are reported too.
    vt.feed(b"\x1b[>3u");
    let mut release = key(KeyCode::Char('a'), KeyModifiers::NONE);
    release.kind = crossterm::event::KeyEventKind::Release;
    let bytes = vt.encode_key(&release);
    assert!(
        bytes.starts_with(b"\x1b[97;1:3u") || bytes.ends_with(b":3u"),
        "release encoding was {bytes:?}"
    );
}

#[test]
fn win32_input_mode_takes_escape_and_modified_enter() {
    let mut vt = Vt::new(10, 2).expect("VT");
    vt.feed(b"\x1b[?9001h");
    assert_eq!(
        vt.encode_key(&key(KeyCode::Esc, KeyModifiers::NONE)),
        b"\x1b[27;1;27;1;0;1_"
    );
    assert_eq!(
        vt.encode_key(&key(KeyCode::Enter, KeyModifiers::CONTROL)),
        b"\x1b[13;28;10;1;8;1_"
    );
    // An unmodified Enter still goes through Ghostty's encoder.
    assert_eq!(
        vt.encode_key(&key(KeyCode::Enter, KeyModifiers::NONE)),
        b"\r"
    );
    // Kitty input wins when the program negotiated it.
    vt.feed(b"\x1b[>1u");
    assert_eq!(
        vt.encode_key(&key(KeyCode::Esc, KeyModifiers::NONE)),
        b"\x1b[27u"
    );
}

#[test]
fn mouse_events_are_encoded_only_while_tracking_is_on() {
    let mut vt = Vt::new(10, 4).expect("VT");
    let click = mouse(MouseEventKind::Down(MouseButton::Left), 3, 2);
    assert_eq!(vt.encode_mouse(&click), Vec::<u8>::new());
    vt.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(vt.encode_mouse(&click), b"\x1b[<0;4;3M");
    let release = mouse(MouseEventKind::Up(MouseButton::Left), 3, 2);
    assert_eq!(vt.encode_mouse(&release), b"\x1b[<0;4;3m");
    let wheel = mouse(MouseEventKind::ScrollUp, 3, 2);
    assert_eq!(vt.encode_mouse(&wheel), b"\x1b[<64;4;3M");
}

#[test]
fn mouse_without_sgr_uses_the_legacy_format() {
    let mut vt = Vt::new(10, 4).expect("VT");
    vt.feed(b"\x1b[?1000h");
    let click = mouse(MouseEventKind::Down(MouseButton::Left), 3, 2);
    assert_eq!(vt.encode_mouse(&click), b"\x1b[M $#");
}

#[test]
fn a_golden_frame_has_text_styles_widths_and_a_cursor() {
    let screen = screen(
        b"\x1b[1;31mRed\x1b[0m plain\r\n\xe4\xbd\xa0\xe5\xa5\xbd \xc3\xa9\r\n\x1b[44mblue bg\x1b[0m",
        12,
        3,
    );
    assert_eq!(screen.lines(), ["Red plain", "你好 é", "blue bg"]);
    assert_eq!((screen.cursor.x, screen.cursor.y), (7, 2));

    let red = screen.cell(0, 0).expect("R");
    assert!(red.attrs.bold);
    assert_eq!(red.fg, Color::Rgb(0xCC, 0x66, 0x66));

    assert_eq!(screen.cell(0, 1).expect("你").width, 2);
    assert_eq!(screen.cell(1, 1).expect("spacer").width, 0);
    assert_eq!(screen.cell(2, 1).expect("好").text, "好");
    assert_eq!(screen.cell(3, 1).expect("spacer").width, 0);
    assert_eq!(screen.cell(5, 1).expect("é").text, "é");

    // Palette index 4 is Ghostty's blue.
    assert_eq!(
        screen.cell(0, 2).expect("blue").bg,
        Color::Rgb(0x81, 0xA2, 0xBE)
    );
}

#[test]
fn a_large_output_burst_is_processed_and_bounded() {
    use std::time::{Duration, Instant};
    // Four MiB through the VT, in the same 64 KiB chunks the drain uses.
    let line = "0123456789abcdefghijklmnopqrstuvwxyz\r\n";
    let mut burst = Vec::with_capacity(4 * 1024 * 1024 + line.len());
    while burst.len() < 4 * 1024 * 1024 {
        burst.extend_from_slice(line.as_bytes());
    }
    let mut vt = Vt::new_with_scrollback(120, 40, 10_000).expect("VT");
    let started = Instant::now();
    for chunk in burst.chunks(64 * 1024) {
        vt.feed(chunk);
    }
    let screen = vt.snapshot().expect("snapshot");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "4 MiB took {elapsed:?}; the local VT must stay interactive"
    );
    assert_eq!(screen.lines().len(), 40);
    assert!(screen.lines().join("\n").contains("0123456789abcdef"));
    assert!(
        vt.scrollback_rows() <= 10_000 + 1_024,
        "history is bounded: {} rows",
        vt.scrollback_rows()
    );
    assert!(
        vt.snapshot().expect("second snapshot").lines().len() == 40,
        "a redraw after the burst still produces a full frame"
    );
}
