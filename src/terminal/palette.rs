//! The terminal's color set: alacritty's cell colors mapped onto a fixed
//! ANSI ramp, and the theme's own default foreground/background the OSC
//! 10/11/12 replies answer with.
//!
//! Terminals need stable, saturated ANSI colors — theme-derived tints
//! wash out agent CLIs' output. Both ramps are Zed's official
//! "One Dark" / "One Light" terminal ANSI colors
//! (zed-industries/zed `assets/themes/one/one.json`).

use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor, Rgb};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

/// Maps alacritty cell colors onto a fixed terminal palette.
///
/// Terminals need stable, saturated ANSI colors — theme-derived tints
/// wash out agent CLIs' output. Both palettes are Zed's official
/// "One Dark" / "One Light" terminal ANSI ramps
/// (zed-industries/zed `assets/themes/one/one.json`).
pub(crate) struct TerminalPalette {
    pub(crate) fg: Hsla,
    pub(crate) bg: Hsla,
    /// Block cursor fill / unfocused outline.
    pub(crate) cursor: Hsla,
    /// Selection wash — opaque, contrasts with `bg` in both modes
    /// (VSCode dark / macOS light selection blues).
    pub(crate) selection: Hsla,
    pub(crate) base: [Hsla; 16],
}

impl TerminalPalette {
    pub(crate) fn new(theme: &Theme) -> Self {
        let colors = DefaultColors::of(theme);
        match theme.mode {
            ThemeMode::Dark => Self {
                fg: rgb(colors.fg).into(),
                bg: rgb(colors.bg).into(),
                cursor: rgb(0x61afef).into(),
                selection: rgb(0x264f78).into(),
                base: one_dark_palette(),
            },
            ThemeMode::Light => Self {
                fg: rgb(colors.fg).into(),
                bg: rgb(colors.bg).into(),
                cursor: rgb(0x2f5af3).into(),
                selection: rgb(0xb3d7ff).into(),
                base: one_light_palette(),
            },
        }
    }
}

/// Zed "One Dark" terminal ANSI colors: dim row 0-7, bright row 8-15.
const ONE_DARK: [u32; 16] = [
    0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, //
    0x636d83, 0xEA858B, 0xAAD581, 0xFFD885, 0x85C1FF, 0xD398EB, 0x6ED5DE, 0xfafafa,
];

/// Zed "One Light" terminal ANSI colors: dim row 0-7, bright row 8-15.
const ONE_LIGHT: [u32; 16] = [
    0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0x950095, 0x0997b3, 0xbbbbbb, //
    0x555555, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0xa00095, 0x0bbcd6, 0xffffff,
];

fn one_dark_palette() -> [Hsla; 16] {
    ONE_DARK.map(|c| rgb(c).into())
}

fn one_light_palette() -> [Hsla; 16] {
    ONE_LIGHT.map(|c| rgb(c).into())
}

impl TerminalPalette {
    pub(crate) fn color(&self, c: TermColor) -> Hsla {
        match c {
            TermColor::Named(NamedColor::Foreground) => self.fg,
            TermColor::Named(NamedColor::Background) => self.bg,
            TermColor::Named(NamedColor::Cursor) => self.fg,
            TermColor::Named(named) => self.base[dimmed_index(named)],
            TermColor::Indexed(i) => match i {
                0..=15 => self.base[i as usize],
                16..=231 => {
                    let i = i - 16;
                    let (r, g, b) = (i / 36, (i % 36) / 6, i % 6);
                    let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
                    rgb_u24(v(r), v(g), v(b)).into()
                }
                gray => {
                    let g = 8 + 10 * (gray.saturating_sub(232));
                    rgb_u24(g, g, g).into()
                }
            },
            TermColor::Spec(rgb) => rgb_u24(rgb.r, rgb.g, rgb.b).into(),
        }
    }
}

fn dimmed_index(named: NamedColor) -> usize {
    match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        _ => 7,
    }
}

fn rgb_u24(r: u8, g: u8, b: u8) -> Rgba {
    rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
}
/// The default foreground/background the grid paints with: the *theme's*
/// own two colors, so a terminal cell a child left at its default reads
/// as strongly as the prose beside the pane, and an OSC 10/11/12 query
/// answers the color the grid actually paints. Held as one value so the
/// paint path (the UI thread, `TerminalPalette::new`) and the reply path
/// (the pump thread, via [`query_rgb`]) can never disagree.
///
/// They used to be pinned to Zed's One Dark/Light *terminal* pair
/// (`#abb2bf` on dark), which is deliberately dimmer than the app's
/// foreground: plain command output therefore sat a contrast step under
/// every other pane and read as blurry next to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DefaultColors {
    pub(crate) fg: u32,
    pub(crate) bg: u32,
}

impl DefaultColors {
    pub(crate) fn of(theme: &Theme) -> Self {
        Self {
            fg: hsla_u24(theme.foreground),
            bg: hsla_u24(theme.background),
        }
    }
}

/// An `Hsla` as the 24-bit value a terminal color is stored and replied
/// with (the 8-bit channel is the interface's own resolution).
fn hsla_u24(color: Hsla) -> u32 {
    let rgba: Rgba = color.into();
    let c = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (c(rgba.r) << 16) | (c(rgba.g) << 8) | c(rgba.b)
}

/// What an OSC 10/11/12 color query answers for `index`, or `None` for
/// the colors the ANSI ramp owns (the child gets no reply and keeps its
/// own default).
pub(crate) fn query_rgb(index: usize, colors: DefaultColors) -> Option<Rgb> {
    let value = if index == NamedColor::Background as usize {
        colors.bg
    } else if index == NamedColor::Foreground as usize || index == NamedColor::Cursor as usize {
        colors.fg
    } else {
        return None;
    };
    Some(ansi_rgb(value))
}

/// One of the constants above as alacritty's own RGB, for a reply.
fn ansi_rgb(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

#[cfg(test)]
mod palette_tests {
    use super::{NamedColor, dimmed_index};
    #[test]
    fn standard_ansi_colors_do_not_fall_back_to_white() {
        for (color, expected) in [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
        ]
        .into_iter()
        .zip(0..8)
        {
            assert_eq!(dimmed_index(color), expected);
        }
        assert_eq!(dimmed_index(NamedColor::BrightRed), 9);
        assert_eq!(dimmed_index(NamedColor::DimRed), 1);
    }
}
