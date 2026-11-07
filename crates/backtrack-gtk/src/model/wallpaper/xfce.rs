// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Xfce's wallpaper (S18-T19), from its desktop's settings as xfconf writes
//! them to `xfce4-desktop.xml`: properties nested by path, the picture for
//! each monitor and workspace at
//! `/backdrop/screen0/monitor<connector>/workspace<n>/last-image`.
//!
//! Which workspace is shown is not in the file: the first is taken, or the
//! one `single-workspace-number` names when every workspace shows one.

use std::collections::HashMap;
use std::path::PathBuf;

use backtrack_core::xml::{self, TagKind};

use super::{Fit, Rgb, Wallpaper};

/// Every property with a value, by its path; an array's values joined with
/// commas, as `rgba1`'s four numbers.
fn properties(text: &str) -> (HashMap<String, String>, Vec<String>) {
    let mut values = HashMap::new();
    let mut order = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut array: Option<(String, Vec<String>)> = None;
    for tag in xml::tags(text) {
        match (tag.name, tag.kind) {
            ("property", TagKind::Open | TagKind::Empty) => {
                let name = tag.attribute("name").unwrap_or_default();
                let full = format!("{}/{name}", path.join(""));
                order.push(full.clone());
                if let Some(value) = tag.attribute("value") {
                    values.insert(full.clone(), value);
                }
                if tag.kind == TagKind::Open {
                    if tag.attribute("type").as_deref() == Some("array") {
                        array = Some((full, Vec::new()));
                    }
                    path.push(format!("/{name}"));
                }
            }
            ("property", TagKind::Close) => {
                path.pop();
                if let Some((full, items)) = array.take() {
                    values.insert(full, items.join(","));
                }
            }
            ("value", TagKind::Empty) => {
                if let (Some((_, items)), Some(value)) = (&mut array, tag.attribute("value")) {
                    items.push(value);
                }
            }
            _ => {}
        }
    }
    (values, order)
}

/// What Xfce shows on `output` (its connector, such as `eDP-1`), or on the
/// first monitor the settings name.
pub fn choose(text: &str, output: Option<&str>) -> Option<Wallpaper> {
    let (values, order) = properties(text);
    let screen = "/backdrop/screen0/";
    let monitor = output
        .map(|output| format!("monitor{output}"))
        .filter(|monitor| {
            values
                .keys()
                .any(|k| k.starts_with(&format!("{screen}{monitor}/")))
        })
        .or_else(|| {
            order.iter().find_map(|path| {
                let rest = path.strip_prefix(screen)?;
                (rest.starts_with("monitor") && !rest.contains('/')).then(|| rest.to_owned())
            })
        })?;
    let single = values
        .get("/backdrop/single-workspace-mode")
        .map(String::as_str)
        == Some("true");
    let workspace = single
        .then(|| values.get("/backdrop/single-workspace-number"))
        .flatten()
        .cloned()
        .unwrap_or_else(|| "0".to_owned());
    let at = |key: &str| values.get(&format!("{screen}{monitor}/workspace{workspace}/{key}"));
    let behind = at("rgba1")
        .and_then(|rgba| {
            let parts: Vec<f64> = rgba.split(',').filter_map(|v| v.parse().ok()).collect();
            let channel = |i: usize| Some((parts.get(i)?.clamp(0.0, 1.0) * 255.0).round() as u8);
            Some(Rgb(channel(0)?, channel(1)?, channel(2)?))
        })
        .unwrap_or(Rgb(0, 0, 0));
    let fit = match at("image-style").map(String::as_str) {
        Some("0") => return Some(Wallpaper::Colour(behind)),
        Some("1") => Fit::Center,
        Some("2") => Fit::Tile,
        Some("3") => Fit::Stretch,
        Some("4") => Fit::Contain,
        // 5 zoomed, the default, and 6 spanning every monitor.
        _ => Fit::Cover,
    };
    let path = at("last-image").filter(|p| !p.is_empty())?;
    Some(Wallpaper::Picture {
        path: PathBuf::from(path),
        fit,
        behind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As xfconf writes it, cut down: two monitors, two workspaces.
    const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>

<channel name="xfce4-desktop" version="1.0">
  <property name="backdrop" type="empty">
    <property name="screen0" type="empty">
      <property name="monitorVirtual-1" type="empty">
        <property name="workspace0" type="empty">
          <property name="color-style" type="int" value="0"/>
          <property name="image-style" type="int" value="5"/>
          <property name="last-image" type="string" value="/usr/share/backgrounds/xfce/xfce-shapes.svg"/>
        </property>
        <property name="workspace1" type="empty">
          <property name="image-style" type="int" value="3"/>
          <property name="last-image" type="string" value="/home/k/second.png"/>
        </property>
      </property>
      <property name="monitoreDP-1" type="empty">
        <property name="workspace0" type="empty">
          <property name="rgba1" type="array">
            <value type="double" value="0.2"/>
            <value type="double" value="0.4"/>
            <value type="double" value="0.6"/>
            <value type="double" value="1"/>
          </property>
          <property name="image-style" type="int" value="0"/>
          <property name="last-image" type="string" value="/home/k/hidden.png"/>
        </property>
      </property>
    </property>
  </property>
</channel>
"#;

    fn picture(chosen: Option<Wallpaper>) -> Option<(PathBuf, Fit)> {
        match chosen? {
            Wallpaper::Picture { path, fit, .. } => Some((path, fit)),
            Wallpaper::Colour(_) => None,
        }
    }

    #[test]
    fn the_monitors_first_workspace_is_read() {
        assert_eq!(
            picture(choose(SETTINGS, Some("Virtual-1"))),
            Some((
                PathBuf::from("/usr/share/backgrounds/xfce/xfce-shapes.svg"),
                Fit::Cover
            ))
        );
        assert_eq!(
            picture(choose(SETTINGS, None)),
            picture(choose(SETTINGS, Some("Virtual-1"))),
            "not knowing the monitor, the first named"
        );
        assert_eq!(
            picture(choose(SETTINGS, Some("HDMI-1"))),
            picture(choose(SETTINGS, Some("Virtual-1"))),
            "a monitor it has nothing for, the first named"
        );
    }

    #[test]
    fn with_one_picture_for_every_workspace_that_one_is_read() {
        let single = SETTINGS.replace(
            "  <property name=\"backdrop\" type=\"empty\">\n",
            "  <property name=\"backdrop\" type=\"empty\">\n\
             <property name=\"single-workspace-mode\" type=\"bool\" value=\"true\"/>\n\
             <property name=\"single-workspace-number\" type=\"int\" value=\"1\"/>\n",
        );
        assert_eq!(
            picture(choose(&single, Some("Virtual-1"))),
            Some((PathBuf::from("/home/k/second.png"), Fit::Stretch))
        );
    }

    #[test]
    fn no_picture_is_its_colour() {
        assert_eq!(
            choose(SETTINGS, Some("eDP-1")),
            Some(Wallpaper::Colour(Rgb(51, 102, 153)))
        );
        assert_eq!(choose("<channel name=\"xfce4-desktop\"/>", None), None);
    }
}
