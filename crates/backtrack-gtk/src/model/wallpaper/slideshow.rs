// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! GNOME's timed wallpapers: a wallpaper setting can name an XML file that
//! shows one picture after another through the day, from a start time,
//! round and round. Rewind shows the picture the desktop is showing now; in a
//! transition between two, whichever of them it is closer to.

use std::path::PathBuf;

use backtrack_core::xml;
use gtk4::glib;

/// A slideshow's pictures and when they show.
#[derive(Debug, Clone, PartialEq)]
pub struct Slideshow {
    /// Year, month, day, hour, minute, second, in the desktop's own zone.
    start: [i32; 6],
    slides: Vec<Slide>,
}

#[derive(Debug, Clone, PartialEq)]
struct Slide {
    seconds: f64,
    /// The picture shown, or for a transition the pictures it is between.
    from: Picture,
    to: Option<Picture>,
}

/// A picture, in one size or in several.
#[derive(Debug, Clone, PartialEq)]
struct Picture {
    sizes: Vec<(Option<(u32, u32)>, PathBuf)>,
}

impl Picture {
    fn read(body: &str) -> Option<Picture> {
        let sizes: Vec<_> = xml::children(body)
            .filter(|e| e.name == "size")
            .filter_map(|e| {
                let width = e.attribute("width")?.parse().ok()?;
                let height = e.attribute("height")?.parse().ok()?;
                Some((
                    Some((width, height)),
                    PathBuf::from(xml::unescape(e.body.trim())?),
                ))
            })
            .collect();
        if !sizes.is_empty() {
            return Some(Picture { sizes });
        }
        let path = xml::unescape(body.trim())?;
        (!path.is_empty()).then(|| Picture {
            sizes: vec![(None, PathBuf::from(path))],
        })
    }

    /// The size that covers the screen with the least to spare, or the
    /// largest when none covers it.
    fn for_screen(&self, screen: (u32, u32)) -> PathBuf {
        let covering = self
            .sizes
            .iter()
            .filter(|(size, _)| size.is_some_and(|(w, h)| w >= screen.0 && h >= screen.1))
            .min_by_key(|(size, _)| size.map(|(w, h)| u64::from(w) * u64::from(h)));
        let largest = self
            .sizes
            .iter()
            .max_by_key(|(size, _)| size.map_or(0, |(w, h)| u64::from(w) * u64::from(h)));
        covering
            .or(largest)
            .map(|(_, path)| path.clone())
            .unwrap_or_default()
    }
}

/// Read a slideshow's XML. None when it is not one.
pub fn parse(text: &str) -> Option<Slideshow> {
    let body = xml::element(text, "background")?;
    let mut start = None;
    let mut slides = Vec::new();
    for element in xml::children(body) {
        let field = |name: &str| {
            xml::children(element.body)
                .find(|e| e.name == name)
                .map(|e| e.body)
        };
        match element.name {
            "starttime" => {
                let mut parts = [0; 6];
                for (part, name) in parts
                    .iter_mut()
                    .zip(["year", "month", "day", "hour", "minute", "second"])
                {
                    *part = field(name)?.trim().parse().ok()?;
                }
                start = Some(parts);
            }
            "static" => slides.push(Slide {
                seconds: field("duration")?.trim().parse().ok()?,
                from: Picture::read(field("file")?)?,
                to: None,
            }),
            "transition" => slides.push(Slide {
                seconds: field("duration")?.trim().parse().ok()?,
                from: Picture::read(field("from")?)?,
                to: Some(Picture::read(field("to")?)?),
            }),
            _ => {}
        }
    }
    let total: f64 = slides.iter().map(|s| s.seconds).sum();
    (total > 0.0 && slides.iter().all(|s| s.seconds >= 0.0)).then_some(Slideshow {
        start: start?,
        slides,
    })
}

impl Slideshow {
    /// The picture shown at `now`, in the size best for `screen`. Before the
    /// start, the first picture.
    pub fn picture_at(&self, now: &glib::DateTime, screen: (u32, u32)) -> Option<PathBuf> {
        let [year, month, day, hour, minute, second] = self.start;
        let start = glib::DateTime::new(
            &now.timezone(),
            year,
            month,
            day,
            hour,
            minute,
            f64::from(second),
        )
        .ok()?;
        let total: f64 = self.slides.iter().map(|s| s.seconds).sum();
        let elapsed = now.difference(&start).as_seconds() as f64;
        let mut into = if elapsed < 0.0 { 0.0 } else { elapsed % total };
        for slide in &self.slides {
            if into < slide.seconds {
                let picture = match &slide.to {
                    Some(to) if into >= slide.seconds / 2.0 => to,
                    _ => &slide.from,
                };
                return Some(picture.for_screen(screen));
            }
            into -= slide.seconds;
        }
        self.slides.last().map(|s| s.from.for_screen(screen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UHD: (u32, u32) = (3840, 2160);

    fn show() -> Slideshow {
        let text = include_str!("testdata/day-and-night.xml");
        parse(text).unwrap()
    }

    fn at(day: i32, hour: i32, minute: i32) -> glib::DateTime {
        glib::DateTime::new(&glib::TimeZone::utc(), 2026, 10, day, hour, minute, 0.0).unwrap()
    }

    fn name(path: Option<PathBuf>) -> String {
        path.unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn the_picture_follows_the_time_of_day() {
        let show = show();
        assert_eq!(name(show.picture_at(&at(27, 12, 0), UHD)), "day.jxl");
        // The dusk transition runs from 18:00 to 19:00: the picture it comes
        // from for its first half, the one it goes to for its second.
        assert_eq!(name(show.picture_at(&at(27, 18, 20), UHD)), "day.jxl");
        assert_eq!(name(show.picture_at(&at(27, 18, 40), UHD)), "night.jxl");
        assert_eq!(
            name(show.picture_at(&at(27, 23, 0), UHD)),
            "night-3840x2160.jxl"
        );
        assert_eq!(
            name(show.picture_at(&at(28, 5, 0), UHD)),
            "night-3840x2160.jxl"
        );
        // Dawn, 06:00 to 07:00, then the day again.
        assert_eq!(name(show.picture_at(&at(28, 6, 45), UHD)), "day.jxl");
        assert_eq!(name(show.picture_at(&at(28, 7, 30), UHD)), "day.jxl");
    }

    #[test]
    fn the_size_that_covers_the_screen_with_the_least_to_spare_is_chosen() {
        let show = show();
        let night = at(27, 23, 0);
        assert_eq!(
            name(show.picture_at(&night, (1920, 1080))),
            "night-1920x1080.jxl"
        );
        assert_eq!(
            name(show.picture_at(&night, (2560, 1440))),
            "night-3840x2160.jxl"
        );
        // Nothing covers it: the largest there is.
        assert_eq!(
            name(show.picture_at(&night, (7680, 4320))),
            "night-5120x2880.jxl"
        );
    }

    #[test]
    fn before_its_start_a_slideshow_shows_its_first_picture() {
        let early = glib::DateTime::new(&glib::TimeZone::utc(), 2025, 6, 1, 23, 0, 0.0).unwrap();
        assert_eq!(name(show().picture_at(&early, UHD)), "day.jxl");
    }

    #[test]
    fn what_is_not_a_slideshow_is_not_read_as_one() {
        assert_eq!(parse("<background></background>"), None);
        assert_eq!(parse("<wallpapers><wallpaper/></wallpapers>"), None);
        assert_eq!(
            parse(
                "<background><starttime><year>2026</year></starttime>\
                 <static><duration>10</duration><file>/a.png</file></static></background>"
            ),
            None,
            "a start time without its hour, minute and second"
        );
    }
}
