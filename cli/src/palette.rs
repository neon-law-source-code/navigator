//! Neon Law Navigator's CLI palette — three Tailwind cyans, darkest to
//! lightest.
//!
//! - `CYAN_700` (`#0E7490`) — borders / separators / dim emphasis
//! - `CYAN_500` (`#06B6D4`) — primary highlights (codes, keys)
//! - `CYAN_300` (`#67E8F9`) — strong emphasis (table headers, titles)
//!
//! All styling routes through [`owo_colors::OwoColorize::if_supports_color`]
//! against `Stream::Stdout`, so when stdout isn't a TTY (CI, captured
//! by `assert_cmd`, piped through `less`) the output is plain ASCII
//! and downstream string matching still works.

use std::fmt::Display;

use owo_colors::{OwoColorize, Stream, Style};

/// Tailwind `cyan-700` — `#0E7490`. The darkest of the three.
pub const CYAN_700: (u8, u8, u8) = (14, 116, 144);
/// Tailwind `cyan-500` — `#06B6D4`. The `L`.
pub const CYAN_500: (u8, u8, u8) = (6, 182, 212);
/// Tailwind `cyan-300` — `#67E8F9`. The `N`.
pub const CYAN_300: (u8, u8, u8) = (103, 232, 249);

fn paint<T: Display>(value: T, rgb: (u8, u8, u8), bold: bool) -> String {
    let mut style = Style::new().truecolor(rgb.0, rgb.1, rgb.2);
    if bold {
        style = style.bold();
    }
    value
        .if_supports_color(Stream::Stdout, |t| t.style(style))
        .to_string()
}

/// Header/title text in bold cyan-300.
pub fn header<T: Display>(value: T) -> String {
    paint(value, CYAN_300, true)
}

/// Primary highlight (rule codes, counts, identifiers) in cyan-500.
pub fn highlight<T: Display>(value: T) -> String {
    paint(value, CYAN_500, false)
}

/// Dim accent (em-dash separators, paths, summary lines) in cyan-700.
pub fn dim<T: Display>(value: T) -> String {
    paint(value, CYAN_700, false)
}
