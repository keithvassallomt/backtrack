// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The desktop's wallpaper, asked for and made ready to show behind Rewind
//! (S18-T2).
//!
//! `model::wallpaper` works out what the desktop's settings mean. This is the
//! part that reads them: GSettings natively, dconf's files inside the
//! Flatpak, Plasma's shell over the session bus, and the colour scheme from
//! the Settings portal. Then the picture is decoded (GTK decodes JPEG XL,
//! Fedora's wallpapers, through glycin, natively and in the Flatpak alike),
//! drawn once at the screen's size and kept in the cache, so that the next
//! opening reads a picture already the right size.
//!
//! `backtrack-gtk --wallpaper-report` runs all of it and says what it found,
//! for checking a desktop by hand.

mod report;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use gtk4::{gdk, gdk_pixbuf, gio, glib};
use tracing::{debug, info};

use crate::host::Host;
use crate::model::wallpaper::{gnome, plasma, slideshow, Fit, Rgb, Wallpaper, Why};

pub use report::report;

/// The screen Rewind is on, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

impl Screen {
    /// The monitor's size in device pixels, at a scale of two at most. The
    /// background is dimmed behind Rewind's windows and needs no more, and the
    /// cap keeps a monitor that reports an outsized scale from asking for a
    /// picture of hundreds of megabytes: GTK gives 3.56 for a rotated 5K
    /// monitor on Hyprland at scale 2 (2026-10-27), the panel's width over the
    /// rotated width.
    pub fn of(monitor: &gdk::Monitor) -> Screen {
        let geometry = monitor.geometry();
        let scale = monitor.scale().clamp(1.0, 2.0);
        let pixels = |logical: i32| (f64::from(logical.max(1)) * scale).round() as u32;
        Screen {
            width: pixels(geometry.width()),
            height: pixels(geometry.height()),
        }
    }
}

/// Which desktop this is, from `XDG_CURRENT_DESKTOP`, which the Flatpak
/// passes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Plasma,
    Other(String),
}

fn desktop_of(names: &str) -> Desktop {
    for name in names.split(':') {
        match name.trim().to_ascii_lowercase().as_str() {
            "gnome" => return Desktop::Gnome,
            "kde" => return Desktop::Plasma,
            _ => {}
        }
    }
    Desktop::Other(names.to_owned())
}

/// What was found, and how, for the report.
#[derive(Debug, Clone)]
pub struct Found {
    pub wallpaper: Wallpaper,
    pub lines: Vec<String>,
}

/// What the desktop shows behind its windows, on a screen of `screen`.
pub async fn find(screen: Screen) -> Result<Found, (Why, Vec<String>)> {
    let host = Host::here();
    let desktop = desktop_of(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default());
    let mut lines = vec![format!(
        "Desktop: {}{}",
        match &desktop {
            Desktop::Gnome => "GNOME".to_owned(),
            Desktop::Plasma => "Plasma".to_owned(),
            Desktop::Other(name) if name.is_empty() => "not named".to_owned(),
            Desktop::Other(name) => name.clone(),
        },
        if host.flatpak { ", in the Flatpak" } else { "" }
    )];
    let connection = zbus::Connection::session().await.ok();
    let dark = match &connection {
        Some(connection) => prefers_dark(connection).await,
        None => None,
    };
    lines.push(format!(
        "Colour scheme: {}",
        match dark {
            Some(true) => "dark",
            Some(false) => "light, or no preference",
            None => "not known (no Settings portal), taken as light",
        }
    ));
    let dark = dark.unwrap_or(false);
    lines.push(format!("Screen: {} by {}", screen.width, screen.height));
    let found = match desktop {
        Desktop::Gnome => gnome(&host, screen, dark, &mut lines),
        Desktop::Plasma => match &connection {
            Some(connection) => plasma(&host, connection, screen, dark, &mut lines).await,
            None => Err(Why::Unreachable("there is no session bus".to_owned())),
        },
        Desktop::Other(name) => Err(Why::OtherDesktop(name)),
    };
    match found {
        Ok(wallpaper) => Ok(Found { wallpaper, lines }),
        Err(why) => Err((why, lines)),
    }
}

/// Whether the desktop's colour scheme is dark, from the Settings portal,
/// which GNOME and Plasma both answer, natively and in the Flatpak.
async fn prefers_dark(connection: &zbus::Connection) -> Option<bool> {
    let call = connection.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.Settings"),
        "ReadOne",
        &("org.freedesktop.appearance", "color-scheme"),
    );
    let reply = glib::future_with_timeout(Duration::from_secs(2), call)
        .await
        .ok()?
        .ok()?;
    let mut value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    // Some portals wrap the value in a further variant.
    while let zbus::zvariant::Value::Value(inner) = &*value {
        value = inner.try_to_owned().ok()?;
    }
    let scheme = u32::try_from(&value).ok()?;
    // 0 no preference, 1 prefer dark, 2 prefer light.
    Some(scheme == 1)
}

fn gnome(
    host: &Host,
    screen: Screen,
    dark: bool,
    lines: &mut Vec<String>,
) -> Result<Wallpaper, Why> {
    let settings = if host.flatpak {
        gnome_from_files(host, lines)
    } else {
        gnome_from_gsettings(lines)
    };
    let wallpaper = gnome::choose(&settings, dark)?;
    let Wallpaper::Picture { path, fit, behind } = wallpaper else {
        return Ok(wallpaper);
    };
    let here = host.path(&path);
    if here.extension().is_some_and(|e| e == "xml") {
        let text = std::fs::read_to_string(&here)
            .map_err(|e| Why::Unreadable(path.clone(), e.to_string()))?;
        let show = slideshow::parse(&text)
            .ok_or_else(|| Why::Unreadable(path.clone(), "not a slideshow".to_owned()))?;
        let now = glib::DateTime::now_local()
            .map_err(|e| Why::Unreadable(path.clone(), e.to_string()))?;
        let picture = show
            .picture_at(&now, (screen.width, screen.height))
            .ok_or_else(|| Why::Unreadable(path.clone(), "it shows nothing now".to_owned()))?;
        lines.push(format!(
            "Slideshow: {} shows {} now",
            path.display(),
            picture.display()
        ));
        return picture_at(host, picture, fit, behind);
    }
    picture_at(host, path, fit, behind)
}

/// A picture the desktop names, as read from here, if it is there.
fn picture_at(host: &Host, path: PathBuf, fit: Fit, behind: Rgb) -> Result<Wallpaper, Why> {
    let here = host.path(&path);
    if !here.is_file() {
        return Err(Why::Missing(path));
    }
    Ok(Wallpaper::Picture {
        path: here,
        fit,
        behind,
    })
}

fn gnome_from_gsettings(lines: &mut Vec<String>) -> gnome::Settings {
    let schema = gio::SettingsSchemaSource::default().and_then(|s| s.lookup(gnome::SCHEMA, true));
    let Some(schema) = schema else {
        lines.push(format!("Settings: no {} schema here", gnome::SCHEMA));
        return gnome::Settings::default();
    };
    let settings = gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
    lines.push("Settings: from GSettings".to_owned());
    gnome::Settings::read(|key| {
        let value = schema
            .has_key(key)
            .then(|| settings.string(key).to_string())?;
        lines.push(format!("  {key} = '{value}'"));
        Some(value)
    })
}

/// GNOME's settings from dconf's own files, in dconf's order, falling to the
/// defaults in the host's compiled schemas (where a distribution's vendor
/// override, as Fedora's wallpaper, is compiled in).
fn gnome_from_files(host: &Host, lines: &mut Vec<String>) -> gnome::Settings {
    let layers = host.dconf_layers();
    let schemas = host.schemas();
    let defaults = host.compiled_schema(gnome::SCHEMA);
    lines.push(format!(
        "Settings: from dconf's files ({} read), then {}",
        layers.databases.len(),
        if defaults.is_some() {
            format!("the defaults in {}", schemas.display())
        } else {
            format!(
                "nothing: no {} schema in {}",
                gnome::SCHEMA,
                schemas.display()
            )
        }
    ));
    gnome::Settings::read(|key| {
        let from_dconf = layers
            .read_from(&format!("{}{key}", gnome::PATH))
            .and_then(|(source, value)| Some((source.to_string(), value.str()?.to_owned())));
        let (from, value) = from_dconf.or_else(|| {
            let schema = defaults.as_ref()?;
            let value = schema
                .has_key(key)
                .then(|| schema.key(key).default_value())?;
            Some(("the schema's default".to_owned(), value.str()?.to_owned()))
        })?;
        lines.push(format!("  {key} = '{value}', from {from}"));
        Some(value)
    })
}

async fn plasma(
    host: &Host,
    connection: &zbus::Connection,
    screen: Screen,
    dark: bool,
    lines: &mut Vec<String>,
) -> Result<Wallpaper, Why> {
    let settings = plasma_settings(connection).await?;
    lines.push(format!(
        "Settings: wallpaper plugin {}, Image '{}', FillMode {}",
        settings.plugin, settings.image, settings.fill_mode
    ));
    let fit = plasma::fit(&settings);
    let behind = plasma::behind(&settings);
    let path = match plasma::shown(&settings)? {
        plasma::Shown::Colour(colour) => return Ok(Wallpaper::Colour(colour)),
        plasma::Shown::Path(path) => path,
        plasma::Shown::Default => {
            let kdeglobals: Vec<String> = std::iter::once(host.config.clone())
                .chain(host.config_dirs())
                .filter_map(|dir| std::fs::read_to_string(dir.join("kdeglobals")).ok())
                .collect();
            let look = plasma::look_and_feel(kdeglobals.iter().map(String::as_str));
            let defaults = std::iter::once(host.data.clone())
                .chain(host.data_dirs())
                .map(|dir| {
                    dir.join("plasma/look-and-feel")
                        .join(&look)
                        .join("contents/defaults")
                })
                .find_map(|path| std::fs::read_to_string(path).ok());
            let name = defaults
                .as_deref()
                .and_then(plasma::default_wallpaper)
                .ok_or_else(|| Why::Missing(format!("the wallpaper {look} names").into()))?;
            lines.push(format!(
                "Default: the look-and-feel package {look} names {name}"
            ));
            if name.starts_with('/') {
                PathBuf::from(name)
            } else {
                std::iter::once(host.data.clone())
                    .chain(host.data_dirs())
                    .map(|dir| dir.join("wallpapers").join(&name))
                    .find(|dir| dir.is_dir())
                    .ok_or_else(|| Why::Missing(format!("the wallpaper package {name}").into()))?
            }
        }
    };
    let here = host.path(&path);
    if here.is_dir() {
        let list = |folder: &str| -> Vec<String> {
            std::fs::read_dir(here.join("contents").join(folder))
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        let picked = plasma::in_package(
            &list("images"),
            &list("images_dark"),
            (screen.width, screen.height),
            dark,
        )
        .ok_or_else(|| Why::Missing(path.join("contents/images")))?;
        lines.push(format!("Package: {} gives {picked}", path.display()));
        return Ok(Wallpaper::Picture {
            path: here.join("contents").join(picked),
            fit,
            behind,
        });
    }
    picture_at(host, path, fit, behind)
}

/// What Plasma's shell says about the first screen's wallpaper.
///
/// Plasma numbers its screens its own way, and Rewind does not yet map the
/// monitor it is on to one of them, so the first screen's is shown.
async fn plasma_settings(connection: &zbus::Connection) -> Result<plasma::Settings, Why> {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};

    let call = connection.call_method(
        Some("org.kde.plasmashell"),
        "/PlasmaShell",
        Some("org.kde.PlasmaShell"),
        "wallpaper",
        &(0u32),
    );
    let reply = glib::future_with_timeout(Duration::from_secs(3), call)
        .await
        .map_err(|_| Why::Unreachable("Plasma's shell did not answer".to_owned()))?
        .map_err(|e| Why::Unreachable(e.to_string()))?;
    let map: HashMap<String, OwnedValue> = reply
        .body()
        .deserialize()
        .map_err(|e| Why::Unreachable(e.to_string()))?;
    let text = |key: &str| {
        map.get(key)
            .and_then(|v| <&str>::try_from(&**v).ok().map(str::to_owned))
            .unwrap_or_default()
    };
    // The colour comes as a struct of one ARGB number, Qt's QColor marshalled.
    let color = map.get("Color").and_then(|v| match &**v {
        Value::Structure(s) => s.fields().first().and_then(|f| u32::try_from(f).ok()),
        Value::U32(argb) => Some(*argb),
        _ => None,
    });
    Ok(plasma::Settings {
        plugin: text("wallpaperPlugin"),
        image: text("Image"),
        fill_mode: map
            .get("FillMode")
            .and_then(|v| i32::try_from(&**v).ok())
            .unwrap_or(2),
        color,
    })
}

/// The wallpaper, drawn at the screen's size.
#[derive(Debug)]
pub struct Drawn {
    pub texture: gdk::Texture,
    /// The cached picture it was read from, or written to.
    pub file: PathBuf,
    pub from_cache: bool,
    pub took: Duration,
}

/// Decode the wallpaper and draw it at the screen's size, or read the copy
/// drawn last time. Blocking: run it off the main thread.
pub fn draw(wallpaper: &Wallpaper, screen: Screen) -> Result<Drawn, Why> {
    let started = Instant::now();
    let folder = glib::user_cache_dir().join("backtrack").join("wallpaper");
    let (source, key) = match wallpaper {
        Wallpaper::Picture { path, fit, behind } => {
            let meta = std::fs::metadata(path).map_err(|_| Why::Missing(path.clone()))?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            let key = format!(
                "{}|{}.{}|{}|{fit:?}|{behind:?}|{}x{}",
                path.display(),
                modified.as_secs(),
                modified.subsec_nanos(),
                meta.len(),
                screen.width,
                screen.height
            );
            (Some((path, *fit, *behind)), key)
        }
        Wallpaper::Colour(colour) => (
            None,
            format!("{colour:?}|{}x{}", screen.width, screen.height),
        ),
    };
    let name = glib::compute_checksum_for_string(glib::ChecksumType::Sha256, &key)
        .map(|sum| sum[..24].to_owned())
        .unwrap_or_default();
    let file = folder.join(format!("{name}.png"));
    if let Ok(texture) = gdk::Texture::from_filename(&file) {
        return Ok(Drawn {
            texture,
            file,
            from_cache: true,
            took: started.elapsed(),
        });
    }

    let canvas = match source {
        Some((path, fit, behind)) => {
            let picture = gdk::Texture::from_filename(path)
                .map_err(|e| Why::Unreadable(path.clone(), e.message().to_owned()))?;
            let mut downloader = gdk::TextureDownloader::new(&picture);
            downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
            let (bytes, stride) = downloader.download_bytes();
            let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
                &bytes,
                gdk_pixbuf::Colorspace::Rgb,
                true,
                8,
                picture.width(),
                picture.height(),
                stride as i32,
            );
            drop(picture);
            fitted(&pixbuf, fit, behind, screen)
        }
        None => {
            let Wallpaper::Colour(colour) = wallpaper else {
                unreachable!("a picture has a source")
            };
            canvas(*colour, screen)
        }
    }
    .ok_or_else(|| Why::Unreadable(PathBuf::from("the drawing"), "out of memory".to_owned()))?;

    if std::fs::create_dir_all(&folder).is_ok() {
        forget_all_but_the_newest(&folder, 3);
        if let Err(error) = canvas.savev(&file, "png", &[]) {
            debug!(%error, "the drawn wallpaper could not be cached");
        }
    }
    let texture = gdk::MemoryTexture::new(
        canvas.width(),
        canvas.height(),
        gdk::MemoryFormat::R8g8b8,
        &canvas.read_pixel_bytes(),
        canvas.rowstride() as usize,
    )
    .upcast();
    info!(file = %file.display(), took = ?started.elapsed(), "the wallpaper was drawn");
    Ok(Drawn {
        texture,
        file,
        from_cache: false,
        took: started.elapsed(),
    })
}

/// A screen-sized canvas of one colour.
fn canvas(colour: Rgb, screen: Screen) -> Option<gdk_pixbuf::Pixbuf> {
    let canvas = gdk_pixbuf::Pixbuf::new(
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        screen.width as i32,
        screen.height as i32,
    )?;
    let Rgb(r, g, b) = colour;
    canvas.fill(u32::from_be_bytes([r, g, b, 0xff]));
    Some(canvas)
}

/// `picture` on a screen-sized canvas, as `fit` says, over `behind`.
fn fitted(
    picture: &gdk_pixbuf::Pixbuf,
    fit: Fit,
    behind: Rgb,
    screen: Screen,
) -> Option<gdk_pixbuf::Pixbuf> {
    let out = canvas(behind, screen)?;
    let (sw, sh) = (f64::from(screen.width), f64::from(screen.height));
    let (pw, ph) = (f64::from(picture.width()), f64::from(picture.height()));
    let interp = gdk_pixbuf::InterpType::Bilinear;
    // The picture placed at `scale` and offset, onto the part of the canvas it
    // covers.
    let place = |scale_x: f64, scale_y: f64, x: f64, y: f64| {
        let left = x.max(0.0).floor() as i32;
        let top = y.max(0.0).floor() as i32;
        let right = (x + pw * scale_x).min(sw).ceil() as i32;
        let bottom = (y + ph * scale_y).min(sh).ceil() as i32;
        if right > left && bottom > top {
            picture.composite(
                &out,
                left,
                top,
                right - left,
                bottom - top,
                x,
                y,
                scale_x,
                scale_y,
                interp,
                255,
            );
        }
    };
    match fit {
        Fit::Cover | Fit::Contain => {
            let scale = if fit == Fit::Cover {
                (sw / pw).max(sh / ph)
            } else {
                (sw / pw).min(sh / ph)
            };
            place(
                scale,
                scale,
                (sw - pw * scale) / 2.0,
                (sh - ph * scale) / 2.0,
            );
        }
        Fit::Stretch => place(sw / pw, sh / ph, 0.0, 0.0),
        Fit::Center => place(
            1.0,
            1.0,
            ((sw - pw) / 2.0).round(),
            ((sh - ph) / 2.0).round(),
        ),
        Fit::Tile => {
            let mut y = 0.0;
            while y < sh {
                let mut x = 0.0;
                while x < sw {
                    place(1.0, 1.0, x, y);
                    x += pw;
                }
                y += ph;
            }
        }
    }
    Some(out)
}

/// Keep the cache to the few pictures drawn last: one per screen size in
/// use, and the one before a change of wallpaper.
fn forget_all_but_the_newest(folder: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut pictures: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let path = e.path();
            (path.extension()? == "png").then_some(())?;
            Some((e.metadata().ok()?.modified().ok()?, path))
        })
        .collect();
    pictures.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in pictures.into_iter().skip(keep.saturating_sub(1)) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_is_named_by_any_part_of_xdg_current_desktop() {
        assert_eq!(desktop_of("GNOME"), Desktop::Gnome);
        assert_eq!(desktop_of("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(desktop_of("KDE"), Desktop::Plasma);
        assert_eq!(desktop_of("Hyprland"), Desktop::Other("Hyprland".into()));
        assert_eq!(desktop_of(""), Desktop::Other(String::new()));
    }

    #[test]
    fn inside_the_flatpak_the_hosts_usr_and_etc_are_under_run_host() {
        let flatpak = Host {
            flatpak: true,
            config: "/home/keith/.config".into(),
            data: "/home/keith/.local/share".into(),
        };
        assert_eq!(
            flatpak.path(Path::new("/usr/share/backgrounds/a.jxl")),
            Path::new("/run/host/usr/share/backgrounds/a.jxl")
        );
        assert_eq!(
            flatpak.path(Path::new("/etc/dconf/db/local")),
            Path::new("/run/host/etc/dconf/db/local")
        );
        assert_eq!(
            flatpak.path(Path::new("/home/keith/Pictures/a.jpg")),
            Path::new("/home/keith/Pictures/a.jpg")
        );
        let native = Host {
            flatpak: false,
            ..flatpak
        };
        assert_eq!(
            native.path(Path::new("/usr/share/backgrounds/a.jxl")),
            Path::new("/usr/share/backgrounds/a.jxl")
        );
    }

    #[test]
    fn a_vendors_default_is_read_from_the_compiled_schemas() {
        // Compiled from the fixture schema with a distribution's override, as
        // Fedora's wallpaper is set: the default is the override's.
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/model/wallpaper/testdata/schemas"
        );
        let source = gio::SettingsSchemaSource::from_directory(dir, None, false).unwrap();
        let schema = source.lookup(gnome::SCHEMA, false).unwrap();
        let default = |key: &str| schema.key(key).default_value().str().unwrap().to_owned();
        assert_eq!(
            default("picture-uri"),
            "file:///usr/share/backgrounds/f45/default/f45-01-day.jxl"
        );
        assert_eq!(
            default("picture-uri-dark"),
            "file:///usr/share/backgrounds/f45/default/f45-01-night.jxl"
        );
        assert_eq!(default("picture-options"), "zoom", "not overridden");
    }

    fn picture(width: i32, height: i32) -> gdk_pixbuf::Pixbuf {
        // Left half red, right half blue.
        let p =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, width, height).unwrap();
        p.fill(0xff00_00ff);
        let half = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, width / 2, height)
            .unwrap();
        half.fill(0x0000_ffff);
        half.copy_area(0, 0, width / 2, height, &p, width / 2, 0);
        p
    }

    fn pixel(p: &gdk_pixbuf::Pixbuf, x: i32, y: i32) -> (u8, u8, u8) {
        let bytes = p.read_pixel_bytes();
        let at = (y * p.rowstride() + x * p.n_channels()) as usize;
        (bytes[at], bytes[at + 1], bytes[at + 2])
    }

    const RED: (u8, u8, u8) = (255, 0, 0);
    const BLUE: (u8, u8, u8) = (0, 0, 255);
    const GREY: (u8, u8, u8) = (40, 40, 40);

    #[test]
    fn cover_fills_the_screen_and_contain_leaves_the_colour_behind_around_it() {
        let screen = Screen {
            width: 160,
            height: 90,
        };
        // A square picture on a wide screen.
        let square = picture(100, 100);
        let cover = fitted(&square, Fit::Cover, Rgb(40, 40, 40), screen).unwrap();
        assert_eq!((cover.width(), cover.height()), (160, 90));
        assert_eq!(pixel(&cover, 2, 45), RED, "reaches the left edge");
        assert_eq!(pixel(&cover, 157, 45), BLUE, "and the right");
        let contain = fitted(&square, Fit::Contain, Rgb(40, 40, 40), screen).unwrap();
        assert_eq!(pixel(&contain, 10, 45), GREY, "bars at the sides");
        assert_eq!(pixel(&contain, 60, 45), RED);
        assert_eq!(pixel(&contain, 100, 45), BLUE);
        assert_eq!(pixel(&contain, 150, 45), GREY);
    }

    #[test]
    fn centre_and_tile_keep_the_pictures_own_size() {
        let screen = Screen {
            width: 100,
            height: 40,
        };
        let small = picture(20, 10);
        let centred = fitted(&small, Fit::Center, Rgb(40, 40, 40), screen).unwrap();
        assert_eq!(pixel(&centred, 5, 5), GREY);
        assert_eq!(pixel(&centred, 42, 20), RED);
        assert_eq!(pixel(&centred, 57, 20), BLUE);
        let tiled = fitted(&small, Fit::Tile, Rgb(40, 40, 40), screen).unwrap();
        assert_eq!(pixel(&tiled, 2, 2), RED);
        assert_eq!(pixel(&tiled, 12, 2), BLUE);
        assert_eq!(pixel(&tiled, 22, 32), RED, "repeated across and down");
        let stretched = fitted(&small, Fit::Stretch, Rgb(40, 40, 40), screen).unwrap();
        assert_eq!(pixel(&stretched, 10, 35), RED);
        assert_eq!(pixel(&stretched, 90, 5), BLUE);
    }
}
