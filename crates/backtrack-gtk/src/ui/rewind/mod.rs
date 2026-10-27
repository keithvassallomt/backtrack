// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind, the full-screen way through the history (Stage 18).
//!
//! The folder as it is now in front, its backups receding behind it, the
//! desktop's wallpaper (or the starfield) behind them, as mockups 25 and 26
//! show. It opens over Backtrack's window, at the folder and backup being
//! viewed there, on the same monitor, and Escape returns to it as it was.
//!
//! The backups are those listed for the folder (Stage 15), newest first after
//! the folder as it is now. Each one's folder is read from the catalogue, the
//! backup in front first; the folder as it is now is read from the disk.

mod card;
mod stage;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use backtrack_core::index::{Entry, Kind};
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, Align, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{debug, warn};

use crate::index::Index;
use crate::model::{rewind, trash};
use crate::state::View;
use card::Content;
use stage::{Background, Stage};

/// Open Rewind over `parent`, at what it shows in `view`.
pub fn open(parent: &adw::ApplicationWindow, view: &View, index: &Index) {
    let window = adw::Window::builder()
        .title("Rewind")
        .transient_for(parent)
        .build();
    window.add_css_class("rewind");

    let tz = glib::TimeZone::local();
    let folder_name = folder_name(&view.folder);
    let mut slots = vec![Content {
        folder: folder_name.clone(),
        when: "Today (Now)".to_owned(),
        entries: None,
    }];
    slots.extend(view.listed.iter().map(|backup| {
        Content {
            folder: folder_name.clone(),
            when: crate::model::format::at(backup.ts, &tz, "%a %e %b %Y, %H:%M")
                .trim_start()
                .to_owned(),
            entries: None,
        }
    }));
    let at = view
        .seq
        .and_then(|seq| view.listed.iter().position(|a| a.seq == seq))
        .map_or(0, |i| i + 1);

    let stage = Stage::new();
    stage.set_slots(slots, at);
    stage.set_background(Background::Waiting);

    let when = Label::new(None);
    when.add_css_class("rewind-when");
    let where_ = Label::new(None);
    where_.add_css_class("rewind-where");
    let lines = gtk4::Box::new(Orientation::Vertical, 2);
    lines.set_halign(Align::Center);
    lines.set_valign(Align::End);
    lines.set_margin_bottom(26);
    lines.append(&when);
    lines.append(&where_);
    lines.set_can_target(false);

    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&stage));
    overlay.add_overlay(&lines);
    window.set_content(Some(&overlay));

    let view = Rc::new(view.clone());
    let say = {
        let view = Rc::clone(&view);
        let (when, where_) = (when.clone(), where_.clone());
        move |index: usize| {
            let backup = index.checked_sub(1).and_then(|i| view.listed.get(i));
            let (first, second) = rewind::lines(
                index,
                backup.map(|b| b.ts),
                view.listed.len(),
                view.is_filtered(),
                backup.is_some_and(|b| trash::is_entry(b.seq)),
                &glib::TimeZone::local(),
            );
            when.set_text(&first);
            where_.set_text(&second);
        }
    };
    say(at);

    keys(&window, &stage, say, parent);

    let monitor = parent
        .surface()
        .and_then(|surface| WidgetExt::display(parent).monitor_at_surface(&surface));
    match &monitor {
        Some(monitor) => window.fullscreen_on_monitor(monitor),
        None => window.fullscreen(),
    }
    window.present();

    read_folders(&stage, &view, index.clone(), at);
    find_background(&stage, monitor);
}

/// The keys, and the mouse wheel: Up and Down (and Page Up and Page Down) step
/// back and forward, Home goes to now and End to the oldest, Escape leaves.
fn keys(
    window: &adw::Window,
    stage: &Stage,
    say: impl Fn(usize) + 'static,
    parent: &adw::ApplicationWindow,
) {
    let say = Rc::new(say);
    // Keys held down: GTK reports each repeat as another press, and a press
    // of a key already down is a repeat.
    let held: Rc<RefCell<HashSet<u32>>> = Rc::default();
    let controller = gtk4::EventControllerKey::new();
    {
        let (stage, held, say) = (stage.clone(), Rc::clone(&held), Rc::clone(&say));
        let (window, parent) = (window.clone(), parent.clone());
        controller.connect_key_pressed(move |_, key, code, _| {
            let repeat = !held.borrow_mut().insert(code);
            let to = match key {
                gdk::Key::Up | gdk::Key::KP_Up | gdk::Key::Page_Up | gdk::Key::KP_Page_Up => {
                    stage.step(1, repeat)
                }
                gdk::Key::Down
                | gdk::Key::KP_Down
                | gdk::Key::Page_Down
                | gdk::Key::KP_Page_Down => stage.step(-1, repeat),
                gdk::Key::Home | gdk::Key::KP_Home => stage.go_to(0),
                gdk::Key::End | gdk::Key::KP_End => stage.go_to(stage.oldest()),
                gdk::Key::Escape => {
                    window.close();
                    parent.present();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            say(to);
            glib::Propagation::Stop
        });
    }
    {
        let held = Rc::clone(&held);
        controller.connect_key_released(move |_, _, code, _| {
            held.borrow_mut().remove(&code);
        });
    }
    window.add_controller(controller);

    // A notch of the wheel, or the same distance on a touchpad, is a step;
    // like a held key, it runs at most two backups ahead of the stack.
    let wheel = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
    let travelled = Rc::new(std::cell::Cell::new(0.0));
    {
        let stage = stage.clone();
        wheel.connect_scroll(move |_, _, dy| {
            travelled.set(travelled.get() + dy);
            if travelled.get().abs() >= 1.0 {
                let steps = if travelled.get() < 0.0 { 1 } else { -1 };
                travelled.set(0.0);
                say(stage.step(steps, true));
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(wheel);
}

/// Read what the folder held at each backup, nearest the one in front first,
/// and the folder as it is now from the disk.
fn read_folders(stage: &Stage, view: &Rc<View>, index: Index, at: usize) {
    let stage = stage.downgrade();
    let view = Rc::clone(view);
    crate::ui::spawn(async move {
        let count = view.listed.len() + 1;
        // Outwards from the one in front: at, at+1, at-1, at+2, …
        let order: Vec<usize> = (0..count * 2)
            .map(|k| {
                if k % 2 == 0 {
                    at + k / 2
                } else {
                    at.wrapping_sub(k / 2 + 1)
                }
            })
            .filter(|slot| *slot < count)
            .collect();
        for slot in order {
            let entries = if slot == 0 {
                let folder = crate::path::to_filesystem(&view.folder);
                gio::spawn_blocking(move || read_disk(&folder))
                    .await
                    .unwrap_or_else(|_| Err("the folder could not be read".to_owned()))
            } else {
                let backup = &view.listed[slot - 1];
                if !backup.catalogued {
                    continue;
                }
                let seq = match trash::base_of(backup.seq) {
                    // A Trash entry shows its backup before the gap; S18-T4
                    // puts what went to the Trash back in it.
                    Some(base) => base,
                    None => Some(backup.seq),
                };
                match seq {
                    Some(seq) => {
                        let folder = view.folder.clone();
                        index
                            .query(move |reader| reader.folder_at(&folder, seq))
                            .await
                    }
                    None => Ok(Vec::new()),
                }
            };
            let Some(stage) = stage.upgrade() else {
                return;
            };
            match entries {
                Ok(mut entries) => {
                    entries.sort_by(crate::model::folders_first);
                    stage.set_entries(slot, entries);
                }
                Err(error) => warn!(slot, %error, "a folder in Rewind could not be read"),
            }
        }
    });
}

/// The folder on the disk now, as entries: names and kinds, which is all a
/// card shows.
fn read_disk(folder: &std::path::Path) -> Result<Vec<Entry>, String> {
    let entries = std::fs::read_dir(folder).map_err(|e| e.to_string())?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| {
            let kind = match entry.file_type() {
                Ok(t) if t.is_dir() => Kind::Dir,
                Ok(t) if t.is_symlink() => Kind::Symlink,
                _ => Kind::File,
            };
            Entry {
                path_id: 0,
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
                size: 0,
                mtime: 0,
                deleted_after: false,
                changed_since: false,
            }
        })
        .collect())
}

/// The desktop's wallpaper behind the stack (S18-T2), or the starfield.
fn find_background(stage: &Stage, monitor: Option<gdk::Monitor>) {
    let stage = stage.downgrade();
    crate::ui::spawn(async move {
        let screen = monitor.as_ref().map_or(
            crate::wallpaper::Screen {
                width: 1920,
                height: 1080,
            },
            crate::wallpaper::Screen::of,
        );
        let background = match crate::wallpaper::find(screen).await {
            Ok(found) => {
                let wallpaper = found.wallpaper;
                match gio::spawn_blocking(move || crate::wallpaper::draw(&wallpaper, screen)).await
                {
                    Ok(Ok(drawn)) => {
                        // Darkened more in dark style, as the prototype does.
                        let dim = if adw::StyleManager::default().is_dark() {
                            0.5
                        } else {
                            0.3
                        };
                        Background::Wallpaper {
                            texture: drawn.texture,
                            dim,
                        }
                    }
                    Ok(Err(why)) => {
                        debug!(%why, "Rewind shows the starfield");
                        Background::Stars
                    }
                    Err(_) => Background::Stars,
                }
            }
            Err((why, _)) => {
                debug!(%why, "Rewind shows the starfield");
                Background::Stars
            }
        };
        if let Some(stage) = stage.upgrade() {
            stage.set_background(background);
        }
    });
}

/// The folder's own name, as its card's header bar says it.
fn folder_name(folder: &str) -> String {
    let home = crate::path::to_archive(&glib::home_dir());
    if folder == home {
        return "Home".to_owned();
    }
    match crate::path::name(folder) {
        "" => "Computer".to_owned(),
        name => name.to_owned(),
    }
}
