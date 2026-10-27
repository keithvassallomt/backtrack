// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! `backtrack-gtk --wallpaper-report`: a command, run from a terminal, whose
//! output is what it is for. It finds the desktop's wallpaper as Rewind would,
//! draws it, and prints how each step went, so a desktop can be checked by
//! hand. The one file of the window's that prints (see
//! `scripts/check_prints.py`).

use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

use super::{draw, find, Screen};
use crate::model::wallpaper::{Rgb, Wallpaper};

/// `backtrack-gtk --wallpaper-report`: find the wallpaper as Rewind would,
/// draw it, and say how it went.
pub fn report() -> glib::ExitCode {
    if let Err(error) = gtk4::init() {
        eprintln!("Backtrack could not reach the display: {error}");
        return glib::ExitCode::FAILURE;
    }
    let Some(monitor) = gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_then(|m| m.downcast::<gdk::Monitor>().ok())
    else {
        eprintln!("Backtrack could not find a monitor.");
        return glib::ExitCode::FAILURE;
    };
    let screen = Screen::of(&monitor);
    glib::MainContext::default().block_on(async move {
        match find(screen).await {
            Ok(found) => {
                for line in &found.lines {
                    println!("{line}");
                }
                println!("Wallpaper: {}", describe(&found.wallpaper));
                let wallpaper = found.wallpaper.clone();
                match gio::spawn_blocking(move || draw(&wallpaper, screen)).await {
                    Ok(Ok(drawn)) => {
                        println!(
                            "Drawn: {} by {}, {} in {:.2} s: {}",
                            drawn.texture.width(),
                            drawn.texture.height(),
                            if drawn.from_cache {
                                "read from the cache"
                            } else {
                                "decoded and drawn"
                            },
                            drawn.took.as_secs_f64(),
                            drawn.file.display()
                        );
                        glib::ExitCode::SUCCESS
                    }
                    Ok(Err(why)) => {
                        println!("Not drawn: {why}. Rewind would show the starfield.");
                        glib::ExitCode::FAILURE
                    }
                    Err(_) => {
                        println!("Not drawn: the drawing failed. Rewind would show the starfield.");
                        glib::ExitCode::FAILURE
                    }
                }
            }
            Err((why, lines)) => {
                for line in &lines {
                    println!("{line}");
                }
                println!("Not found: {why}. Rewind would show the starfield.");
                glib::ExitCode::FAILURE
            }
        }
    })
}

fn describe(wallpaper: &Wallpaper) -> String {
    match wallpaper {
        Wallpaper::Picture { path, fit, behind } => {
            let Rgb(r, g, b) = behind;
            format!("{} ({fit:?}, over #{r:02x}{g:02x}{b:02x})", path.display())
        }
        Wallpaper::Colour(Rgb(r, g, b)) => format!("the colour #{r:02x}{g:02x}{b:02x}"),
    }
}
