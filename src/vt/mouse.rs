//! Crossterm mouse events to Ghostty's mouse encoder.
//!
//! The encoder is configured from the terminal on every event, so the
//! program's tracking mode (X10/normal/button/any) and format (X10, UTF-8,
//! SGR, URXVT, SGR-pixels) decide the bytes. A TUI has no pixel size, so the
//! encoder is told cells are one pixel: the event position is the cell
//! column and row, and Ghostty converts it to its one-based grid coordinate.

use ghostty_sys as sys;

/// What one crossterm mouse event asks the encoder to report.
pub(super) struct Mouse {
    pub action: sys::GhosttyMouseAction,
    pub button: Option<sys::GhosttyMouseButton>,
    pub mods: sys::GhosttyMods,
    pub column: u16,
    pub row: u16,
    /// Set for press/drag and cleared for release, so the caller can keep the
    /// encoder's "any button pressed" option in step.
    pub holds_button: bool,
    pub releases_button: bool,
}

pub(super) fn mouse(event: &crossterm::event::MouseEvent) -> Option<Mouse> {
    use crossterm::event::MouseEventKind as K;
    let mut out = Mouse {
        action: sys::GHOSTTY_MOUSE_ACTION_MOTION,
        button: None,
        mods: mods(event.modifiers),
        column: event.column,
        row: event.row,
        holds_button: false,
        releases_button: false,
    };
    match event.kind {
        K::Down(button) => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_PRESS;
            out.button = Some(button_of(button));
            out.holds_button = true;
        }
        K::Up(button) => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_RELEASE;
            out.button = Some(button_of(button));
            out.releases_button = true;
        }
        K::Drag(button) => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_MOTION;
            out.button = Some(button_of(button));
            out.holds_button = true;
        }
        K::Moved => {}
        // A wheel notch is a press of the wheel "button"; there is no release.
        K::ScrollUp => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_PRESS;
            out.button = Some(sys::GHOSTTY_MOUSE_BUTTON_FOUR);
        }
        K::ScrollDown => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_PRESS;
            out.button = Some(sys::GHOSTTY_MOUSE_BUTTON_FIVE);
        }
        K::ScrollLeft => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_PRESS;
            out.button = Some(sys::GHOSTTY_MOUSE_BUTTON_SIX);
        }
        K::ScrollRight => {
            out.action = sys::GHOSTTY_MOUSE_ACTION_PRESS;
            out.button = Some(sys::GHOSTTY_MOUSE_BUTTON_SEVEN);
        }
    }
    Some(out)
}

fn button_of(button: crossterm::event::MouseButton) -> sys::GhosttyMouseButton {
    match button {
        crossterm::event::MouseButton::Left => sys::GHOSTTY_MOUSE_BUTTON_LEFT,
        crossterm::event::MouseButton::Right => sys::GHOSTTY_MOUSE_BUTTON_RIGHT,
        crossterm::event::MouseButton::Middle => sys::GHOSTTY_MOUSE_BUTTON_MIDDLE,
    }
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
    mods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn event(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn a_click_presses_and_a_release_releases_the_left_button() {
        let down = mouse(&event(MouseEventKind::Down(MouseButton::Left))).expect("mouse event");
        assert_eq!(down.action, sys::GHOSTTY_MOUSE_ACTION_PRESS);
        assert_eq!(down.button, Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT));
        assert!(down.holds_button);
        assert!(!down.releases_button);

        let up = mouse(&event(MouseEventKind::Up(MouseButton::Left))).expect("mouse event");
        assert_eq!(up.action, sys::GHOSTTY_MOUSE_ACTION_RELEASE);
        assert_eq!(up.button, Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT));
        assert!(up.releases_button);
    }

    #[test]
    fn wheel_notches_are_wheel_buttons() {
        assert_eq!(
            mouse(&event(MouseEventKind::ScrollUp))
                .expect("mouse event")
                .button,
            Some(sys::GHOSTTY_MOUSE_BUTTON_FOUR)
        );
        assert_eq!(
            mouse(&event(MouseEventKind::ScrollDown))
                .expect("mouse event")
                .button,
            Some(sys::GHOSTTY_MOUSE_BUTTON_FIVE)
        );
    }

    #[test]
    fn motion_without_a_button_has_no_button() {
        let moved = mouse(&event(MouseEventKind::Moved)).expect("mouse event");
        assert_eq!(moved.action, sys::GHOSTTY_MOUSE_ACTION_MOTION);
        assert_eq!(moved.button, None);
    }
}
