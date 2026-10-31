// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! GNOME's wallpaper, from `org.gnome.desktop.background`: `picture-uri`, or
//! `picture-uri-dark` when the desktop's colour scheme is dark, fitted as
//! `picture-options` says, over `primary-color`. Cinnamon and MATE kept
//! GNOME 2's keys under schemas of their own (S18-T19), and are read the same
//! way.

use gtk4::glib;

use super::{Fit, Rgb, Wallpaper, Why};

/// The schema the keys are in, and the dconf path it is kept at.
pub const SCHEMA: &str = "org.gnome.desktop.background";
pub const PATH: &str = "/org/gnome/desktop/background/";

/// Where a desktop that keeps its wallpaper as GNOME does keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schema {
    pub id: &'static str,
    pub path: &'static str,
    /// The key that names the picture: MATE's is a path, `picture-filename`,
    /// which `choose` reads as it reads the plain paths of an older GNOME.
    picture: &'static str,
}

impl Schema {
    /// This schema's name for one of `KEYS`.
    pub fn key(&self, name: &'static str) -> &'static str {
        if name == KEYS[0] {
            self.picture
        } else {
            name
        }
    }
}

pub const GNOME: Schema = Schema {
    id: SCHEMA,
    path: PATH,
    picture: "picture-uri",
};
pub const CINNAMON: Schema = Schema {
    id: "org.cinnamon.desktop.background",
    path: "/org/cinnamon/desktop/background/",
    picture: "picture-uri",
};
pub const MATE: Schema = Schema {
    id: "org.mate.background",
    path: "/org/mate/desktop/background/",
    picture: "picture-filename",
};

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
    pub fn read(mut key: impl FnMut(&'static str) -> Option<String>) -> Settings {
        let mut get = |name| key(name).unwrap_or_default();
        Settings {
            picture_uri: get(KEYS[0]),
            picture_uri_dark: get(KEYS[1]),
            picture_options: get(KEYS[2]),
            primary_color: get(KEYS[3]),
        }
    }
}

/// Cinnamon's wallpaper since 6.7, `picture-uri-list`: one entry for each
/// monitor, by its `connector`, in its 'independent' mode, or one with none in
/// 'mirror' and 'spanned', each with GNOME's keys. The entry for `output`,
/// else the one for every monitor, else the first; None while the list is
/// empty, when the older keys stand. Fedora 45's Cinnamon 6.7 changes only
/// the list (found on the GNOME VM, 2026-10-31).
pub fn from_list(
    entries: &[std::collections::HashMap<String, String>],
    output: Option<&str>,
) -> Option<Settings> {
    let connector = |entry: &std::collections::HashMap<String, String>| {
        entry.get("connector").filter(|c| !c.is_empty()).cloned()
    };
    let entry = output
        .and_then(|output| {
            entries
                .iter()
                .find(|e| connector(e).as_deref() == Some(output))
        })
        .or_else(|| entries.iter().find(|e| connector(e).is_none()))
        .or_else(|| entries.first())?;
    let get = |key: &str| entry.get(key).cloned().unwrap_or_default();
    Some(Settings {
        picture_uri: get("picture-uri"),
        picture_uri_dark: String::new(),
        picture_options: get("picture-options"),
        primary_color: get("primary-color"),
    })
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

    #[test]
    fn cinnamons_list_gives_each_monitor_its_own() {
        let entry = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<std::collections::HashMap<_, _>>()
        };
        // As on the GNOME VM after Keith changed it, in 'mirror' mode.
        let mirror = [entry(&[
            (
                "picture-uri",
                "file:///usr/share/backgrounds/gnome/blendpills-l.jxl",
            ),
            ("picture-options", "zoom"),
            ("primary-color", "#023c88"),
        ])];
        assert_eq!(
            from_list(&mirror, Some("Virtual-1")).map(|s| s.picture_uri),
            Some("file:///usr/share/backgrounds/gnome/blendpills-l.jxl".to_owned())
        );
        let independent = [
            entry(&[("connector", "DP-1"), ("picture-uri", "file:///a.png")]),
            entry(&[("connector", "eDP-1"), ("picture-uri", "file:///b.png")]),
        ];
        assert_eq!(
            from_list(&independent, Some("eDP-1")).map(|s| s.picture_uri),
            Some("file:///b.png".to_owned())
        );
        assert_eq!(
            from_list(&independent, Some("HDMI-1")).map(|s| s.picture_uri),
            Some("file:///a.png".to_owned()),
            "a monitor it has nothing for, the first"
        );
        assert_eq!(from_list(&[], Some("eDP-1")), None, "the older keys stand");
    }

    #[test]
    fn cinnamon_and_mate_keep_gnomes_keys_under_their_own_schemas() {
        assert_eq!(CINNAMON.key("picture-uri"), "picture-uri");
        assert_eq!(MATE.key("picture-uri"), "picture-filename");
        assert_eq!(MATE.key("picture-options"), "picture-options");
        // MATE's picture is a plain path.
        let mate = Settings {
            picture_uri: "/usr/share/backgrounds/mate/nature/Wood.jpg".into(),
            picture_options: "stretched".into(),
            ..Settings::default()
        };
        assert_eq!(
            picture(choose(&mate, false)),
            (
                "/usr/share/backgrounds/mate/nature/Wood.jpg".to_owned(),
                Fit::Stretch,
                Rgb::BLACK
            )
        );
    }
}
