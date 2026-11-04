//! UI themes and color-depth mapping.
//!
//! Terminal cells keep their own colors; a theme styles only the chrome:
//! the space rail, pane frames, the bottom line, overlays, peek, help,
//! palette, search and local selection. The default (and `dark`) is the
//! design's dark palette; `light` is the design's light palette. The two
//! Catppuccin palettes keep their old names for people who prefer them, and
//! `terminal` keeps the earlier look in the outer terminal's own named
//! palette. Explicit colors are mapped down for 16-color and 256-color
//! terminals.

use ratatui::style::{Color, Modifier, Style};

/// The theme name from config.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    /// The design's dark palette.
    #[default]
    Default,
    /// The design's dark palette under its explicit name.
    Dark,
    /// The design's light palette.
    Light,
    /// The old Catppuccin Mocha look.
    Catppuccin,
    /// The old Catppuccin Latte look.
    CatppuccinLatte,
    /// The earlier look in the outer terminal's own named palette.
    Terminal,
}

impl ThemeChoice {
    pub fn parse(value: &str) -> Option<ThemeChoice> {
        match value {
            "default" => Some(ThemeChoice::Default),
            "dark" => Some(ThemeChoice::Dark),
            "light" => Some(ThemeChoice::Light),
            "catppuccin" => Some(ThemeChoice::Catppuccin),
            "catppuccin-latte" => Some(ThemeChoice::CatppuccinLatte),
            "terminal" => Some(ThemeChoice::Terminal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Default => "default",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
            ThemeChoice::Catppuccin => "catppuccin",
            ThemeChoice::CatppuccinLatte => "catppuccin-latte",
            ThemeChoice::Terminal => "terminal",
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

/// The chrome colors for one theme. Every role exists in every theme, so the
/// drawing code never asks which theme is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// The outer page background behind the chrome.
    pub page: Color,
    /// The chrome background behind bars, panels and overlays.
    pub panel_bg: Color,
    /// The rail's background, a step apart from the panes.
    pub side: Color,
    /// The bottom line's background.
    pub bar: Color,
    /// Hairlines: frames, separators and unfocused borders.
    pub line: Color,
    /// Secondary text: hints, section headers, idle agents.
    pub muted: Color,
    /// Dim text, quieter than muted.
    pub dim: Color,
    /// The focus and emphasis color (the design's mint).
    pub mint: Color,
    /// The color of a blocked agent.
    pub amber: Color,
    /// The color of a working agent.
    pub blue: Color,
    /// The background of the active space and the focused agent row.
    pub active_bg: Color,
    /// Raised rows: selected sidebar rows and inactive tabs.
    pub surface0: Color,
    /// Older name for the hairline color.
    pub surface1: Color,
    /// Older name for the muted text color.
    pub overlay0: Color,
    /// Muted text, between secondary and primary.
    pub overlay1: Color,
    /// Primary text.
    pub text: Color,
    /// Slightly quieter primary text.
    pub subtext0: Color,
    /// The focus and emphasis color.
    pub accent: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub mauve: Color,
    pub peach: Color,
    pub teal: Color,
    /// The ordinary status bar (background and primary text).
    pub status: Style,
    /// An active tab: accent background, panel text.
    pub tab_active: Style,
    /// An inactive tab: surface0 background, subtext text.
    pub tab_inactive: Style,
    /// The terminal text selection highlight.
    pub selection: Color,
}

impl Default for Theme {
    fn default() -> Theme {
        design_dark()
    }
}

impl Theme {
    /// The earlier look: the outer terminal's own named colors and reversed
    /// bars, kept for `theme: "terminal"`.
    pub fn terminal() -> Theme {
        Theme {
            page: Color::Reset,
            panel_bg: Color::Reset,
            side: Color::Reset,
            bar: Color::Reset,
            line: Color::DarkGray,
            muted: Color::Cyan,
            dim: Color::DarkGray,
            mint: Color::Cyan,
            amber: Color::Yellow,
            blue: Color::LightBlue,
            active_bg: Color::DarkGray,
            surface0: Color::Reset,
            surface1: Color::DarkGray,
            overlay0: Color::Cyan,
            overlay1: Color::DarkGray,
            text: Color::Reset,
            subtext0: Color::Reset,
            accent: Color::Cyan,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::Red,
            mauve: Color::Magenta,
            peach: Color::LightRed,
            teal: Color::LightCyan,
            status: Style::default().add_modifier(Modifier::REVERSED),
            tab_active: Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            tab_inactive: Style::default().add_modifier(Modifier::DIM),
            selection: Color::DarkGray,
        }
    }

    /// The background for overlays and panels.
    pub fn panel(&self) -> Style {
        Style::default().bg(self.panel_bg)
    }

    /// The selected row of a list overlay: accent background, panel text.
    pub fn selected_row(&self) -> Style {
        Style::default()
            .fg(self.panel_bg)
            .bg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// The sidebar's selected row, quieter than an overlay's. The legacy
    /// terminal theme has no surface color, so it reverses the row like the
    /// old overlay did; a reset background would leave no visible cue.
    pub fn selected_sidebar(&self) -> Style {
        if self.surface0 == Color::Reset {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().bg(self.surface0)
        }
    }

    /// The color of one Core-reported agent word. Only the word decides it;
    /// nothing is inferred from a screen.
    pub fn status_word(&self, word: &str) -> Color {
        match word {
            "working" => self.blue,
            "blocked" => self.amber,
            "error" => self.red,
            "done" => self.mint,
            "idle" => self.dim,
            _ => self.muted,
        }
    }
}

/// The approved design's dark roles.
fn design_dark() -> Theme {
    let page = Color::Rgb(0x10, 0x14, 0x16);
    let bg = Color::Rgb(0x17, 0x1c, 0x1f);
    let side = Color::Rgb(0x1c, 0x23, 0x26);
    let bar = Color::Rgb(0x20, 0x28, 0x2c);
    let line = Color::Rgb(0x30, 0x3d, 0x42);
    let text = Color::Rgb(0xd4, 0xdf, 0xe1);
    let muted = Color::Rgb(0x8a, 0x9b, 0x9f);
    let dim = Color::Rgb(0x64, 0x77, 0x7d);
    let mint = Color::Rgb(0x9d, 0xdb, 0xc6);
    let active_bg = Color::Rgb(0x27, 0x3e, 0x36);
    let amber = Color::Rgb(0xe6, 0xbd, 0x7c);
    let blue = Color::Rgb(0x99, 0xbd, 0xde);
    let red = Color::Rgb(0xdc, 0x92, 0x92);
    Theme {
        page,
        panel_bg: bg,
        side,
        bar,
        line,
        muted,
        dim,
        mint,
        amber,
        blue,
        active_bg,
        surface0: bar,
        surface1: line,
        overlay0: muted,
        overlay1: dim,
        text,
        subtext0: muted,
        accent: mint,
        green: mint,
        yellow: amber,
        red,
        mauve: blue,
        peach: amber,
        teal: mint,
        status: Style::default().fg(text).bg(bar),
        tab_active: Style::default()
            .fg(bg)
            .bg(mint)
            .add_modifier(Modifier::BOLD),
        tab_inactive: Style::default().fg(muted).bg(bar),
        selection: active_bg,
    }
}

/// The approved design's light roles.
fn design_light() -> Theme {
    let page = Color::Rgb(0xe9, 0xed, 0xeb);
    let bg = Color::Rgb(0xf7, 0xf8, 0xf4);
    let side = Color::Rgb(0xef, 0xf2, 0xee);
    let bar = Color::Rgb(0xe5, 0xeb, 0xe5);
    let line = Color::Rgb(0xcb, 0xd6, 0xce);
    let text = Color::Rgb(0x26, 0x3c, 0x37);
    let muted = Color::Rgb(0x59, 0x70, 0x67);
    let dim = Color::Rgb(0x71, 0x86, 0x7e);
    let mint = Color::Rgb(0x23, 0x6b, 0x55);
    let active_bg = Color::Rgb(0xdc, 0xe9, 0xdf);
    let amber = Color::Rgb(0x9c, 0x64, 0x21);
    let blue = Color::Rgb(0x3b, 0x6a, 0x91);
    let red = Color::Rgb(0xae, 0x4b, 0x4b);
    Theme {
        page,
        panel_bg: bg,
        side,
        bar,
        line,
        muted,
        dim,
        mint,
        amber,
        blue,
        active_bg,
        surface0: bar,
        surface1: line,
        overlay0: muted,
        overlay1: dim,
        text,
        subtext0: muted,
        accent: mint,
        green: mint,
        yellow: amber,
        red,
        mauve: blue,
        peach: amber,
        teal: mint,
        status: Style::default().fg(text).bg(bar),
        tab_active: Style::default()
            .fg(bg)
            .bg(mint)
            .add_modifier(Modifier::BOLD),
        tab_inactive: Style::default().fg(muted).bg(bar),
        selection: active_bg,
    }
}

/// The Catppuccin Mocha roles, kept for the explicit `catppuccin` theme.
fn mocha() -> Theme {
    let panel_bg = Color::Rgb(0x18, 0x18, 0x25);
    let surface0 = Color::Rgb(0x31, 0x32, 0x44);
    let surface1 = Color::Rgb(0x45, 0x47, 0x5a);
    let overlay0 = Color::Rgb(0x6c, 0x70, 0x86);
    let overlay1 = Color::Rgb(0x7f, 0x84, 0x9c);
    let text = Color::Rgb(0xcd, 0xd6, 0xf4);
    let subtext0 = Color::Rgb(0xa6, 0xad, 0xc8);
    let accent = Color::Rgb(0x89, 0xb4, 0xfa);
    Theme {
        page: Color::Rgb(0x1e, 0x1e, 0x2e),
        panel_bg,
        side: Color::Rgb(0x18, 0x18, 0x25),
        bar: Color::Rgb(0x11, 0x11, 0x1b),
        line: surface1,
        muted: overlay0,
        dim: overlay1,
        mint: Color::Rgb(0x94, 0xe2, 0xd5),
        amber: Color::Rgb(0xf9, 0xe2, 0xaf),
        blue: accent,
        active_bg: surface0,
        surface0,
        surface1,
        overlay0,
        overlay1,
        text,
        subtext0,
        accent,
        green: Color::Rgb(0xa6, 0xe3, 0xa1),
        yellow: Color::Rgb(0xf9, 0xe2, 0xaf),
        red: Color::Rgb(0xf3, 0x8b, 0xa8),
        mauve: Color::Rgb(0xcb, 0xa6, 0xf7),
        peach: Color::Rgb(0xfa, 0xb3, 0x87),
        teal: Color::Rgb(0x94, 0xe2, 0xd5),
        status: Style::default().fg(text).bg(panel_bg),
        tab_active: Style::default()
            .fg(panel_bg)
            .bg(accent)
            .add_modifier(Modifier::BOLD),
        tab_inactive: Style::default().fg(subtext0).bg(surface0),
        selection: surface1,
    }
}

/// The Catppuccin Latte roles, kept for `catppuccin-latte`.
fn latte() -> Theme {
    let panel_bg = Color::Rgb(0xe6, 0xe9, 0xef);
    let surface0 = Color::Rgb(0xcc, 0xd0, 0xda);
    let surface1 = Color::Rgb(0xbc, 0xc0, 0xcc);
    let overlay0 = Color::Rgb(0x9c, 0xa0, 0xb0);
    let overlay1 = Color::Rgb(0x8c, 0x8f, 0xa1);
    let text = Color::Rgb(0x4c, 0x4f, 0x69);
    let subtext0 = Color::Rgb(0x6c, 0x6f, 0x85);
    let accent = Color::Rgb(0x1e, 0x66, 0xf5);
    Theme {
        page: Color::Rgb(0xef, 0xf1, 0xf5),
        panel_bg,
        side: Color::Rgb(0xe6, 0xe9, 0xef),
        bar: Color::Rgb(0xdc, 0xe0, 0xe8),
        line: surface1,
        muted: overlay0,
        dim: overlay1,
        mint: Color::Rgb(0x17, 0x92, 0x99),
        amber: Color::Rgb(0xdf, 0x8e, 0x1d),
        blue: accent,
        active_bg: surface0,
        surface0,
        surface1,
        overlay0,
        overlay1,
        text,
        subtext0,
        accent,
        green: Color::Rgb(0x40, 0xa0, 0x2b),
        yellow: Color::Rgb(0xdf, 0x8e, 0x1d),
        red: Color::Rgb(0xd2, 0x0f, 0x39),
        mauve: Color::Rgb(0x88, 0x39, 0xef),
        peach: Color::Rgb(0xfe, 0x64, 0x0b),
        teal: Color::Rgb(0x17, 0x92, 0x99),
        status: Style::default().fg(text).bg(panel_bg),
        tab_active: Style::default()
            .fg(panel_bg)
            .bg(accent)
            .add_modifier(Modifier::BOLD),
        tab_inactive: Style::default().fg(subtext0).bg(surface0),
        selection: surface1,
    }
}

/// Builds the theme for a choice and terminal depth.
pub fn theme(choice: ThemeChoice, depth: ColorDepth) -> Theme {
    match choice {
        ThemeChoice::Default | ThemeChoice::Dark => map(design_dark(), depth),
        ThemeChoice::Light => map(design_light(), depth),
        ThemeChoice::Catppuccin => map(mocha(), depth),
        ThemeChoice::CatppuccinLatte => map(latte(), depth),
        ThemeChoice::Terminal => Theme::terminal(),
    }
}

fn map(mut theme: Theme, depth: ColorDepth) -> Theme {
    theme.page = convert(theme.page, depth);
    theme.panel_bg = convert(theme.panel_bg, depth);
    theme.side = convert(theme.side, depth);
    theme.bar = convert(theme.bar, depth);
    theme.line = convert(theme.line, depth);
    theme.muted = convert(theme.muted, depth);
    theme.dim = convert(theme.dim, depth);
    theme.mint = convert(theme.mint, depth);
    theme.amber = convert(theme.amber, depth);
    theme.blue = convert(theme.blue, depth);
    theme.active_bg = convert(theme.active_bg, depth);
    theme.surface0 = convert(theme.surface0, depth);
    theme.surface1 = convert(theme.surface1, depth);
    theme.overlay0 = convert(theme.overlay0, depth);
    theme.overlay1 = convert(theme.overlay1, depth);
    theme.text = convert(theme.text, depth);
    theme.subtext0 = convert(theme.subtext0, depth);
    theme.accent = convert(theme.accent, depth);
    theme.green = convert(theme.green, depth);
    theme.yellow = convert(theme.yellow, depth);
    theme.red = convert(theme.red, depth);
    theme.mauve = convert(theme.mauve, depth);
    theme.peach = convert(theme.peach, depth);
    theme.teal = convert(theme.teal, depth);
    theme.selection = convert(theme.selection, depth);
    theme.status = map_style(theme.status, depth);
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
    fn the_default_theme_is_the_design_dark_palette() {
        let theme = theme(ThemeChoice::Default, ColorDepth::TrueColor);
        assert_eq!(theme.page, Color::Rgb(0x10, 0x14, 0x16));
        assert_eq!(theme.panel_bg, Color::Rgb(0x17, 0x1c, 0x1f));
        assert_eq!(theme.side, Color::Rgb(0x1c, 0x23, 0x26));
        assert_eq!(theme.bar, Color::Rgb(0x20, 0x28, 0x2c));
        assert_eq!(theme.line, Color::Rgb(0x30, 0x3d, 0x42));
        assert_eq!(theme.text, Color::Rgb(0xd4, 0xdf, 0xe1));
        assert_eq!(theme.muted, Color::Rgb(0x8a, 0x9b, 0x9f));
        assert_eq!(theme.dim, Color::Rgb(0x64, 0x77, 0x7d));
        assert_eq!(theme.mint, Color::Rgb(0x9d, 0xdb, 0xc6));
        assert_eq!(theme.active_bg, Color::Rgb(0x27, 0x3e, 0x36));
        assert_eq!(theme.amber, Color::Rgb(0xe6, 0xbd, 0x7c));
        assert_eq!(theme.blue, Color::Rgb(0x99, 0xbd, 0xde));
        assert_eq!(theme.red, Color::Rgb(0xdc, 0x92, 0x92));
        assert_eq!(theme.accent, theme.mint, "focus uses the mint role");
        assert!(!theme.status.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(theme.status.bg, Some(theme.bar));
        assert_eq!(theme.tab_active.bg, Some(theme.accent));
        assert_eq!(theme.tab_active.fg, Some(theme.panel_bg));
        assert_eq!(theme.tab_inactive.bg, Some(theme.surface0));
    }

    #[test]
    fn theme_names_map_to_the_approved_palettes_and_keep_the_old_aliases() {
        assert_eq!(ThemeChoice::parse("default"), Some(ThemeChoice::Default));
        assert_eq!(ThemeChoice::parse("dark"), Some(ThemeChoice::Dark));
        assert_eq!(ThemeChoice::parse("light"), Some(ThemeChoice::Light));
        assert_eq!(
            ThemeChoice::parse("catppuccin"),
            Some(ThemeChoice::Catppuccin)
        );
        assert_eq!(
            ThemeChoice::parse("catppuccin-latte"),
            Some(ThemeChoice::CatppuccinLatte)
        );
        assert_eq!(ThemeChoice::parse("terminal"), Some(ThemeChoice::Terminal));
        assert_eq!(ThemeChoice::parse("neon"), None);
        assert_eq!(ThemeChoice::Default.label(), "default");
        assert_eq!(ThemeChoice::Dark.label(), "dark");
        assert_eq!(ThemeChoice::Catppuccin.label(), "catppuccin");
        assert_eq!(ThemeChoice::CatppuccinLatte.label(), "catppuccin-latte");
        assert_eq!(ThemeChoice::Terminal.label(), "terminal");

        let default = theme(ThemeChoice::Default, ColorDepth::TrueColor);
        let dark = theme(ThemeChoice::Dark, ColorDepth::TrueColor);
        assert_eq!(default, dark, "dark is the explicit name of the default");
        let light = theme(ThemeChoice::Light, ColorDepth::TrueColor);
        assert_eq!(light.page, Color::Rgb(0xe9, 0xed, 0xeb));
        assert_ne!(default, light);

        let mocha = theme(ThemeChoice::Catppuccin, ColorDepth::TrueColor);
        assert_eq!(mocha.panel_bg, Color::Rgb(0x18, 0x18, 0x25));
        let latte = theme(ThemeChoice::CatppuccinLatte, ColorDepth::TrueColor);
        assert_eq!(latte.panel_bg, Color::Rgb(0xe6, 0xe9, 0xef));
    }

    #[test]
    fn the_terminal_theme_keeps_the_original_named_palette() {
        let terminal = theme(ThemeChoice::Terminal, ColorDepth::TrueColor);
        assert_eq!(terminal.accent, Color::Cyan);
        assert_eq!(terminal.surface1, Color::DarkGray);
        assert_eq!(terminal.mauve, Color::Magenta);
        assert!(terminal.status.add_modifier.contains(Modifier::REVERSED));
        assert!(
            terminal
                .tab_active
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_ne!(terminal, theme(ThemeChoice::Default, ColorDepth::TrueColor));
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
    fn every_palette_role_degrades_without_leaving_rgb_behind() {
        for choice in [
            ThemeChoice::Default,
            ThemeChoice::Light,
            ThemeChoice::Catppuccin,
            ThemeChoice::CatppuccinLatte,
        ] {
            for depth in [ColorDepth::Ansi256, ColorDepth::Ansi16] {
                let theme = theme(choice, depth);
                for color in [
                    theme.page,
                    theme.panel_bg,
                    theme.side,
                    theme.bar,
                    theme.line,
                    theme.muted,
                    theme.dim,
                    theme.mint,
                    theme.amber,
                    theme.blue,
                    theme.active_bg,
                    theme.surface0,
                    theme.surface1,
                    theme.overlay0,
                    theme.overlay1,
                    theme.text,
                    theme.subtext0,
                    theme.accent,
                    theme.green,
                    theme.yellow,
                    theme.red,
                    theme.mauve,
                    theme.peach,
                    theme.teal,
                    theme.selection,
                ] {
                    assert!(
                        !matches!(color, Color::Rgb(..)),
                        "{choice:?} at {depth:?} left {color:?} unmapped"
                    );
                }
                for style in [theme.status, theme.tab_active, theme.tab_inactive] {
                    assert!(!matches!(style.fg, Some(Color::Rgb(..))));
                    assert!(!matches!(style.bg, Some(Color::Rgb(..))));
                }
            }
        }
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

    #[test]
    fn status_words_use_only_the_core_reported_word() {
        let theme = Theme::default();
        assert_eq!(theme.status_word("working"), theme.blue);
        assert_eq!(theme.status_word("blocked"), theme.amber);
        assert_eq!(theme.status_word("error"), theme.red);
        assert_eq!(theme.status_word("done"), theme.mint);
        assert_eq!(theme.status_word("idle"), theme.dim);
        assert_eq!(theme.status_word("thinking"), theme.muted);
        // The design roles are distinct, so the words never read alike.
        assert_ne!(theme.status_word("working"), theme.status_word("blocked"));
        assert_ne!(theme.status_word("done"), theme.status_word("idle"));
    }
}
