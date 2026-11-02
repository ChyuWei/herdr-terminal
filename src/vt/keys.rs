//! Crossterm key and mouse events to Ghostty's input encoders.
//!
//! The encoder is configured from the terminal state on every event
//! (`ghostty_key_encoder_setopt_from_terminal`), so the program's kitty
//! keyboard flags, cursor-key mode, keypad mode and backarrow mode decide the
//! bytes. This module only translates the event itself; the VT owns the
//! encoder handles.

use ghostty_sys as sys;

/// A crossterm key translated to the encoder's vocabulary.
pub(super) struct Key {
    pub key: sys::GhosttyKey,
    pub mods: sys::GhosttyMods,
    pub action: sys::GhosttyKeyAction,
    /// The unmodified text the key produced, when it produced any.
    pub text: Option<String>,
    /// The unshifted codepoint, for kitty's alternate-key reporting.
    pub unshifted: Option<u32>,
}

pub(super) fn key(event: &crossterm::event::KeyEvent) -> Key {
    let mods = mods(event.modifiers);
    let action = match event.kind {
        crossterm::event::KeyEventKind::Press => sys::GHOSTTY_KEY_ACTION_PRESS,
        crossterm::event::KeyEventKind::Repeat => sys::GHOSTTY_KEY_ACTION_REPEAT,
        crossterm::event::KeyEventKind::Release => sys::GHOSTTY_KEY_ACTION_RELEASE,
    };
    let mut key = Key {
        key: sys::GHOSTTY_KEY_UNIDENTIFIED,
        mods,
        action,
        text: None,
        unshifted: None,
    };
    match event.code {
        crossterm::event::KeyCode::Char(c) => {
            key.key = physical(c);
            // Control characters are not text; the encoder derives them from
            // the logical key and modifiers. Printable characters are.
            if !c.is_control() {
                key.text = Some(c.to_string());
                if c.is_ascii_alphabetic() {
                    key.unshifted = Some(c.to_ascii_lowercase() as u32);
                } else if !event
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT)
                {
                    key.unshifted = Some(c as u32);
                }
            }
        }
        crossterm::event::KeyCode::Enter => key.key = sys::GHOSTTY_KEY_ENTER,
        crossterm::event::KeyCode::Tab => key.key = sys::GHOSTTY_KEY_TAB,
        crossterm::event::KeyCode::BackTab => {
            key.key = sys::GHOSTTY_KEY_TAB;
            key.mods |= sys::GHOSTTY_MODS_SHIFT as sys::GhosttyMods;
        }
        crossterm::event::KeyCode::Backspace => key.key = sys::GHOSTTY_KEY_BACKSPACE,
        crossterm::event::KeyCode::Esc => key.key = sys::GHOSTTY_KEY_ESCAPE,
        crossterm::event::KeyCode::Left => key.key = sys::GHOSTTY_KEY_ARROW_LEFT,
        crossterm::event::KeyCode::Right => key.key = sys::GHOSTTY_KEY_ARROW_RIGHT,
        crossterm::event::KeyCode::Up => key.key = sys::GHOSTTY_KEY_ARROW_UP,
        crossterm::event::KeyCode::Down => key.key = sys::GHOSTTY_KEY_ARROW_DOWN,
        crossterm::event::KeyCode::Home => key.key = sys::GHOSTTY_KEY_HOME,
        crossterm::event::KeyCode::End => key.key = sys::GHOSTTY_KEY_END,
        crossterm::event::KeyCode::PageUp => key.key = sys::GHOSTTY_KEY_PAGE_UP,
        crossterm::event::KeyCode::PageDown => key.key = sys::GHOSTTY_KEY_PAGE_DOWN,
        crossterm::event::KeyCode::Insert => key.key = sys::GHOSTTY_KEY_INSERT,
        crossterm::event::KeyCode::Delete => key.key = sys::GHOSTTY_KEY_DELETE,
        crossterm::event::KeyCode::F(n) => key.key = function(n),
        crossterm::event::KeyCode::CapsLock => key.key = sys::GHOSTTY_KEY_CAPS_LOCK,
        crossterm::event::KeyCode::ScrollLock => key.key = sys::GHOSTTY_KEY_SCROLL_LOCK,
        crossterm::event::KeyCode::NumLock => key.key = sys::GHOSTTY_KEY_NUM_LOCK,
        crossterm::event::KeyCode::PrintScreen => key.key = sys::GHOSTTY_KEY_PRINT_SCREEN,
        crossterm::event::KeyCode::Pause => key.key = sys::GHOSTTY_KEY_PAUSE,
        crossterm::event::KeyCode::Menu => key.key = sys::GHOSTTY_KEY_CONTEXT_MENU,
        crossterm::event::KeyCode::KeypadBegin => key.key = sys::GHOSTTY_KEY_NUMPAD_BEGIN,
        crossterm::event::KeyCode::Media(media) => key.key = media_key(media),
        crossterm::event::KeyCode::Modifier(modifier) => {
            let (keycode, side) = modifier_key(modifier);
            key.key = keycode;
            key.mods |= side;
        }
        crossterm::event::KeyCode::Null => {}
    }
    key
}

fn mods(modifiers: crossterm::event::KeyModifiers) -> sys::GhosttyMods {
    use crossterm::event::KeyModifiers as M;
    let mut mods = 0;
    if modifiers.contains(M::SHIFT) {
        mods |= sys::GHOSTTY_MODS_SHIFT as sys::GhosttyMods;
    }
    if modifiers.contains(M::CONTROL) {
        mods |= sys::GHOSTTY_MODS_CTRL as sys::GhosttyMods;
    }
    if modifiers.contains(M::ALT) {
        mods |= sys::GHOSTTY_MODS_ALT as sys::GhosttyMods;
    }
    // Ghostty has one super/meta bit; both crossterm modifiers mean "the
    // platform key next to space" on the platforms that report them.
    if modifiers.intersects(M::SUPER | M::META) {
        mods |= sys::GHOSTTY_MODS_SUPER as sys::GhosttyMods;
    }
    mods
}

/// The physical key for a character, as far as a US layout can tell. The
/// encoder uses this for legacy control sequences and kitty alternate keys;
/// the text itself is passed separately.
fn physical(c: char) -> sys::GhosttyKey {
    use sys::*;
    match c.to_ascii_lowercase() {
        'a' => GHOSTTY_KEY_A,
        'b' => GHOSTTY_KEY_B,
        'c' => GHOSTTY_KEY_C,
        'd' => GHOSTTY_KEY_D,
        'e' => GHOSTTY_KEY_E,
        'f' => GHOSTTY_KEY_F,
        'g' => GHOSTTY_KEY_G,
        'h' => GHOSTTY_KEY_H,
        'i' => GHOSTTY_KEY_I,
        'j' => GHOSTTY_KEY_J,
        'k' => GHOSTTY_KEY_K,
        'l' => GHOSTTY_KEY_L,
        'm' => GHOSTTY_KEY_M,
        'n' => GHOSTTY_KEY_N,
        'o' => GHOSTTY_KEY_O,
        'p' => GHOSTTY_KEY_P,
        'q' => GHOSTTY_KEY_Q,
        'r' => GHOSTTY_KEY_R,
        's' => GHOSTTY_KEY_S,
        't' => GHOSTTY_KEY_T,
        'u' => GHOSTTY_KEY_U,
        'v' => GHOSTTY_KEY_V,
        'w' => GHOSTTY_KEY_W,
        'x' => GHOSTTY_KEY_X,
        'y' => GHOSTTY_KEY_Y,
        'z' => GHOSTTY_KEY_Z,
        '0' => GHOSTTY_KEY_DIGIT_0,
        '1' => GHOSTTY_KEY_DIGIT_1,
        '2' => GHOSTTY_KEY_DIGIT_2,
        '3' => GHOSTTY_KEY_DIGIT_3,
        '4' => GHOSTTY_KEY_DIGIT_4,
        '5' => GHOSTTY_KEY_DIGIT_5,
        '6' => GHOSTTY_KEY_DIGIT_6,
        '7' => GHOSTTY_KEY_DIGIT_7,
        '8' => GHOSTTY_KEY_DIGIT_8,
        '9' => GHOSTTY_KEY_DIGIT_9,
        ' ' => GHOSTTY_KEY_SPACE,
        '`' | '~' => GHOSTTY_KEY_BACKQUOTE,
        '\\' | '|' => GHOSTTY_KEY_BACKSLASH,
        '[' | '{' => GHOSTTY_KEY_BRACKET_LEFT,
        ']' | '}' => GHOSTTY_KEY_BRACKET_RIGHT,
        ',' | '<' => GHOSTTY_KEY_COMMA,
        '=' | '+' => GHOSTTY_KEY_EQUAL,
        '-' | '_' => GHOSTTY_KEY_MINUS,
        '.' | '>' => GHOSTTY_KEY_PERIOD,
        '\'' | '"' => GHOSTTY_KEY_QUOTE,
        ';' | ':' => GHOSTTY_KEY_SEMICOLON,
        '/' | '?' => GHOSTTY_KEY_SLASH,
        '!' => GHOSTTY_KEY_DIGIT_1,
        '@' => GHOSTTY_KEY_DIGIT_2,
        '#' => GHOSTTY_KEY_DIGIT_3,
        '$' => GHOSTTY_KEY_DIGIT_4,
        '%' => GHOSTTY_KEY_DIGIT_5,
        '^' => GHOSTTY_KEY_DIGIT_6,
        '&' => GHOSTTY_KEY_DIGIT_7,
        '*' => GHOSTTY_KEY_DIGIT_8,
        '(' => GHOSTTY_KEY_DIGIT_9,
        ')' => GHOSTTY_KEY_DIGIT_0,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn function(n: u8) -> sys::GhosttyKey {
    use sys::*;
    match n {
        1 => GHOSTTY_KEY_F1,
        2 => GHOSTTY_KEY_F2,
        3 => GHOSTTY_KEY_F3,
        4 => GHOSTTY_KEY_F4,
        5 => GHOSTTY_KEY_F5,
        6 => GHOSTTY_KEY_F6,
        7 => GHOSTTY_KEY_F7,
        8 => GHOSTTY_KEY_F8,
        9 => GHOSTTY_KEY_F9,
        10 => GHOSTTY_KEY_F10,
        11 => GHOSTTY_KEY_F11,
        12 => GHOSTTY_KEY_F12,
        13 => GHOSTTY_KEY_F13,
        14 => GHOSTTY_KEY_F14,
        15 => GHOSTTY_KEY_F15,
        16 => GHOSTTY_KEY_F16,
        17 => GHOSTTY_KEY_F17,
        18 => GHOSTTY_KEY_F18,
        19 => GHOSTTY_KEY_F19,
        20 => GHOSTTY_KEY_F20,
        21 => GHOSTTY_KEY_F21,
        22 => GHOSTTY_KEY_F22,
        23 => GHOSTTY_KEY_F23,
        24 => GHOSTTY_KEY_F24,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn media_key(media: crossterm::event::MediaKeyCode) -> sys::GhosttyKey {
    use crossterm::event::MediaKeyCode as M;
    use sys::*;
    match media {
        M::Play | M::Pause | M::PlayPause => GHOSTTY_KEY_MEDIA_PLAY_PAUSE,
        M::Stop => GHOSTTY_KEY_MEDIA_STOP,
        M::TrackNext => GHOSTTY_KEY_MEDIA_TRACK_NEXT,
        M::TrackPrevious => GHOSTTY_KEY_MEDIA_TRACK_PREVIOUS,
        M::LowerVolume => GHOSTTY_KEY_AUDIO_VOLUME_DOWN,
        M::RaiseVolume => GHOSTTY_KEY_AUDIO_VOLUME_UP,
        M::MuteVolume => GHOSTTY_KEY_AUDIO_VOLUME_MUTE,
        M::Reverse | M::FastForward | M::Rewind | M::Record => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// A modifier key and the side bit the encoder expects.
fn modifier_key(
    modifier: crossterm::event::ModifierKeyCode,
) -> (sys::GhosttyKey, sys::GhosttyMods) {
    use crossterm::event::ModifierKeyCode as M;
    use sys::*;
    match modifier {
        M::LeftShift => (GHOSTTY_KEY_SHIFT_LEFT, GHOSTTY_MODS_SHIFT as GhosttyMods),
        M::RightShift => (
            GHOSTTY_KEY_SHIFT_RIGHT,
            (GHOSTTY_MODS_SHIFT | GHOSTTY_MODS_SHIFT_SIDE) as GhosttyMods,
        ),
        M::LeftControl => (GHOSTTY_KEY_CONTROL_LEFT, GHOSTTY_MODS_CTRL as GhosttyMods),
        M::RightControl => (
            GHOSTTY_KEY_CONTROL_RIGHT,
            (GHOSTTY_MODS_CTRL | GHOSTTY_MODS_CTRL_SIDE) as GhosttyMods,
        ),
        M::LeftAlt => (GHOSTTY_KEY_ALT_LEFT, GHOSTTY_MODS_ALT as GhosttyMods),
        M::RightAlt => (
            GHOSTTY_KEY_ALT_RIGHT,
            (GHOSTTY_MODS_ALT | GHOSTTY_MODS_ALT_SIDE) as GhosttyMods,
        ),
        M::LeftSuper => (GHOSTTY_KEY_META_LEFT, GHOSTTY_MODS_SUPER as GhosttyMods),
        M::RightSuper => (
            GHOSTTY_KEY_META_RIGHT,
            (GHOSTTY_MODS_SUPER | GHOSTTY_MODS_SUPER_SIDE) as GhosttyMods,
        ),
        M::LeftHyper
        | M::RightHyper
        | M::LeftMeta
        | M::RightMeta
        | M::IsoLevel3Shift
        | M::IsoLevel5Shift => (GHOSTTY_KEY_UNIDENTIFIED, 0),
    }
}

/// Whether this is the screen's detach key: Ctrl-].
///
/// A terminal that speaks the kitty keyboard protocol sends Ctrl-] as a
/// CSI-u key, which crossterm reports as `Char(']')` with CONTROL. Without
/// that protocol the key is the single byte 0x1d, which crossterm's legacy
/// parser reports as `Char('5')` with CONTROL (0x1c..0x1f map to Ctrl-4..7).
/// Both are the same key on the wire, so both detach.
pub fn is_detach(event: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.kind != crossterm::event::KeyEventKind::Release
        && event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            event.code,
            KeyCode::Char(']') | KeyCode::Char('5') | KeyCode::Char('\u{1d}')
        )
}

/// Windows' win32-input-mode wants Escape and modified Enter as
/// `CSI Vk;Sc;Uc;Kd;Cs;Rc _`, which Ghostty's encoder does not produce.
/// The caller only uses this while the program asked for win32 input and is
/// not using the kitty keyboard protocol.
pub(super) fn win32(event: &crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers as M};
    let shift = event.modifiers.contains(M::SHIFT);
    let ctrl = event.modifiers.contains(M::CONTROL);
    let alt = event.modifiers.contains(M::ALT);
    let down = match event.kind {
        KeyEventKind::Press | KeyEventKind::Repeat => 1,
        KeyEventKind::Release => 0,
    };
    let (vk, scan, typed) = match event.code {
        KeyCode::Esc => (27, 1, 27),
        KeyCode::Enter if event.modifiers.intersects(M::SHIFT | M::CONTROL | M::ALT) => {
            (13, 28, if ctrl { 10 } else { 13 })
        }
        _ => return None,
    };
    // Control-key state bits are Windows': Shift 0x10, left Ctrl 0x08,
    // left Alt 0x02.
    let held = ((shift as u32) * 0x10) | ((ctrl as u32) * 0x08) | ((alt as u32) * 0x02);
    Some(format!("\x1b[{vk};{scan};{typed};{down};{held};1_").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn a_letter_carries_its_text_and_physical_key() {
        let key = key(&press(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(key.key, sys::GHOSTTY_KEY_A);
        assert_eq!(key.text.as_deref(), Some("a"));
        assert_eq!(key.unshifted, Some('a' as u32));
        assert_eq!(key.action, sys::GHOSTTY_KEY_ACTION_PRESS);
    }

    #[test]
    fn a_shifted_letter_reports_the_unshifted_codepoint() {
        let key = key(&press(KeyCode::Char('A'), KeyModifiers::SHIFT));
        assert_eq!(key.key, sys::GHOSTTY_KEY_A);
        assert_eq!(key.text.as_deref(), Some("A"));
        assert_eq!(key.unshifted, Some('a' as u32));
        assert_eq!(key.mods, sys::GHOSTTY_MODS_SHIFT as sys::GhosttyMods);
    }

    #[test]
    fn shifted_punctuation_maps_to_its_number_key() {
        let key = key(&press(KeyCode::Char('!'), KeyModifiers::SHIFT));
        assert_eq!(key.key, sys::GHOSTTY_KEY_DIGIT_1);
        assert_eq!(
            key.unshifted, None,
            "an unknown unshifted form is left unset"
        );
    }

    #[test]
    fn control_characters_are_not_passed_as_text() {
        let key = key(&press(KeyCode::Char('\u{1}'), KeyModifiers::CONTROL));
        assert_eq!(key.text, None);
        assert_eq!(key.key, sys::GHOSTTY_KEY_UNIDENTIFIED);
    }

    #[test]
    fn enter_and_escape_are_logical_keys_without_text() {
        let enter = key(&press(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(enter.key, sys::GHOSTTY_KEY_ENTER);
        assert_eq!(enter.text, None);
        let escape = key(&press(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(escape.key, sys::GHOSTTY_KEY_ESCAPE);
    }

    #[test]
    fn backtab_is_shift_tab() {
        let key = key(&press(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(key.key, sys::GHOSTTY_KEY_TAB);
        assert_eq!(key.mods, sys::GHOSTTY_MODS_SHIFT as sys::GhosttyMods);
    }

    #[test]
    fn key_kinds_map_to_actions() {
        let mut event = press(KeyCode::Char('x'), KeyModifiers::NONE);
        event.kind = crossterm::event::KeyEventKind::Repeat;
        assert_eq!(key(&event).action, sys::GHOSTTY_KEY_ACTION_REPEAT);
        event.kind = crossterm::event::KeyEventKind::Release;
        assert_eq!(key(&event).action, sys::GHOSTTY_KEY_ACTION_RELEASE);
    }

    #[test]
    fn ctrl_close_bracket_is_the_detach_key() {
        assert!(is_detach(&press(KeyCode::Char(']'), KeyModifiers::CONTROL)));
        // The legacy 0x1d byte arrives as Ctrl-5 from crossterm.
        assert!(is_detach(&press(KeyCode::Char('5'), KeyModifiers::CONTROL)));
        assert!(is_detach(&press(
            KeyCode::Char('\u{1d}'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_detach(&press(KeyCode::Char(']'), KeyModifiers::NONE)));
        assert!(!is_detach(&press(KeyCode::Char('5'), KeyModifiers::NONE)));
        assert!(!is_detach(&press(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn win32_encodes_escape_and_modified_enter_only() {
        assert_eq!(
            win32(&press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(b"\x1b[27;1;27;1;0;1_".to_vec())
        );
        assert_eq!(
            win32(&press(KeyCode::Enter, KeyModifiers::CONTROL)),
            Some(b"\x1b[13;28;10;1;8;1_".to_vec())
        );
        assert_eq!(win32(&press(KeyCode::Enter, KeyModifiers::NONE)), None);
        assert_eq!(win32(&press(KeyCode::Char('a'), KeyModifiers::NONE)), None);
    }
}
