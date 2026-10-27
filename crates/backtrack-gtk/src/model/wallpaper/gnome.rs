// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! GNOME's wallpaper, from `org.gnome.desktop.background`: `picture-uri`, or
//! `picture-uri-dark` when the desktop's colour scheme is dark, fitted as
//! `picture-options` says, over `primary-color`.

use gtk4::glib;

use super::{Fit, Rgb, Wallpaper, Why};

/// The schema the keys are in, and the dconf path it is kept at.
pub const SCHEMA: &str = "org.gnome.desktop.background";
pub const PATH: &str = "/org/gnome/desktop/background/";

/// The keys, as the desktop has them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub picture_uri: String,
    pub picture_uri_dark: String,
    pub picture_options: String,
    pub primary_color: String,
}

/// The keys `Settings` holds, by name, for reading them one at a time.
pub const KEYS: [&str; 4] = [
    "picture-uri",
    "picture-uri-dark",
    "picture-options",
    "primary-color",
];

impl Settings {
    /// Settings from a reader of one key at a time, as GSettings and dconf's
    /// files each give them. A key it cannot read is empty, as GNOME treats
    /// an empty one.
    pub fn read(mut key: impl FnMut(&str) -> Option<String>) -> Settings {
        let mut get = |name| key(name).unwrap_or_default();
        Settings {
            picture_uri: get(KEYS[0]),
            picture_uri_dark: get(KEYS[1]),
            picture_options: get(KEYS[2]),
            primary_color: get(KEYS[3]),
        }
    }
}

/// What GNOME shows. The path may be a timed slideshow's XML (see
/// `super::slideshow`), which only the caller can read.
pub fn choose(settings: &Settings, dark: bool) -> Result<Wallpaper, Why> {
    let behind = Rgb::parse(&settings.primary_color).unwrap_or(Rgb::BLACK);
    let fit = match settings.picture_options.as_str() {
        "none" => return Ok(Wallpaper::Colour(behind)),
        "wallpaper" => Fit::Tile,
        "centered" => Fit::Center,
        "scaled" => Fit::Contain,
        "stretched" => Fit::Stretch,
        // "zoom", the default, and "spanned", which spreads one picture over
        // every monitor: on the one Rewind is on, each is the picture cover
        // that screen.
        _ => Fit::Cover,
    };
    // GNOME's dark picture is optional: without one, dark is the light one.
    let uri = if dark && !settings.picture_uri_dark.trim().is_empty() {
        settings.picture_uri_dark.trim()
    } else {
        settings.picture_uri.trim()
    };
    if uri.is_empty() {
        return Ok(Wallpaper::Colour(behind));
    }
    let path = match glib::filename_from_uri(uri) {
        Ok((path, _)) => path,
        // A plain path is what an older GNOME, or a person, sometimes wrote.
        Err(_) if uri.starts_with('/') => uri.into(),
        Err(_) => return Err(Why::NotAFile(uri.to_owned())),
    };
    Ok(Wallpaper::Picture { path, fit, behind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fedora() -> Settings {
        // Fedora 45's defaults, from its vendor override, 2026-10-27.
        Settings {
            picture_uri: "file:///usr/share/backgrounds/f45/default/f45-01-day.jxl".into(),
            picture_uri_dark: "file:///usr/share/backgrounds/f45/default/f45-01-night.jxl".into(),
            picture_options: "zoom".into(),
            primary_color: "#023c88".into(),
        }
    }

    fn picture(chosen: Result<Wallpaper, Why>) -> (String, Fit, Rgb) {
        match chosen.unwrap() {
            Wallpaper::Picture { path, fit, behind } => (path.display().to_string(), fit, behind),
            other => panic!("not a picture: {other:?}"),
        }
    }

    #[test]
    fn the_dark_picture_is_shown_when_the_desktop_is_dark() {
        let (light, fit, behind) = picture(choose(&fedora(), false));
        assert_eq!(light, "/usr/share/backgrounds/f45/default/f45-01-day.jxl");
        assert_eq!((fit, behind), (Fit::Cover, Rgb(0x02, 0x3c, 0x88)));
        let (dark, _, _) = picture(choose(&fedora(), true));
        assert_eq!(dark, "/usr/share/backgrounds/f45/default/f45-01-night.jxl");
    }

    #[test]
    fn without_a_dark_picture_dark_shows_the_light_one() {
        let settings = Settings {
            picture_uri_dark: String::new(),
            ..fedora()
        };
        let (dark, _, _) = picture(choose(&settings, true));
        assert_eq!(dark, "/usr/share/backgrounds/f45/default/f45-01-day.jxl");
    }

    #[test]
    fn a_uri_is_unescaped_into_its_path() {
        let settings = Settings {
            picture_uri: "file:///home/keith/Pictures/lake%20at%20night.jpg".into(),
            ..fedora()
        };
        let (path, _, _) = picture(choose(&settings, false));
        assert_eq!(path, "/home/keith/Pictures/lake at night.jpg");
    }

    #[test]
    fn the_fit_follows_picture_options() {
        for (options, fit) in [
            ("zoom", Fit::Cover),
            ("spanned", Fit::Cover),
            ("scaled", Fit::Contain),
            ("stretched", Fit::Stretch),
            ("centered", Fit::Center),
            ("wallpaper", Fit::Tile),
        ] {
            let settings = Settings {
                picture_options: options.into(),
                ..fedora()
            };
            assert_eq!(picture(choose(&settings, false)).1, fit, "{options}");
        }
    }

    #[test]
    fn no_picture_is_the_colour_alone() {
        let none = Settings {
            picture_options: "none".into(),
            ..fedora()
        };
        assert_eq!(
            choose(&none, false),
            Ok(Wallpaper::Colour(Rgb(0x02, 0x3c, 0x88)))
        );
        let empty = Settings {
            picture_uri: String::new(),
            picture_uri_dark: String::new(),
            primary_color: "not a colour".into(),
            ..fedora()
        };
        assert_eq!(choose(&empty, true), Ok(Wallpaper::Colour(Rgb::BLACK)));
    }

    #[test]
    fn a_picture_that_is_not_a_local_file_is_refused() {
        let settings = Settings {
            picture_uri: "resource:///org/gnome/shell/theme/noise.png".into(),
            ..fedora()
        };
        assert_eq!(
            choose(&settings, false),
            Err(Why::NotAFile(
                "resource:///org/gnome/shell/theme/noise.png".into()
            ))
        );
    }

    #[test]
    fn settings_are_read_key_by_key() {
        let settings = Settings::read(|key| match key {
            "picture-uri" => Some("file:///a.png".into()),
            "picture-options" => Some("scaled".into()),
            _ => None,
        });
        assert_eq!(settings.picture_uri, "file:///a.png");
        assert_eq!(settings.picture_uri_dark, "");
        assert_eq!(settings.picture_options, "scaled");
    }
}
