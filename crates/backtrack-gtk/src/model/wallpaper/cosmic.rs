// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! COSMIC's wallpaper (S18-T19), from the state cosmic-bg keeps of what each
//! output shows: `wallpapers`, a RON list of `(output, source)` pairs, each
//! source a `Path("…")` or a `Color(Single((r, g, b)))`, its channels from 0
//! to 1, or a `Color(Gradient(…))`, shown here as its first colour. It is
//! rewritten whenever the picture changes, a slideshow's step included.

use std::path::PathBuf;

use super::{Fit, Rgb, Wallpaper};

/// A RON string at the start of `text`, unescaped, and what follows it.
fn string(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((out, &text[at + 2..])),
            '\\' => out.push(chars.next()?.1),
            c => out.push(c),
        }
    }
    None
}

/// The first three numbers after `text`'s start, as a colour.
fn colour(text: &str) -> Option<Rgb> {
    let channels: Vec<u8> = text
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<f64>().ok())
        .take(3)
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    match channels[..] {
        [r, g, b] => Some(Rgb(r, g, b)),
        _ => None,
    }
}

/// Every output's source, in order.
fn wallpapers(text: &str) -> Vec<(String, Wallpaper)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("(\"") {
        let Some((output, after)) = string(&rest[at + 1..]) else {
            break;
        };
        let after = after.trim_start().trim_start_matches(',').trim_start();
        // On past the source, so that a path's own `("` is not taken for
        // the next output.
        let (wallpaper, next) = if let Some(path) = after.strip_prefix("Path(") {
            match string(path.trim_start()) {
                Some((path, beyond)) => (
                    Some(Wallpaper::Picture {
                        path: PathBuf::from(path),
                        fit: Fit::Cover,
                        behind: Rgb(0, 0, 0),
                    }),
                    beyond,
                ),
                None => (None, after),
            }
        } else if let Some(colours) = after.strip_prefix("Color(") {
            (colour(colours).map(Wallpaper::Colour), after)
        } else {
            (None, after)
        };
        rest = next;
        found.extend(wallpaper.map(|w| (output, w)));
    }
    found
}

/// What `output` (its connector, such as `eDP-1`) shows, or the first
/// output's.
pub fn choose(text: &str, output: Option<&str>) -> Option<Wallpaper> {
    let found = wallpapers(text);
    output
        .and_then(|output| found.iter().find(|(name, _)| name == output))
        .or_else(|| found.first())
        .map(|(_, wallpaper)| wallpaper.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As cosmic-config writes the state, pretty-printed by RON.
    const STATE: &str = r#"[
    ("eDP-1", Path("/usr/share/backgrounds/cosmic/orion_nebula_nasa_heic0601a.jpg")),
    ("DP-2", Color(Single((0.2, 0.4, 0.6)))),
    ("HDMI-A-1", Color(Gradient((colors: [(1.0, 0.0, 0.0), (0.0, 0.0, 1.0)], radius: 270.0)))),
]"#;

    #[test]
    fn each_output_reads_its_own() {
        assert_eq!(
            choose(STATE, Some("eDP-1")),
            Some(Wallpaper::Picture {
                path: PathBuf::from(
                    "/usr/share/backgrounds/cosmic/orion_nebula_nasa_heic0601a.jpg"
                ),
                fit: Fit::Cover,
                behind: Rgb(0, 0, 0),
            })
        );
        assert_eq!(
            choose(STATE, Some("DP-2")),
            Some(Wallpaper::Colour(Rgb(51, 102, 153)))
        );
        assert_eq!(
            choose(STATE, Some("HDMI-A-1")),
            Some(Wallpaper::Colour(Rgb(255, 0, 0))),
            "a gradient, as its first colour"
        );
        assert_eq!(
            choose(STATE, Some("Virtual-1")),
            choose(STATE, Some("eDP-1")),
            "an output it does not name, the first"
        );
    }

    #[test]
    fn a_path_may_hold_an_escaped_quote() {
        let state = r#"[("eDP-1", Path("/home/k/a \"b\".png"))]"#;
        assert_eq!(
            choose(state, None),
            Some(Wallpaper::Picture {
                path: PathBuf::from("/home/k/a \"b\".png"),
                fit: Fit::Cover,
                behind: Rgb(0, 0, 0),
            })
        );
        assert_eq!(choose("[]", None), None);
    }
}
