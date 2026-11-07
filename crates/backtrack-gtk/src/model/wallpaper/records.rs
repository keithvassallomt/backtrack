// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the programs that draw the wallpaper keep of it, on desktops where
//! the compositor draws none itself, such as Hyprland (S18-T18).
//!
//! A shell or a wallpaper program draws the picture there, and each keeps a
//! record of it in a file of its own, read here as S18-T15 found each one
//! written. On Hyprland, the background layer of a monitor names the program
//! drawing it, which says whose record to read; elsewhere, and in the
//! Flatpak, which cannot reach Hyprland, the record written last is taken.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Fit, Rgb, Wallpaper};

/// A program that draws the wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Caelestia,
    /// swww's name since 0.12.
    Awww,
    Swww,
    Wpaperd,
    Hyprpaper,
    Waypaper,
    Noctalia,
    DankMaterialShell,
    /// Keeps no record: its picture is on its command line, which only a
    /// program outside the Flatpak can read.
    Swaybg,
}

impl Program {
    pub fn name(self) -> &'static str {
        match self {
            Program::Caelestia => "Caelestia",
            Program::Awww => "awww",
            Program::Swww => "swww",
            Program::Wpaperd => "wpaperd",
            Program::Hyprpaper => "hyprpaper",
            Program::Waypaper => "waypaper",
            Program::Noctalia => "Noctalia",
            Program::DankMaterialShell => "DankMaterialShell",
            Program::Swaybg => "swaybg",
        }
    }

    /// The program behind a background layer, from the layer's name or,
    /// failing that, its process's: the shells all run as Quickshell, so
    /// only their layers tell them apart.
    pub fn of_layer(namespace: &str, process: &str) -> Option<Program> {
        let namespace = namespace.to_ascii_lowercase();
        let by_name = [
            ("caelestia", Program::Caelestia),
            ("noctalia", Program::Noctalia),
            ("dms", Program::DankMaterialShell),
            ("dank", Program::DankMaterialShell),
            ("hyprpaper", Program::Hyprpaper),
            ("awww", Program::Awww),
            ("swww", Program::Swww),
            ("wpaperd", Program::Wpaperd),
        ];
        if let Some((_, program)) = by_name.iter().find(|(name, _)| namespace.contains(name)) {
            return Some(*program);
        }
        match process {
            "swaybg" => Some(Program::Swaybg),
            "hyprpaper" => Some(Program::Hyprpaper),
            "awww-daemon" | "awww" => Some(Program::Awww),
            "swww-daemon" | "swww" => Some(Program::Swww),
            "wpaperd" => Some(Program::Wpaperd),
            _ => None,
        }
    }
}

/// A wallpaper as a program's record gives it, and when that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub program: Program,
    pub wallpaper: Wallpaper,
    pub written: Option<SystemTime>,
}

/// The record written last, where no layer says whose to read.
pub fn newest(records: Vec<Record>) -> Option<Record> {
    records.into_iter().max_by_key(|record| record.written)
}

/// A picture, covering the screen unless the record says otherwise.
fn picture(path: PathBuf, fit: Fit) -> Wallpaper {
    Wallpaper::Picture {
        path,
        fit,
        behind: Rgb(0, 0, 0),
    }
}

/// `~/` and `$HOME/` at the start of a path, as the programs' own configs
/// write them.
fn expand(path: &str, home: &Path) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("$HOME/"))
    {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Caelestia's `wallpaper/path.txt`: the picture's path, one picture on every
/// screen.
pub fn caelestia(text: &str) -> Option<Wallpaper> {
    let path = text.trim();
    path.starts_with('/')
        .then(|| picture(PathBuf::from(path), Fit::Cover))
}

/// awww's cache file for an output: entries of four fields, each ended by a
/// NUL but the last: namespace, resize (with `:gravity`), filter, path. The
/// entry of the default namespace, else the first.
pub fn awww(bytes: &[u8]) -> Option<Wallpaper> {
    let fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let entries = fields.as_chunks::<4>().0;
    let entry = entries
        .iter()
        .find(|e| e[0].is_empty())
        .or_else(|| entries.first())?;
    let resize = std::str::from_utf8(entry[1]).ok()?;
    let path = std::str::from_utf8(entry[3]).ok()?;
    let fit = match resize.split(':').next() {
        Some("fit") => Fit::Contain,
        Some("stretch") => Fit::Stretch,
        Some("no") => Fit::Center,
        _ => Fit::Cover,
    };
    (!path.is_empty()).then(|| picture(PathBuf::from(path), fit))
}

/// swww's cache file for an output, before 0.12: entries of three fields,
/// each ended by a NUL: namespace, filter, path.
pub fn swww(bytes: &[u8]) -> Option<Wallpaper> {
    let fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let entries = fields.as_chunks::<3>().0;
    let entry = entries
        .iter()
        .find(|e| e[0].is_empty())
        .or_else(|| entries.first())?;
    let path = std::str::from_utf8(entry[2]).ok()?;
    (!path.is_empty()).then(|| picture(PathBuf::from(path), Fit::Cover))
}

/// hyprpaper's config: `wallpaper { monitor = … path = … fit_mode = … }`
/// since 0.8, `wallpaper = monitor,path` before it. The output's own, else
/// the one for every monitor (an empty `monitor`). A `path` may be a folder,
/// a slideshow whose current picture the config does not say; whoever reads
/// the picture finds that out.
pub fn hyprpaper(text: &str, output: Option<&str>, home: &Path) -> Option<Wallpaper> {
    let mut found: Vec<(String, String, Option<String>)> = Vec::new();
    let mut block: Option<(String, String, Option<String>)> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if let Some(open) = &mut block {
            if line.starts_with('}') {
                found.extend(block.take());
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim().to_owned();
                match key.trim() {
                    "monitor" => open.0 = value,
                    "path" => open.1 = value,
                    "fit_mode" => open.2 = Some(value),
                    _ => {}
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            if line.starts_with("wallpaper") && line.ends_with('{') {
                block = Some(Default::default());
            }
            continue;
        };
        if key.trim() == "wallpaper" {
            let value = value.trim();
            if value == "{" {
                block = Some(Default::default());
            } else if let Some((monitor, path)) = value.split_once(',') {
                // An old config may put the fit before the path: `contain:…`.
                let (fit, path) = match path.split_once(':') {
                    Some((fit, rest)) if !fit.contains('/') => (Some(fit.to_owned()), rest),
                    _ => (None, path),
                };
                found.push((monitor.trim().to_owned(), path.trim().to_owned(), fit));
            }
        }
    }
    let chosen = output
        .and_then(|output| found.iter().rev().find(|(monitor, _, _)| monitor == output))
        .or_else(|| {
            found
                .iter()
                .rev()
                .find(|(monitor, _, _)| monitor.is_empty())
        })?;
    let fit = match chosen.2.as_deref() {
        Some("contain") => Fit::Contain,
        Some("tile") => Fit::Tile,
        Some("fill") => Fit::Stretch,
        _ => Fit::Cover,
    };
    (!chosen.1.is_empty()).then(|| picture(expand(&chosen.1, home), fit))
}

/// waypaper's `config.ini` (`[Settings]`) or `state.ini` (`[State]`): its
/// last pictures and the monitors they went to, one to a line, `All` for
/// every monitor.
pub fn waypaper(text: &str, output: Option<&str>, home: &Path) -> Option<Wallpaper> {
    let mut values: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            // A continuation of the value above, as Python's configparser
            // writes a value of several lines.
            if let Some((_, value)) = values.last_mut() {
                value.push('\n');
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once('=') {
            values.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let value = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let pictures: Vec<&str> = value("wallpaper")?
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    let monitors: Vec<&str> = value("monitors").unwrap_or("All").lines().collect();
    let index = output
        .and_then(|output| monitors.iter().position(|m| *m == output))
        .unwrap_or(0);
    let path = pictures.get(index).or_else(|| pictures.first())?;
    let fit = match value("fill") {
        Some("fit") => Fit::Contain,
        Some("stretch") => Fit::Stretch,
        Some("center") => Fit::Center,
        Some("tile") => Fit::Tile,
        _ => Fit::Cover,
    };
    Some(picture(expand(path, home), fit))
}

/// Noctalia's settings, its own `settings.toml` last, as it reads them: the
/// last that sets `[wallpaper.default] path` wins. A colour is written
/// `color:#rrggbb`. Its per-monitor settings name folders, not pictures.
pub fn noctalia(texts: &[&str], home: &Path) -> Option<Wallpaper> {
    let mut path = None;
    let mut fill = None;
    for text in texts {
        let Ok(value) = text.parse::<toml::Table>() else {
            continue;
        };
        let wallpaper = value.get("wallpaper");
        if let Some(p) = wallpaper
            .and_then(|w| w.get("default"))
            .and_then(|d| d.get("path"))
            .and_then(|p| p.as_str())
        {
            path = Some(p.to_owned());
        }
        if let Some(f) = wallpaper
            .and_then(|w| w.get("fill_mode"))
            .and_then(|f| f.as_str())
        {
            fill = Some(f.to_owned());
        }
    }
    let path = path.filter(|p| !p.is_empty())?;
    if let Some(colour) = path.strip_prefix("color:") {
        return Rgb::parse(colour).map(Wallpaper::Colour);
    }
    let fit = match fill.as_deref() {
        Some("center") => Fit::Center,
        Some("fit") => Fit::Contain,
        Some("stretch") => Fit::Stretch,
        Some("repeat") => Fit::Tile,
        _ => Fit::Cover,
    };
    Some(picture(expand(&path, home), fit))
}

/// DankMaterialShell's `session.json`: `wallpaperPath`, or with
/// `perMonitorWallpaper` the output's entry in `monitorWallpapers`. A colour
/// is written `#rrggbb`.
pub fn dank_material_shell(text: &str, output: Option<&str>) -> Option<Wallpaper> {
    let session: serde_json::Value = serde_json::from_str(text).ok()?;
    let per_monitor = session
        .get("perMonitorWallpaper")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let own = per_monitor
        .then(|| {
            let output = output?;
            session.get("monitorWallpapers")?.get(output)?.as_str()
        })
        .flatten()
        .filter(|v| !v.is_empty());
    let value = own.or_else(|| {
        session
            .get("wallpaperPath")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
    })?;
    if value.starts_with('#') {
        return Rgb::parse(value).map(Wallpaper::Colour);
    }
    Some(picture(PathBuf::from(value), Fit::Cover))
}

/// swaybg's command line: `-o` chooses an output (`*` for all) for the
/// options after it, before any `-o` they are for all; `-i` the picture,
/// `-m` the mode, `-c` the colour.
pub fn swaybg(args: &[String], output: Option<&str>) -> Option<Wallpaper> {
    #[derive(Default, Clone)]
    struct Set {
        image: Option<String>,
        mode: Option<String>,
        colour: Option<String>,
    }
    let mut sets: Vec<(String, Set)> = vec![("*".to_owned(), Set::default())];
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next().cloned());
        match flag {
            "-o" | "--output" => {
                let name = value().unwrap_or_default();
                if !sets.iter().any(|(n, _)| *n == name) {
                    sets.push((name.clone(), Set::default()));
                }
                // Options that follow are for this output.
                let at = sets.iter().position(|(n, _)| *n == name).unwrap_or(0);
                let chosen = sets.remove(at);
                sets.push(chosen);
            }
            "-i" | "--image" => sets.last_mut()?.1.image = value(),
            "-m" | "--mode" => sets.last_mut()?.1.mode = value(),
            "-c" | "--color" => sets.last_mut()?.1.colour = value(),
            _ => {}
        }
    }
    let set = output
        .and_then(|output| sets.iter().find(|(n, _)| n == output))
        .or_else(|| sets.iter().find(|(n, _)| n == "*"))
        .map(|(_, set)| set.clone())?;
    let behind = set
        .colour
        .as_deref()
        .and_then(|c| Rgb::parse(&format!("#{}", c.trim_start_matches('#'))))
        .unwrap_or(Rgb(0, 0, 0));
    let fit = match set.mode.as_deref() {
        Some("solid_color") => return Some(Wallpaper::Colour(behind)),
        Some("fill") => Fit::Cover,
        Some("fit") => Fit::Contain,
        Some("center") => Fit::Center,
        Some("tile") => Fit::Tile,
        _ => Fit::Stretch,
    };
    let path = set.image?;
    Some(Wallpaper::Picture {
        path: PathBuf::from(path),
        fit,
        behind,
    })
}

/// A background layer on a Hyprland monitor: whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub monitor: String,
    pub namespace: String,
    pub pid: i32,
}

/// The background layer of `output`, else of the first monitor, from
/// Hyprland's `j/layers`: monitors, each with layers by level, level 0 the
/// background. The largest, where there are several.
pub fn background(layers: &str, output: Option<&str>) -> Option<Layer> {
    let monitors: serde_json::Map<String, serde_json::Value> = serde_json::from_str(layers).ok()?;
    let (monitor, levels) = output
        .and_then(|output| monitors.get_key_value(output))
        .or_else(|| monitors.iter().next())?;
    let area = |layer: &serde_json::Value| {
        let side = |key| {
            layer
                .get(key)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        side("w") * side("h")
    };
    let layer = levels
        .get("levels")?
        .get("0")?
        .as_array()?
        .iter()
        .max_by_key(|layer| area(layer))?;
    Some(Layer {
        monitor: monitor.clone(),
        namespace: layer.get("namespace")?.as_str()?.to_owned(),
        pid: layer
            .get("pid")
            .and_then(serde_json::Value::as_i64)
            .and_then(|pid| i32::try_from(pid).ok())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/k")
    }

    fn path_of(wallpaper: Option<Wallpaper>) -> Option<(PathBuf, Fit)> {
        match wallpaper? {
            Wallpaper::Picture { path, fit, .. } => Some((path, fit)),
            Wallpaper::Colour(_) => None,
        }
    }

    #[test]
    fn caelestia_names_one_picture() {
        // As on Keith's laptop, with no newline after it.
        assert_eq!(
            path_of(caelestia(
                "/home/keith/Pictures/wallpaper/space-with-ships.png"
            )),
            Some((
                PathBuf::from("/home/keith/Pictures/wallpaper/space-with-ships.png"),
                Fit::Cover
            ))
        );
        assert_eq!(caelestia(""), None);
    }

    #[test]
    fn awww_and_swww_keep_one_entry_a_namespace() {
        let awww_file =
            b"\0fit:center\0Lanczos3\0/home/k/a.png\0lock\0crop\0Nearest\0/home/k/b.jpg";
        assert_eq!(
            path_of(awww(awww_file)),
            Some((PathBuf::from("/home/k/a.png"), Fit::Contain))
        );
        let named_only = b"lock\0no\0Nearest\0/home/k/b.jpg";
        assert_eq!(
            path_of(awww(named_only)),
            Some((PathBuf::from("/home/k/b.jpg"), Fit::Center))
        );
        let swww_file = b"\0Lanczos3\0/home/k/c.png\0lock\0Nearest\0/home/k/d.png\0";
        assert_eq!(
            path_of(swww(swww_file)),
            Some((PathBuf::from("/home/k/c.png"), Fit::Cover))
        );
        assert_eq!(awww(b""), None);
        assert_eq!(swww(b"\0"), None);
    }

    #[test]
    fn hyprpaper_reads_both_its_syntaxes() {
        let blocks = "preload = ~/x.png\n\
            wallpaper {\n    monitor = DP-3\n    path = ~/dp3.jxl\n    fit_mode = contain\n}\n\
            wallpaper {\n    monitor =\n    path = ~/all.png\n}\n";
        assert_eq!(
            path_of(hyprpaper(blocks, Some("DP-3"), &home())),
            Some((PathBuf::from("/home/k/dp3.jxl"), Fit::Contain))
        );
        assert_eq!(
            path_of(hyprpaper(blocks, Some("eDP-1"), &home())),
            Some((PathBuf::from("/home/k/all.png"), Fit::Cover))
        );
        // Keith's, from before 0.8.
        let old = "preload = $HOME/.config/hypr/assets/blank.png\n\
            wallpaper = ,$HOME/.config/hypr/assets/blank.png\nsplash = false\n";
        assert_eq!(
            path_of(hyprpaper(old, Some("eDP-1"), &home())),
            Some((
                PathBuf::from("/home/k/.config/hypr/assets/blank.png"),
                Fit::Cover
            ))
        );
        let fitted = "wallpaper = eDP-1,contain:~/a.png\n";
        assert_eq!(
            path_of(hyprpaper(fitted, Some("eDP-1"), &home())),
            Some((PathBuf::from("/home/k/a.png"), Fit::Contain))
        );
        assert_eq!(hyprpaper("splash = false\n", None, &home()), None);
    }

    #[test]
    fn waypaper_matches_pictures_to_monitors() {
        let config = "[Settings]\nlanguage = en\n\
            wallpaper = ~/Pictures/left.jpg\n\t~/Pictures/right.jpg\n\
            monitors = DP-1\n\tHDMI-A-1\nfill = fit\nbackend = swww\n";
        assert_eq!(
            path_of(waypaper(config, Some("HDMI-A-1"), &home())),
            Some((PathBuf::from("/home/k/Pictures/right.jpg"), Fit::Contain))
        );
        let everywhere = "[State]\nwallpaper = /srv/a.png\nmonitors = All\n";
        assert_eq!(
            path_of(waypaper(everywhere, Some("DP-1"), &home())),
            Some((PathBuf::from("/srv/a.png"), Fit::Cover))
        );
        assert_eq!(waypaper("[Settings]\nfill = fit\n", None, &home()), None);
    }

    #[test]
    fn noctalias_own_settings_win_over_its_config() {
        let config =
            "[wallpaper]\nfill_mode = \"repeat\"\n[wallpaper.default]\npath = \"~/a.png\"\n";
        let settings = "[wallpaper.default]\npath = \"/home/k/chosen.png\"\n";
        assert_eq!(
            path_of(noctalia(&[config, settings], &home())),
            Some((PathBuf::from("/home/k/chosen.png"), Fit::Tile))
        );
        assert_eq!(
            noctalia(
                &["[wallpaper.default]\npath = \"color:#1a1a2e\"\n"],
                &home()
            ),
            Some(Wallpaper::Colour(Rgb(0x1a, 0x1a, 0x2e)))
        );
        assert_eq!(noctalia(&["[bar]\nx = 1\n"], &home()), None);
    }

    #[test]
    fn dank_material_shell_reads_the_outputs_own_where_it_keeps_one() {
        let session = r##"{"wallpaperPath": "/home/k/all.jpg",
            "perMonitorWallpaper": true,
            "monitorWallpapers": {"DP-2": "/home/k/dp2.jpg", "eDP-1": "#102030"}}"##;
        assert_eq!(
            path_of(dank_material_shell(session, Some("DP-2"))),
            Some((PathBuf::from("/home/k/dp2.jpg"), Fit::Cover))
        );
        assert_eq!(
            dank_material_shell(session, Some("eDP-1")),
            Some(Wallpaper::Colour(Rgb(0x10, 0x20, 0x30)))
        );
        assert_eq!(
            path_of(dank_material_shell(session, Some("HDMI-A-1"))),
            Some((PathBuf::from("/home/k/all.jpg"), Fit::Cover))
        );
        let shared = r#"{"wallpaperPath": "/home/k/all.jpg", "monitorWallpapers": {"DP-2": "/x"}}"#;
        assert_eq!(
            path_of(dank_material_shell(shared, Some("DP-2"))),
            Some((PathBuf::from("/home/k/all.jpg"), Fit::Cover)),
            "without perMonitorWallpaper, one for all"
        );
    }

    #[test]
    fn swaybgs_options_follow_the_output_before_them() {
        let args: Vec<String> = [
            "swaybg",
            "-m",
            "fill",
            "-i",
            "/all.png",
            "-o",
            "DP-1",
            "-i",
            "/dp1.png",
            "-m",
            "fit",
            "--output=eDP-1",
            "--mode=solid_color",
            "--color=#334455",
            // Back to every output: what follows is for all of them again.
            "-o",
            "*",
            "-m",
            "center",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            path_of(swaybg(&args, Some("DP-1"))),
            Some((PathBuf::from("/dp1.png"), Fit::Contain))
        );
        assert_eq!(
            path_of(swaybg(&args, Some("HDMI-A-1"))),
            Some((PathBuf::from("/all.png"), Fit::Center))
        );
        assert_eq!(
            swaybg(&args, Some("eDP-1")),
            Some(Wallpaper::Colour(Rgb(0x33, 0x44, 0x55)))
        );
    }

    #[test]
    fn the_background_layer_says_who_draws_it() {
        // `hyprctl layers -j` on Keith's laptop, cut down.
        let layers = r#"{"eDP-1": {"levels": {
            "0": [{"address": "0x1", "x": 0, "y": 0, "w": 2000, "h": 1250,
                   "namespace": "caelestia-background", "pid": 2024}],
            "2": [{"address": "0x2", "x": 0, "y": 0, "w": 2, "h": 1,
                   "namespace": "quickshell", "pid": 2024}]}}}"#;
        let layer = background(layers, Some("eDP-1")).unwrap();
        assert_eq!(
            layer,
            Layer {
                monitor: "eDP-1".into(),
                namespace: "caelestia-background".into(),
                pid: 2024
            }
        );
        assert_eq!(
            Program::of_layer(&layer.namespace, "qs"),
            Some(Program::Caelestia)
        );
        assert_eq!(
            Program::of_layer("wallpaper", "swaybg"),
            Some(Program::Swaybg)
        );
        assert_eq!(Program::of_layer("", "awww-daemon"), Some(Program::Awww));
        assert_eq!(Program::of_layer("wallpaper", "something-else"), None);
        assert_eq!(
            background(r#"{"eDP-1": {"levels": {"2": []}}}"#, None),
            None
        );
    }

    #[test]
    fn without_a_layer_the_record_written_last_is_taken() {
        let at = |s: u64| Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s));
        let record = |program, written| Record {
            program,
            wallpaper: Wallpaper::Colour(Rgb(0, 0, 0)),
            written,
        };
        let newest_one = newest(vec![
            record(Program::Hyprpaper, at(100)),
            record(Program::Caelestia, at(300)),
            record(Program::Waypaper, at(200)),
        ]);
        assert_eq!(newest_one.map(|r| r.program), Some(Program::Caelestia));
        assert_eq!(newest(Vec::new()), None);
    }
}
