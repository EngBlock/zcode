//! A terminal's colors from the theme: the 16 ANSI colors from its base
//! colors (so they follow light and dark appearance), the editor's
//! foreground and background, and xterm's 256-color cube and gray ramp for
//! the rest. Colors a program sets (OSC 4/10/11) win.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::Theme;
use gpui_kit::{Hsla, Rgba, hsla};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub ansi: [Hsla; 16],
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub link: Hsla,
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let dark = theme.is_dark();
        let gray = |l: f32| hsla(0., 0., l, 1.);
        let (black, bright_black, white, bright_white) =
            if dark { (gray(0.18), gray(0.45), gray(0.80), gray(1.0)) } else { (gray(0.0), gray(0.45), gray(0.75), gray(0.92)) };
        // The light variants are the bright colors on a dark background;
        // on a light one the base colors read better as bright.
        let pair = |base: Hsla, light: Hsla| if dark { (base, light) } else { (base.blend(gray(0.).opacity(0.15)), base) };
        let (red, bright_red) = pair(theme.red, theme.red_light);
        let (green, bright_green) = pair(theme.green, theme.green_light);
        let (yellow, bright_yellow) = pair(theme.yellow, theme.yellow_light);
        let (blue, bright_blue) = pair(theme.blue, theme.blue_light);
        let (magenta, bright_magenta) = pair(theme.magenta, theme.magenta_light);
        let (cyan, bright_cyan) = pair(theme.cyan, theme.cyan_light);
        Self {
            ansi: [
                black, red, green, yellow, blue, magenta, cyan, white, bright_black, bright_red, bright_green, bright_yellow,
                bright_blue, bright_magenta, bright_cyan, bright_white,
            ],
            foreground: theme.foreground,
            background: theme.background,
            cursor: theme.caret,
            selection: theme.selection,
            link: theme.link,
        }
    }

    /// A cell's color, with the program's own colors first.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Hsla {
        match color {
            Color::Spec(rgb) => to_hsla(rgb),
            Color::Indexed(index) => self.indexed(index as usize, overrides),
            Color::Named(named) => self.named(named, overrides),
        }
    }

    fn indexed(&self, index: usize, overrides: &Colors) -> Hsla {
        if let Some(rgb) = overrides[index] {
            return to_hsla(rgb);
        }
        match index {
            0..16 => self.ansi[index],
            _ => to_hsla(xterm_color(index as u8)),
        }
    }

    fn named(&self, named: NamedColor, overrides: &Colors) -> Hsla {
        if let Some(rgb) = overrides[named] {
            return to_hsla(rgb);
        }
        let index = named as usize;
        match named {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.foreground.opacity(0.7),
            _ if index < 16 => self.ansi[index],
            // Dim black … dim white.
            _ => self.ansi[(index - NamedColor::DimBlack as usize) % 8].opacity(0.7),
        }
    }

    /// The color a program asked for (OSC 4/10/11/12 queries).
    pub fn rgb(&self, index: usize, overrides: &Colors) -> Rgb {
        let color = match index {
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            index if index < 256 => self.indexed(index, overrides),
            _ => self.foreground,
        };
        let rgba = Rgba::from(color);
        Rgb { r: (rgba.r * 255.).round() as u8, g: (rgba.g * 255.).round() as u8, b: (rgba.b * 255.).round() as u8 }
    }
}

pub fn to_hsla(rgb: Rgb) -> Hsla {
    Rgba { r: rgb.r as f32 / 255., g: rgb.g as f32 / 255., b: rgb.b as f32 / 255., a: 1. }.into()
}

/// xterm's colors 16–255: a 6×6×6 cube, then 24 grays.
pub fn xterm_color(index: u8) -> Rgb {
    match index {
        16..=231 => {
            let index = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb { r: level(index / 36), g: level(index / 6 % 6), b: level(index % 6) }
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            Rgb { r: v, g: v, b: v }
        }
        // The first 16 come from the theme.
        _ => Rgb { r: 0, g: 0, b: 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_the_gray_ramp() {
        assert_eq!(xterm_color(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(xterm_color(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(xterm_color(21), Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(xterm_color(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(xterm_color(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(xterm_color(255), Rgb { r: 238, g: 238, b: 238 });
    }

    #[test]
    fn named_colors_and_the_program_s_own() {
        let palette = Palette {
            ansi: std::array::from_fn(|i| hsla(i as f32 / 16., 0.5, 0.5, 1.)),
            foreground: hsla(0., 0., 0.9, 1.),
            background: hsla(0., 0., 0.1, 1.),
            cursor: hsla(0., 0., 1., 1.),
            selection: hsla(0.6, 0.5, 0.5, 0.3),
            link: hsla(0.6, 0.5, 0.5, 1.),
        };
        let mut colors = Colors::default();
        assert_eq!(palette.resolve(Color::Named(NamedColor::Red), &colors), palette.ansi[1]);
        assert_eq!(palette.resolve(Color::Named(NamedColor::BrightBlue), &colors), palette.ansi[12]);
        assert_eq!(palette.resolve(Color::Indexed(3), &colors), palette.ansi[3]);
        assert_eq!(palette.resolve(Color::Named(NamedColor::Background), &colors), palette.background);
        assert_eq!(palette.resolve(Color::Named(NamedColor::DimGreen), &colors), palette.ansi[2].opacity(0.7));
        colors[1] = Some(Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(palette.resolve(Color::Named(NamedColor::Red), &colors), to_hsla(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(palette.rgb(196, &colors), Rgb { r: 255, g: 0, b: 0 });
    }
}
