// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The desktop's own wallpaper, as Rewind's background (S18-T2).
//!
//! GNOME and Plasma each say what they show in their own way, natively and
//! inside the Flatpak alike, and this works out from what they say which
//! picture it is and how it fills the screen. Reading what they say (the
//! settings, the files, the session bus) is `crate::wallpaper`'s; this is the
//! part a test can check without a desktop. Any other desktop, and anything
//! that goes wrong on the way, gets Backtrack's own background instead: the
//! starfield, which is always dark.

pub mod dconf;
pub mod gnome;
pub mod gvdb;
pub mod plasma;
pub mod slideshow;

use std::fmt;
use std::path::PathBuf;

/// A colour, as eight bits each of red, green and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);

    /// `#rrggbb` or `#rgb`, as GNOME writes a colour.
    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.trim().strip_prefix('#')?;
        let digit = |i: usize, width: usize| {
            u8::from_str_radix(hex.get(i..i + width)?, 16)
                .ok()
                .map(|v| if width == 1 { v * 17 } else { v })
        };
        match hex.len() {
            6 => Some(Rgb(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?)),
            3 => Some(Rgb(digit(0, 1)?, digit(1, 1)?, digit(2, 1)?)),
            _ => None,
        }
    }
}

/// How a picture fills the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fit {
    /// Scaled to cover the screen, cropped where it overflows. What both
    /// desktops do by default.
    Cover,
    /// Scaled to fit inside the screen, the colour behind it around it.
    Contain,
    /// Scaled to the screen's shape, whatever the picture's own.
    Stretch,
    /// At its own size, in the middle.
    Center,
    /// At its own size, repeated.
    Tile,
}

/// What the desktop shows behind its windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wallpaper {
    /// A picture, fitted, with a colour behind wherever it does not reach.
    Picture {
        path: PathBuf,
        fit: Fit,
        behind: Rgb,
    },
    /// A colour alone.
    Colour(Rgb),
}

/// Why the desktop's wallpaper is not shown, for the report and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// A desktop this does not know how to ask.
    OtherDesktop(String),
    /// The desktop could not be asked.
    Unreachable(String),
    /// The desktop shows something other than a picture or a colour, such as
    /// a Plasma wallpaper plugin of its own.
    NotAPicture(String),
    /// The desktop names a picture that is not a local file.
    NotAFile(String),
    /// The picture, or the wallpaper package, is not where the desktop says.
    Missing(PathBuf),
    /// The picture is there and could not be read.
    Unreadable(PathBuf, String),
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Why::OtherDesktop(name) if name.is_empty() => {
                write!(f, "the desktop does not say which it is")
            }
            Why::OtherDesktop(name) => write!(f, "{name} is not GNOME or Plasma"),
            Why::Unreachable(what) => write!(f, "the desktop could not be asked: {what}"),
            Why::NotAPicture(what) => write!(f, "the desktop shows {what}, not a picture"),
            Why::NotAFile(uri) => write!(f, "the desktop's picture is not a local file: {uri}"),
            Why::Missing(path) => write!(f, "{} is not there", path.display()),
            Why::Unreadable(path, why) => write!(f, "{} could not be read: {why}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_read_as_gnome_writes_them() {
        assert_eq!(Rgb::parse("#023c88"), Some(Rgb(0x02, 0x3c, 0x88)));
        assert_eq!(Rgb::parse("#fff"), Some(Rgb(255, 255, 255)));
        assert_eq!(Rgb::parse("023c88"), None);
        assert_eq!(Rgb::parse("#02x"), None);
    }
}
