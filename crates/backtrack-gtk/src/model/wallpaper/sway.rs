// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Sway's wallpaper (S18-T20), from its config: `output <name|*> bg <file>
//! <mode> [<colour>]`, or a `bg` line in an `output <name> { … }` block, the
//! last one read for a monitor winning, `output *` where a monitor has none
//! of its own. Sway keeps no other record of it: its IPC does not say, and a
//! change made with `swaymsg` at run time is in no file.
//!
//! A config is read as Sway reads it: comments dropped, a line ended by `\`
//! joined to the next, `set $name value` substituted in the lines after it,
//! and `include` taking in other files where it stands. Which files an
//! `include` names is the caller's to find out (`read`).

use std::path::{Path, PathBuf};

use super::{Fit, Rgb, Wallpaper};

/// The configs Sway looks for, in its order: the first that is there is the
/// one it reads.
pub fn configs(home: &Path, config: &Path) -> [PathBuf; 6] {
    [
        home.join(".sway/config"),
        config.join("sway/config"),
        home.join(".i3/config"),
        config.join("i3/config"),
        PathBuf::from("/etc/sway/config"),
        PathBuf::from("/etc/i3/config"),
    ]
}

/// How deep `include`s are followed, against one that includes itself.
const DEEPEST_INCLUDE: usize = 8;

/// A line's words, as Sway splits them: on spaces, quotes keeping a word
/// together.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// The config's commands, its includes in their places and its variables
/// substituted, each as its words.
fn commands(
    text: &str,
    read: &dyn Fn(&str) -> Vec<String>,
    variables: &mut Vec<(String, String)>,
    depth: usize,
    out: &mut Vec<Vec<String>>,
) {
    let mut joined = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(head) = line.strip_suffix('\\') {
            joined.push_str(head);
            joined.push(' ');
            continue;
        }
        joined.push_str(line);
        let line = std::mem::take(&mut joined);
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Longer names first, so that `$wall` does not eat `$wallpaper`; in a
        // `set`, only in its value, as Sway does.
        let substituted = |text: &str| {
            let mut text = text.to_owned();
            for (name, value) in variables.iter() {
                text = text.replace(name.as_str(), value);
            }
            text
        };
        let plain = words(line);
        if plain.first().map(String::as_str) == Some("set")
            && plain.len() >= 3
            && plain[1].starts_with('$')
        {
            let value = substituted(&plain[2..].join(" "));
            variables.push((plain[1].clone(), value));
            variables.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
            continue;
        }
        let words = words(&substituted(line));
        match words.first().map(String::as_str) {
            Some("include") if words.len() >= 2 && depth < DEEPEST_INCLUDE => {
                for included in read(&words[1..].join(" ")) {
                    commands(&included, read, variables, depth + 1, out);
                }
            }
            Some(_) => out.push(words),
            None => {}
        }
    }
}

/// What `bg <file> <mode> [<colour>]` makes of a monitor.
fn background(args: &[String], home: &Path) -> Option<Wallpaper> {
    let file = args.first()?;
    let mode = args.get(1).map(String::as_str).unwrap_or("stretch");
    let colour = |text: &str| Rgb::parse(&format!("#{}", text.trim_start_matches('#')));
    if mode == "solid_color" {
        return colour(file).map(Wallpaper::Colour);
    }
    let behind = args.get(2).and_then(|c| colour(c)).unwrap_or(Rgb(0, 0, 0));
    let fit = match mode {
        "fill" => Fit::Cover,
        "fit" => Fit::Contain,
        "center" => Fit::Center,
        "tile" => Fit::Tile,
        _ => Fit::Stretch,
    };
    let path = match file.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(file),
    };
    Some(Wallpaper::Picture { path, fit, behind })
}

/// What `output` (its connector, such as `eDP-1`) shows, as the config at
/// `text` says, `read` giving the text of each file an `include` names.
pub fn choose(
    text: &str,
    output: Option<&str>,
    home: &Path,
    read: &dyn Fn(&str) -> Vec<String>,
) -> Option<Wallpaper> {
    let mut lines = Vec::new();
    commands(text, read, &mut Vec::new(), 0, &mut lines);
    // Each monitor's backgrounds, in the order read; a block's `bg` lines
    // belong to the monitor it opened with.
    let mut found: Vec<(String, Wallpaper)> = Vec::new();
    let mut block: Option<String> = None;
    for words in &lines {
        if let Some(name) = &block {
            if words.first().map(String::as_str) == Some("}") {
                block = None;
            } else if let Some(at) = words.iter().position(|w| w == "bg" || w == "background") {
                found.extend(background(&words[at + 1..], home).map(|w| (name.clone(), w)));
            }
            continue;
        }
        if words.first().map(String::as_str) != Some("output") || words.len() < 3 {
            continue;
        }
        let name = words[1].clone();
        if words.last().map(String::as_str) == Some("{") {
            block = Some(name);
            continue;
        }
        if let Some(at) = words.iter().position(|w| w == "bg" || w == "background") {
            found.extend(background(&words[at + 1..], home).map(|w| (name, w)));
        }
    }
    let last_for = |name: &str| found.iter().rev().find(|(n, _)| n == name);
    output
        .and_then(last_for)
        .or_else(|| last_for("*"))
        .map(|(_, wallpaper)| wallpaper.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/k")
    }

    fn none(_: &str) -> Vec<String> {
        Vec::new()
    }

    fn picture(chosen: Option<Wallpaper>) -> Option<(PathBuf, Fit)> {
        match chosen? {
            Wallpaper::Picture { path, fit, .. } => Some((path, fit)),
            Wallpaper::Colour(_) => None,
        }
    }

    #[test]
    fn every_monitors_line_and_a_monitors_own() {
        let config = "# Read `man 5 sway` for a complete reference.\n\
            set $wallpaper /usr/share/backgrounds/default.png\n\
            output * bg $wallpaper fill\n\
            output eDP-1 resolution 1920x1080 position 0,0 bg ~/own.jpg fit\n";
        assert_eq!(
            picture(choose(config, Some("eDP-1"), &home(), &none)),
            Some((PathBuf::from("/home/k/own.jpg"), Fit::Contain))
        );
        assert_eq!(
            picture(choose(config, Some("DP-2"), &home(), &none)),
            Some((
                PathBuf::from("/usr/share/backgrounds/default.png"),
                Fit::Cover
            ))
        );
    }

    #[test]
    fn includes_are_read_where_they_stand_and_the_last_line_wins() {
        let config =
            "set $dir /srv/walls\ninclude /etc/sway/config.d/*\noutput * bg $dir/late.png center\n";
        let read = |pattern: &str| {
            assert_eq!(pattern, "/etc/sway/config.d/*");
            vec!["output * bg $dir/early.png fill\noutput HDMI-A-1 \\\n    bg \"/srv/walls/with space.png\" tile\n".to_owned()]
        };
        assert_eq!(
            picture(choose(config, Some("eDP-1"), &home(), &read)),
            Some((PathBuf::from("/srv/walls/late.png"), Fit::Center)),
            "the later line, after the include"
        );
        assert_eq!(
            picture(choose(config, Some("HDMI-A-1"), &home(), &read)),
            Some((PathBuf::from("/srv/walls/with space.png"), Fit::Tile)),
            "joined across `\\`, quoted"
        );
    }

    #[test]
    fn a_block_and_a_colour() {
        let config = "output DP-1 {\n    mode 2560x1440@144Hz\n    bg #285577 solid_color\n}\noutput * bg /a.png stretch #000000\n";
        assert_eq!(
            choose(config, Some("DP-1"), &home(), &none),
            Some(Wallpaper::Colour(Rgb(0x28, 0x55, 0x77)))
        );
        assert_eq!(
            picture(choose(config, Some("eDP-1"), &home(), &none)),
            Some((PathBuf::from("/a.png"), Fit::Stretch))
        );
        assert_eq!(choose("bar { position top }\n", None, &home(), &none), None);
    }

    #[test]
    fn a_line_put_out_of_use_in_a_block_is_not_read() {
        let config = "output DP-1 {\n    bg /now.png fill\n    # bg /before.png fit\n}\n";
        assert_eq!(
            picture(choose(config, Some("DP-1"), &home(), &none)),
            Some((PathBuf::from("/now.png"), Fit::Cover))
        );
    }

    #[test]
    fn a_config_that_includes_itself_is_read_a_few_times_over() {
        let config = "include ~/.config/sway/config\noutput * bg /a.png fill\n";
        let read = |_: &str| vec![config.to_owned()];
        assert_eq!(
            picture(choose(config, None, &home(), &read)),
            Some((PathBuf::from("/a.png"), Fit::Cover))
        );
    }

    #[test]
    fn longer_variables_are_substituted_first() {
        let config = "set $wall /x\nset $wallpaper /y.png\noutput * bg $wallpaper fill\n";
        assert_eq!(
            picture(choose(config, None, &home(), &none)),
            Some((PathBuf::from("/y.png"), Fit::Cover))
        );
    }
}
