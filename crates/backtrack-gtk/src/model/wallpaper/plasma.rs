// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Plasma's wallpaper, from what `org.kde.PlasmaShell.wallpaper(screen)` says
//! on the session bus: the screen's wallpaper plugin and its settings.
//!
//! The image plugin's `Image` names a picture, or a wallpaper package (a
//! folder of the same picture in many sizes, light and dark), or nothing,
//! which means the default: the one the look-and-feel package in use names.
//! All three were checked against what the Plasma VM's desktop shows
//! (Fedora 45, Plasma 6.7, 2026-10-27): with `kdeglobals` naming Breeze Dark
//! and a dark colour scheme, the desktop shows the dark picture of the Next
//! package, as resolved here.

use std::path::PathBuf;

use gtk4::glib;

use super::{Fit, Rgb, Why};

/// The look-and-feel package Plasma falls back to when nothing names one.
pub const DEFAULT_LOOK_AND_FEEL: &str = "org.kde.breeze.desktop";

/// What `wallpaper(screen)` says, as far as Rewind needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub plugin: String,
    pub image: String,
    pub fill_mode: i32,
    /// The colour behind a picture, or the colour plugin's colour, as ARGB.
    pub color: Option<u32>,
}

/// What the plugin shows, before files are looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// A picture, or a wallpaper package's folder: which, only the files say.
    Path(PathBuf),
    /// The look-and-feel package's default.
    Default,
    Colour(Rgb),
}

pub fn shown(settings: &Settings) -> Result<Shown, Why> {
    match settings.plugin.as_str() {
        "org.kde.color" => Ok(Shown::Colour(behind(settings))),
        "org.kde.image" => {
            let image = settings.image.trim();
            if image.is_empty() {
                return Ok(Shown::Default);
            }
            match glib::filename_from_uri(image) {
                Ok((path, _)) => Ok(Shown::Path(path)),
                Err(_) if image.starts_with('/') => Ok(Shown::Path(image.into())),
                Err(_) => Err(Why::NotAFile(image.to_owned())),
            }
        }
        "org.kde.slideshow" => Err(Why::NotAPicture("a slideshow".to_owned())),
        "" => Err(Why::Unreachable(
            "Plasma named no wallpaper plugin".to_owned(),
        )),
        other => Err(Why::NotAPicture(format!("the wallpaper plugin {other}"))),
    }
}

/// How the picture fills the screen: the image plugin's `FillMode`, Qt's
/// `Image.FillMode`, cover by default.
pub fn fit(settings: &Settings) -> Fit {
    match settings.fill_mode {
        0 => Fit::Stretch,
        1 => Fit::Contain,
        3..=5 => Fit::Tile,
        6 => Fit::Center,
        _ => Fit::Cover,
    }
}

/// The colour behind the picture, black when there is none.
pub fn behind(settings: &Settings) -> Rgb {
    settings.color.map_or(Rgb::BLACK, |argb| {
        let [_, r, g, b] = argb.to_be_bytes();
        Rgb(r, g, b)
    })
}

/// `key` in `[section]` of a KConfig file. Entries written with a flag
/// (`key[$e]=`) count as the key.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name == section;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let name = name.split_once('[').map_or(name, |(bare, _)| bare);
        if name == key {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// The look-and-feel package in use: the first of the `kdeglobals` files (most
/// important first, as KConfig reads them) to name one.
pub fn look_and_feel<'a>(kdeglobals: impl IntoIterator<Item = &'a str>) -> String {
    kdeglobals
        .into_iter()
        .find_map(|text| ini_value(text, "KDE", "LookAndFeelPackage").filter(|v| !v.is_empty()))
        .unwrap_or_else(|| DEFAULT_LOOK_AND_FEEL.to_owned())
}

/// The wallpaper a look-and-feel package's `contents/defaults` names: a
/// package's name, as `Next`, or a path.
pub fn default_wallpaper(defaults: &str) -> Option<String> {
    ini_value(defaults, "Wallpaper", "Image").filter(|v| !v.is_empty())
}

/// The picture in a wallpaper package for a screen of `screen` pixels: in
/// `images_dark/` when the colour scheme is dark and the package has one,
/// otherwise `images/`; of the sizes there, the one Plasma picks.
pub fn in_package(
    images: &[String],
    dark_images: &[String],
    screen: (u32, u32),
    dark: bool,
) -> Option<String> {
    let best = |folder: &str, names: &[String]| {
        names
            .iter()
            .filter_map(|name| Some((size_in_name(name)?, name)))
            .min_by(|(a, _), (b, _)| distance(*a, screen).total_cmp(&distance(*b, screen)))
            .map(|(_, name)| format!("{folder}/{name}"))
    };
    let dark_pick = if dark {
        best("images_dark", dark_images)
    } else {
        None
    };
    dark_pick.or_else(|| best("images", images))
}

/// `1920x1080.jxl` is 1920 by 1080; a name without a size is not a candidate.
fn size_in_name(name: &str) -> Option<(u32, u32)> {
    let stem = name.split('.').next()?;
    let (w, h) = stem.split_once('x')?;
    let size = (w.parse().ok()?, h.parse().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Plasma's own measure of how well a picture's size suits the screen, from
/// plasma-workspace's `packagefinder.cpp`: a different shape counts for a lot,
/// and a picture smaller than the screen twice what a larger one does, since it
/// has to be scaled up.
fn distance(size: (u32, u32), desired: (u32, u32)) -> f64 {
    let ratio = |(w, h): (u32, u32)| f64::from(w) / f64::from(h);
    let desired = if desired.0 == 0 || desired.1 == 0 {
        (1920, 1080)
    } else {
        desired
    };
    let delta = f64::from(size.0) - f64::from(desired.0);
    let delta = if delta >= 0.0 { delta } else { -delta * 2.0 };
    (ratio(size) - ratio(desired)).abs() * 25000.0 + delta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &str) -> Vec<String> {
        list.split_whitespace().map(str::to_owned).collect()
    }

    /// Fedora's package, as on the Plasma VM: the same sizes light and dark.
    fn fedora() -> Vec<String> {
        names(
            "1024x600.jxl 1024x768.jxl 1080x1920.jxl 1280x800.jxl 1366x768.jxl 1440x2960.jxl \
             1920x1080.jxl 1920x1200.jxl 2560x1440.jxl 2560x1600.jxl 3440x1440.jxl \
             3840x2160.jxl 5120x2880.jxl 800x600.jxl",
        )
    }

    #[test]
    fn a_package_gives_the_size_plasma_would_pick() {
        let fedora = fedora();
        assert_eq!(
            in_package(&fedora, &fedora, (3840, 2160), false).as_deref(),
            Some("images/3840x2160.jxl")
        );
        assert_eq!(
            in_package(&fedora, &fedora, (1920, 1200), false).as_deref(),
            Some("images/1920x1200.jxl")
        );
        // An odd size: the same shape wins over the nearest width.
        assert_eq!(
            in_package(&fedora, &fedora, (2195, 1235), false).as_deref(),
            Some("images/2560x1440.jxl")
        );
        // Next has three sizes, and on the Plasma VM's 3840 by 2160 screen
        // the desktop shows its 5120 by 2880 picture.
        let next = names("1440x2960.png 5120x2880.png 7680x2160.png");
        assert_eq!(
            in_package(&next, &next, (3840, 2160), true).as_deref(),
            Some("images_dark/5120x2880.png")
        );
    }

    #[test]
    fn the_dark_picture_is_used_only_when_dark_and_there() {
        let light = names("1920x1080.png");
        let dark = names("1920x1080.png");
        assert_eq!(
            in_package(&light, &dark, (1920, 1080), false).as_deref(),
            Some("images/1920x1080.png")
        );
        assert_eq!(
            in_package(&light, &dark, (1920, 1080), true).as_deref(),
            Some("images_dark/1920x1080.png")
        );
        assert_eq!(
            in_package(&light, &[], (1920, 1080), true).as_deref(),
            Some("images/1920x1080.png"),
            "a package without dark pictures"
        );
        assert_eq!(
            in_package(&names("screenshot.png"), &[], (1920, 1080), false),
            None
        );
    }

    #[test]
    fn the_default_comes_from_the_look_and_feel_kdeglobals_names() {
        // ~/.config/kdeglobals on the Plasma VM names Breeze Dark; Fedora's
        // own default, read after it, names Fedora's.
        let user = "[General]\nColorScheme=BreezeDark\n\n[KDE]\nLookAndFeelPackage=org.kde.breezedark.desktop\ncontrast=4\n";
        let fedora =
            "[KDE]\nLookAndFeelPackage=org.fedoraproject.fedora.desktop\nSingleClick=false\n";
        assert_eq!(look_and_feel([user, fedora]), "org.kde.breezedark.desktop");
        assert_eq!(
            look_and_feel(["[KDE]\ncontrast=4\n", fedora]),
            "org.fedoraproject.fedora.desktop"
        );
        assert_eq!(look_and_feel(["", "[General]\n"]), DEFAULT_LOOK_AND_FEEL);
        assert_eq!(
            default_wallpaper("[Wallpaper]\nImage=Next\n").as_deref(),
            Some("Next")
        );
        assert_eq!(
            default_wallpaper("[kdeglobals][KDE]\nwidgetStyle=Breeze\n"),
            None
        );
        assert_eq!(
            ini_value("[Wallpaper]\nImage[$e]=$HOME/a.png\n", "Wallpaper", "Image").as_deref(),
            Some("$HOME/a.png")
        );
    }

    #[test]
    fn what_the_image_plugin_shows() {
        // As wallpaper(0) answered on the Plasma VM: Image empty, FillMode 2.
        let vm = Settings {
            plugin: "org.kde.image".into(),
            image: String::new(),
            fill_mode: 2,
            color: Some(0xffff_ffff),
        };
        assert_eq!(shown(&vm), Ok(Shown::Default));
        assert_eq!(fit(&vm), Fit::Cover);
        assert_eq!(behind(&vm), Rgb(255, 255, 255));
        let package = Settings {
            image: "file:///usr/share/wallpapers/Next/".into(),
            ..vm.clone()
        };
        assert_eq!(
            shown(&package),
            Ok(Shown::Path("/usr/share/wallpapers/Next/".into()))
        );
        let picture = Settings {
            image: "/home/keith/Pictures/lake.jpg".into(),
            fill_mode: 1,
            ..vm.clone()
        };
        assert_eq!(
            shown(&picture),
            Ok(Shown::Path("/home/keith/Pictures/lake.jpg".into()))
        );
        assert_eq!(fit(&picture), Fit::Contain);
    }

    #[test]
    fn the_colour_plugin_is_its_colour_and_other_plugins_are_not_pictures() {
        let colour = Settings {
            plugin: "org.kde.color".into(),
            color: Some(0xff20_4a87),
            ..Settings::default()
        };
        assert_eq!(shown(&colour), Ok(Shown::Colour(Rgb(0x20, 0x4a, 0x87))));
        let slideshow = Settings {
            plugin: "org.kde.slideshow".into(),
            ..Settings::default()
        };
        assert!(matches!(shown(&slideshow), Err(Why::NotAPicture(_))));
        let other = Settings {
            plugin: "org.kde.potd".into(),
            ..Settings::default()
        };
        assert_eq!(
            shown(&other),
            Err(Why::NotAPicture("the wallpaper plugin org.kde.potd".into()))
        );
    }
}
