//! UI themes and color-depth mapping.
//!
//! Terminal cells keep their own colors; a theme styles only the chrome:
//! tab bar, pane borders, status bar, sidebar, peek, help, palette, search and
//! local selection. The `default` theme leaves the UI in the outer terminal's
//! own palette (the earlier look). `dark` and `light` pick explicit colors and
//! map them down for 16-color and 256-color terminals.

use ratatui::style::{Color, Modifier, Style};

/// The theme name from config.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    Default,
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn parse(value: &str) -> Option<ThemeChoice> {
        match value {
            "default" => Some(ThemeChoice::Default),
            "dark" => Some(ThemeChoice::Dark),
            "light" => Some(ThemeChoice::Light),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Default => "default",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        }
    }
}

/// What the outer terminal can display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    #[default]
    Ansi256,
    Ansi16,
}

impl ColorDepth {
    /// COLORTEM/TERM decide the depth, with conservative fallbacks.
    pub fn detect() -> ColorDepth {
        depth_from(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }
}

/// The pure depth rule, so its quirks are testable without touching the env.
pub fn depth_from(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
    let colorterm = colorterm.unwrap_or_default().to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorDepth::TrueColor;
    }
    if term
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("256color")
    {
        return ColorDepth::Ansi256;
    }
    ColorDepth::Ansi16
}

/// Chrome colors for one theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub border: Color,
    pub overlay: Color,
    pub peek: Color,
    pub highlight: Color,
    pub selection: Color,
    pub status: Style,
    pub tab_active: Style,
    pub tab_inactive: Style,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            accent: Color::Cyan,
            border: Color::DarkGray,
            overlay: Color::Cyan,
            peek: Color::Magenta,
            highlight: Color::Yellow,
            selection: Color::DarkGray,
            status: Style::default().add_modifier(Modifier::REVERSED),
            tab_active: Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            tab_inactive: Style::default().add_modifier(Modifier::DIM),
        }
    }
}

/// Builds the theme for a choice and terminal depth.
pub fn theme(choice: ThemeChoice, depth: ColorDepth) -> Theme {
    match choice {
        ThemeChoice::Default => Theme::default(),
        ThemeChoice::Dark => map(
            Theme {
                accent: Color::Rgb(0x7a, 0xa2, 0xf7),
                border: Color::Rgb(0x45, 0x47, 0x5a),
                overlay: Color::Rgb(0xbb, 0x9a, 0xf7),
                peek: Color::Rgb(0xf5, 0xa9, 0x7f),
                highlight: Color::Rgb(0xf9, 0xe2, 0xaf),
                selection: Color::Rgb(0x3b, 0x42, 0x61),
                status: Style::default()
                    .fg(Color::Rgb(0xcd, 0xd6, 0xf4))
                    .bg(Color::Rgb(0x1e, 0x1e, 0x2e)),
                tab_active: Style::default()
                    .fg(Color::Rgb(0x7a, 0xa2, 0xf7))
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
                tab_inactive: Style::default()
                    .fg(Color::Rgb(0x45, 0x47, 0x5a))
                    .add_modifier(Modifier::DIM),
            },
            depth,
        ),
        ThemeChoice::Light => map(
            Theme {
                accent: Color::Rgb(0x1e, 0x66, 0xf5),
                border: Color::Rgb(0x9c, 0xa3, 0xaf),
                overlay: Color::Rgb(0x88, 0x3c, 0x9a),
                peek: Color::Rgb(0xb0, 0x4a, 0x1e),
                highlight: Color::Rgb(0x91, 0x54, 0x00),
                selection: Color::Rgb(0xcc, 0xd0, 0xe0),
                status: Style::default()
                    .fg(Color::Rgb(0x1f, 0x23, 0x28))
                    .bg(Color::Rgb(0xe5, 0xe9, 0xef)),
                tab_active: Style::default()
                    .fg(Color::Rgb(0x1e, 0x66, 0xf5))
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
                tab_inactive: Style::default()
                    .fg(Color::Rgb(0x9c, 0xa3, 0xaf))
                    .add_modifier(Modifier::DIM),
            },
            depth,
        ),
    }
}

fn map(mut theme: Theme, depth: ColorDepth) -> Theme {
    theme.accent = convert(theme.accent, depth);
    theme.border = convert(theme.border, depth);
    theme.overlay = convert(theme.overlay, depth);
    theme.peek = convert(theme.peek, depth);
    theme.highlight = convert(theme.highlight, depth);
    theme.selection = convert(theme.selection, depth);
    if let Some(fg) = theme.status.fg {
        theme.status.fg = Some(convert(fg, depth));
    }
    if let Some(bg) = theme.status.bg {
        theme.status.bg = Some(convert(bg, depth));
    }
    theme.tab_active = map_style(theme.tab_active, depth);
    theme.tab_inactive = map_style(theme.tab_inactive, depth);
    theme
}

fn map_style(mut style: Style, depth: ColorDepth) -> Style {
    if let Some(fg) = style.fg {
        style.fg = Some(convert(fg, depth));
    }
    if let Some(bg) = style.bg {
        style.bg = Some(convert(bg, depth));
    }
    style
}

/// Converts one explicit color down to what the outer terminal supports.
/// Named colors and `Reset` are already portable and pass through.
pub fn convert(color: Color, depth: ColorDepth) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    match depth {
        ColorDepth::TrueColor => color,
        ColorDepth::Ansi256 => Color::Indexed(nearest_256(r, g, b)),
        ColorDepth::Ansi16 => nearest_16(r, g, b),
    }
}

fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    // The 256-color cube plus its grayscale ramp. Search all of them; a
    // linear scan of 240 entries is trivial and avoids edge-case tables.
    let mut best = 16u8;
    let mut best_distance = u32::MAX;
    for index in 16u8..=255 {
        let candidate = xterm(index);
        let distance = distance((r, g, b), candidate);
        if distance < best_distance {
            best_distance = distance;
            best = index;
        }
    }
    best
}

fn xterm(index: u8) -> (u8, u8, u8) {
    if index >= 232 {
        let level = 8 + 10 * (index - 232);
        return (level, level, level);
    }
    let levels = [0, 95, 135, 175, 215, 255];
    let index = index - 16;
    (
        levels[(index / 36) as usize],
        levels[((index % 36) / 6) as usize],
        levels[(index % 6) as usize],
    )
}

fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    // Standard xterm-16 RGB values, paired with ratatui's names.
    const PALETTE: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (205, 0, 0)),
        (Color::Green, (0, 205, 0)),
        (Color::Yellow, (205, 205, 0)),
        (Color::Blue, (0, 0, 238)),
        (Color::Magenta, (205, 0, 205)),
        (Color::Cyan, (0, 205, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::DarkGray, (127, 127, 127)),
        (Color::LightRed, (255, 0, 0)),
        (Color::LightGreen, (0, 255, 0)),
        (Color::LightYellow, (255, 255, 0)),
        (Color::LightBlue, (92, 92, 255)),
        (Color::LightMagenta, (255, 0, 255)),
        (Color::LightCyan, (0, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    PALETTE
        .iter()
        .min_by_key(|(_, rgb)| distance((r, g, b), *rgb))
        .map(|(color, _)| *color)
        .unwrap_or(Color::Reset)
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = i32::from(a.0) - i32::from(b.0);
    let dg = i32::from(a.1) - i32::from(b.1);
    let db = i32::from(a.2) - i32::from(b.2);
    (dr * dr + dg * dg + db * db) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_theme_keeps_the_original_named_palette() {
        let dark = theme(ThemeChoice::Default, ColorDepth::Ansi16);
        assert_eq!(dark.accent, Color::Cyan);
        assert_eq!(dark.border, Color::DarkGray);
        assert_eq!(dark.peek, Color::Magenta);
        assert!(dark.status.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn depth_detection_prefers_truecolor_then_256_then_16() {
        assert_eq!(
            depth_from(Some("truecolor"), Some("xterm-256color")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            depth_from(Some("24bit"), Some("xterm")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            depth_from(None, Some("screen-256color")),
            ColorDepth::Ansi256
        );
        assert_eq!(depth_from(None, Some("xterm")), ColorDepth::Ansi16);
        assert_eq!(depth_from(None, None), ColorDepth::Ansi16);
    }

    #[test]
    fn explicit_colors_map_to_each_depth() {
        let rgb = Color::Rgb(0x7a, 0xa2, 0xf7);
        assert_eq!(convert(rgb, ColorDepth::TrueColor), rgb);
        assert!(matches!(
            convert(rgb, ColorDepth::Ansi256),
            Color::Indexed(_)
        ));
        assert!(matches!(
            convert(rgb, ColorDepth::Ansi16),
            Color::Black
                | Color::Red
                | Color::Green
                | Color::Yellow
                | Color::Blue
                | Color::Magenta
                | Color::Cyan
                | Color::Gray
                | Color::DarkGray
                | Color::LightRed
                | Color::LightGreen
                | Color::LightYellow
                | Color::LightBlue
                | Color::LightMagenta
                | Color::LightCyan
                | Color::White
        ));
        assert_eq!(convert(Color::Cyan, ColorDepth::Ansi16), Color::Cyan);
        assert_eq!(convert(Color::Reset, ColorDepth::Ansi256), Color::Reset);
    }

    #[test]
    fn dark_and_light_themes_differ_and_are_consistent() {
        let dark = theme(ThemeChoice::Dark, ColorDepth::TrueColor);
        let light = theme(ThemeChoice::Light, ColorDepth::TrueColor);
        assert_ne!(dark.accent, light.accent);
        assert_ne!(dark.border, light.border);
        assert_ne!(dark.status.bg, light.status.bg);
        assert_eq!(dark.accent, Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(light.accent, Color::Rgb(0x1e, 0x66, 0xf5));
    }

    #[test]
    fn the_256_palette_uses_the_cube_and_grayscale_ramp() {
        assert_eq!(xterm(16), (0, 0, 0));
        assert_eq!(xterm(231), (255, 255, 255));
        assert_eq!(xterm(232), (8, 8, 8));
        assert_eq!(xterm(255), (238, 238, 238));
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(0, 0, 0), 16);
    }
}
