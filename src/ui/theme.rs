//! Colour palette. Values follow herdr's default Catppuccin Mocha palette.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub panel_bg: Color,
    pub active_row_bg: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface_dim: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub mauve: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub cyan: Color,
    pub magenta: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::mocha()
    }
}

impl Theme {
    pub const fn mocha() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250),
            panel_bg: Color::Rgb(24, 24, 37),
            active_row_bg: Color::Rgb(30, 30, 46),
            surface0: Color::Rgb(49, 50, 68),
            surface1: Color::Rgb(69, 71, 90),
            surface_dim: Color::Rgb(30, 30, 46),
            overlay0: Color::Rgb(108, 112, 134),
            overlay1: Color::Rgb(127, 132, 156),
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            mauve: Color::Rgb(203, 166, 247),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            blue: Color::Rgb(116, 199, 236),
            cyan: Color::Rgb(148, 226, 213),
            magenta: Color::Rgb(245, 194, 231),
        }
    }
}
