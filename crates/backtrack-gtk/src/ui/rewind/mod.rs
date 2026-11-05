// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Rewind, the full-screen way through the history (Stage 18).
//!
//! The folder as it is now in front, its backups receding behind it, the
//! desktop's wallpaper (or the starfield) behind them, as mockups 25 and 26
//! show. It opens over Backtrack's window, at the folder, backup and file
//! being viewed there, on the same monitor.
//!
//! Rewind moves through folders and backups in a state of its own, forked
//! from the window's (S18-T4), so that walking into folders lists their
//! backups and lands on them exactly as the window does, while the window
//! stays where it is underneath. Leaving gives the window Rewind's folder
//! and selection.
//!
//! The backups are those listed for the folder (Stage 15), newest first after
//! the folder as it is now. Each is listed as the window's file pane lists it
//! (`files::Lister`), the backup in front first; the folder as it is now is
//! read from the disk.

mod folder;
mod stage;
mod timeline;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use backtrack_core::index::Kind;
use gtk4::prelude::*;
use gtk4::{gdk, gdk_pixbuf, gio, glib, Align, Label, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{debug, warn};

use crate::index::Index;
use crate::model::group::{self, Kept};
use crate::model::{rewind, thumbnail, timeline as timeline_model, trash};
use crate::state::{AppState, Change, View};
use crate::ui::files::{Listed, Lister, Row};
use folder::{Actions, Body, Content, Mode};
use stage::{Background, Stage};
use timeline::{Tick, Timeline};

// How the windows showed their folders when Rewind was last left: icons
// until the list is chosen, then the list, for as long as Backtrack runs.
thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Icons) };
}

/// The rows of a backup whose pictures are looked for in the desktop's cache
/// as it is listed: about as many as a card shows. The window in front looks
/// for the rest as they come into sight.
const PICTURES_AHEAD: usize = 64;

/// Which of the folder's times a slot shows: the folder as it is now, or a
/// backup.
type When = Option<i64>;

struct Rewind {
    state: Rc<AppState>,
    stage: Stage,
    /// The backups down the right-hand edge (S18-T5).
    timeline: Timeline,
    lister: Rc<Lister>,
    index: Index,
    /// What each time held, as listed, for the folder being viewed: kept
    /// while the listing changes, so that only what is new is listed again.
    bodies: RefCell<HashMap<When, Body>>,
    /// The time each slot shows, front first.
    slots: RefCell<Vec<When>>,
    /// Counts the folder's listings, so that a late answer to an earlier one
    /// is dropped.
    generation: Cell<u64>,
    /// Pictures being made for the window in front, so each is made once.
    making: RefCell<HashSet<(String, i64)>>,
    /// The desktop's thumbnail cache.
    thumbnails: std::path::PathBuf,
}

/// Open Rewind over `parent`, at what `window` (its state) shows, listing
/// folders with `lister`.
pub fn open(
    parent: &adw::ApplicationWindow,
    window: &Rc<AppState>,
    index: &Index,
    lister: Rc<Lister>,
) {
    let shell = adw::Window::builder()
        .title("Rewind")
        .transient_for(parent)
        .build();
    shell.add_css_class("rewind");

    let state = window.fork();
    let stage = Stage::new();
    stage.set_background(Background::Waiting);
    stage.set_show_hidden(state.view().show_hidden);
    stage.set_mode(MODE.get());
    stage.set_selected(state.view().selected.map(|s| s.name));
    let timeline = Timeline::new();
    {
        let timeline = timeline.downgrade();
        stage.connect_moved(move |position, target| {
            if let Some(timeline) = timeline.upgrade() {
                timeline.set_position(position, target);
            }
        });
    }
    let rewind = Rc::new(Rewind {
        state: Rc::clone(&state),
        stage: stage.clone(),
        timeline: timeline.clone(),
        lister,
        index: index.clone(),
        bodies: RefCell::new(HashMap::new()),
        slots: RefCell::new(Vec::new()),
        generation: Cell::new(0),
        making: RefCell::new(HashSet::new()),
        thumbnails: crate::host::Host::here().cache.join("thumbnails"),
    });
    rewind.relist(false);

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
    overlay.add_overlay(&timeline);
    overlay.add_overlay(&lines);
    shell.set_content(Some(&overlay));

    let say = {
        let state = Rc::clone(&state);
        move |index: usize| {
            let view = state.view();
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
    let say: Rc<dyn Fn(usize)> = Rc::new(say);
    say(stage.target());

    stage.set_actions(rewind.actions());
    rewind.watch(&say);
    {
        let (weak, told) = (Rc::downgrade(&rewind), Rc::clone(&say));
        timeline.connect_chosen(move |index| {
            if let Some(rewind) = weak.upgrade() {
                let to = rewind.stage.go_to(index);
                rewind.moved_to(to);
                told(to);
            }
        });
        let (weak, told) = (Rc::downgrade(&rewind), Rc::clone(&say));
        timeline.connect_step(move |steps| {
            if let Some(rewind) = weak.upgrade() {
                let to = rewind.stage.step(steps, false);
                rewind.moved_to(to);
                told(to);
            }
        });
    }
    keys(&shell, &rewind, &say, parent);
    // However it is closed, the window gets Rewind's folder and selection.
    {
        let (rewind, window, parent) = (Rc::clone(&rewind), Rc::clone(window), parent.clone());
        shell.connect_close_request(move |_| {
            leave(&rewind, &window);
            parent.present();
            glib::Propagation::Proceed
        });
    }

    let monitor = parent
        .surface()
        .and_then(|surface| WidgetExt::display(parent).monitor_at_surface(&surface));
    // The monitor's own size from the first frame: a compositor that makes
    // the window full screen only after it has first been shown (Hyprland
    // does) would otherwise have the stack laid out for a window the size of
    // the two lines under it.
    if let Some(monitor) = &monitor {
        let geometry = monitor.geometry();
        shell.set_default_size(geometry.width(), geometry.height());
    }
    match &monitor {
        Some(monitor) => shell.fullscreen_on_monitor(monitor),
        None => shell.fullscreen(),
    }
    shell.present();

    rewind.read();
    find_background(&stage, monitor);
}

impl Rewind {
    /// What the window in front does when used.
    fn actions(self: &Rc<Self>) -> Actions {
        let state = Rc::clone(&self.state);
        let go = Rc::new(move |folder: String| state.set_folder(folder));
        let stage = self.stage.clone();
        let mode = Rc::new(move |mode: Mode| {
            MODE.set(mode);
            stage.set_mode(mode);
        });
        let (state, stage) = (Rc::clone(&self.state), self.stage.clone());
        let select = Rc::new(move |row: Option<Row>| {
            let folder = state.view().folder;
            stage.set_selected(row.as_ref().map(|r| r.entry.name.clone()));
            state.set_selected(row.map(|r| r.selected(&folder)));
        });
        let weak = Rc::downgrade(self);
        let shown = Rc::new(move |row: &Row| {
            if let Some(rewind) = weak.upgrade() {
                rewind.make_picture(row);
            }
        });
        Actions {
            go,
            mode,
            select,
            shown,
        }
    }

    /// Follow Rewind's own state: another folder is listed afresh, a new
    /// listing of the same folder keeps what was read, and hidden files shown
    /// or hidden list it again.
    fn watch(self: &Rc<Self>, say: &Rc<dyn Fn(usize)>) {
        let weak = Rc::downgrade(self);
        let say = Rc::clone(say);
        self.state.subscribe(move |view, change| {
            let Some(rewind) = weak.upgrade() else {
                return;
            };
            match change {
                Change::Folder => {
                    rewind.bodies.borrow_mut().clear();
                    rewind.relist(true);
                    rewind.ask_listing(view);
                    rewind.read();
                }
                Change::Listed => {
                    if rewind.relist(true) {
                        rewind.read();
                    }
                }
                Change::Hidden => {
                    rewind.bodies.borrow_mut().clear();
                    rewind.stage.set_show_hidden(view.show_hidden);
                    rewind.relist(true);
                    rewind.read();
                }
                _ => return,
            }
            say(rewind.stage.target());
        });
    }

    /// Ask which backups to list for the folder, as the window does when it
    /// moves to another.
    fn ask_listing(self: &Rc<Self>, view: &View) {
        if view.listing_is_current() {
            return;
        }
        let (folder, generation) = (view.folder.clone(), view.generation);
        let (index, state) = (self.index.clone(), Rc::clone(&self.state));
        crate::ui::spawn(async move {
            let asked = folder.clone();
            match index.query(move |reader| reader.listed_for(&asked)).await {
                Ok(listed) => state.set_listing(&folder, generation, listed),
                Err(error) => warn!(%error, folder, "the backups to list could not be read"),
            }
        });
    }

    /// Lay the stack out again from the state's listing, the same backup in
    /// front, or the folder as it is now if that was. With `kept`, only when
    /// the times listed changed. Says whether it did.
    fn relist(&self, kept: bool) -> bool {
        let view = self.state.view();
        let mut slots: Vec<When> = vec![None];
        slots.extend(view.listed.iter().map(|backup| Some(backup.seq)));
        if kept && *self.slots.borrow() == slots && self.stage.has_slots() {
            return false;
        }
        let at_now = kept && self.stage.target() == 0;
        let at = if at_now {
            0
        } else {
            view.seq
                .and_then(|seq| view.listed.iter().position(|a| a.seq == seq))
                .map_or(0, |i| i + 1)
        };
        let tz = glib::TimeZone::local();
        let folder = view.folder.clone();
        let bodies = self.bodies.borrow();
        let contents = slots
            .iter()
            .enumerate()
            .map(|(i, when)| Content {
                folder: folder.clone(),
                when: match i.checked_sub(1).and_then(|i| view.listed.get(i)) {
                    Some(backup) => crate::model::format::at(backup.ts, &tz, "%a %e %b %Y, %H:%M")
                        .trim_start()
                        .to_owned(),
                    None => "Today (Now)".to_owned(),
                },
                body: bodies.get(when).cloned().unwrap_or(Body::Waiting),
            })
            .collect();
        drop(bodies);
        self.generation.set(self.generation.get() + 1);
        *self.slots.borrow_mut() = slots;
        self.stage.set_slots(contents, at);
        let now = glib::DateTime::now_local().map_or(0, |now| now.to_unix());
        let ticks = std::iter::once(None)
            .chain(view.listed.iter().map(Some))
            .map(|backup| {
                let ts = backup.map(|b| b.ts);
                let kept = backup.map_or(Kept::Normally, Kept::of);
                let mut name = timeline_model::name(ts, now, &tz);
                if let Some(badge) = group::badge(kept, &tz) {
                    name = format!("{name} \u{b7} {}", badge.text);
                }
                Tick {
                    ts,
                    mark: timeline_model::mark(kept),
                    name,
                }
            })
            .collect();
        self.timeline.set_ticks(ticks, now, &tz);
        true
    }

    /// List what each time held that is not listed yet, outwards from the
    /// one in front: at, at+1, at-1, at+2, …
    fn read(self: &Rc<Self>) {
        let this = Rc::clone(self);
        let generation = self.generation.get();
        crate::ui::spawn(async move {
            let at = this.stage.target();
            let slots = this.slots.borrow().clone();
            let count = slots.len();
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
                let when = slots[slot];
                if this.bodies.borrow().contains_key(&when) {
                    continue;
                }
                let view = this.state.view();
                let body = match when {
                    None => {
                        let (folder, hidden) = (view.folder.clone(), view.show_hidden);
                        match gio::spawn_blocking(move || Lister::disk(&folder, hidden)).await {
                            Ok(Ok(rows)) => Body::Rows(rows),
                            Ok(Err(error)) => Body::Empty {
                                title: "This folder could not be read".to_owned(),
                                description: error,
                            },
                            Err(_) => continue,
                        }
                    }
                    Some(seq) => {
                        let catalogued = view
                            .listed
                            .iter()
                            .find(|a| a.seq == seq)
                            .is_none_or(|a| a.catalogued);
                        if !catalogued {
                            continue;
                        }
                        match this.lister.list(&view, Some(seq), slot == 1).await {
                            Listed::Rows { rows, .. } => Body::Rows(rows),
                            Listed::Empty {
                                title, description, ..
                            } => Body::Empty { title, description },
                        }
                    }
                };
                if let Body::Rows(rows) = &body {
                    this.cached_pictures(rows, &view.folder).await;
                }
                if this.generation.get() != generation {
                    // Listed again, for another folder or another listing.
                    return;
                }
                this.bodies.borrow_mut().insert(when, body.clone());
                this.stage.set_body(slot, body);
            }
        });
    }

    /// The desktop's thumbnails of the first of `rows`, those of the very
    /// versions listed, looked for before the cards are made.
    async fn cached_pictures(&self, rows: &[Row], folder: &str) {
        let pictures = self.stage.pictures();
        let wanted: Vec<(String, i64, i64, std::path::PathBuf)> = rows
            .iter()
            .filter(|r| r.entry.kind == Kind::File)
            .filter(|r| {
                !pictures
                    .borrow()
                    .contains_key(&(r.entry.name.clone(), r.entry.mtime))
            })
            .take(PICTURES_AHEAD)
            .map(|r| {
                let path = crate::path::to_filesystem(&crate::path::join(folder, &r.entry.name));
                (r.entry.name.clone(), r.entry.size, r.entry.mtime, path)
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let root = self.thumbnails.clone();
        let found = gio::spawn_blocking(move || {
            wanted
                .into_iter()
                .filter_map(|(name, size, mtime, path)| {
                    let source = cached_source(&root, &name, size, mtime, &path, false, false);
                    let thumbnail::Source::Cache(file) = source else {
                        return None;
                    };
                    let texture = gdk::Texture::from_filename(&file).ok()?;
                    Some(((name, mtime), texture))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        debug!(found = found.len(), "Rewind found the desktop's thumbnails");
        pictures.borrow_mut().extend(found);
    }

    /// A file came into sight in the window in front with no picture yet:
    /// the desktop's thumbnail of its version, or one made from its copy in
    /// the backup or on the disk, if it is an image (`model::thumbnail`).
    fn make_picture(self: &Rc<Self>, row: &Row) {
        if row.entry.kind != Kind::File {
            return;
        }
        let key = (row.entry.name.clone(), row.entry.mtime);
        if !self.making.borrow_mut().insert(key.clone()) {
            return;
        }
        let Some(slot) = self.stage.front_slot() else {
            debug!(
                name = row.entry.name,
                "no window in front to make a picture for"
            );
            self.making.borrow_mut().remove(&key);
            return;
        };
        let when = self.slots.borrow().get(slot).copied().flatten();
        let view = self.state.view();
        let file = crate::path::join(&view.folder, &row.entry.name);
        let path = crate::path::to_filesystem(&file);
        // The archive its bytes are read through, as the preview reads them.
        let archive = row.trash.clone().or_else(|| {
            let seq = when?;
            let base = match trash::base_of(seq) {
                Some(base) => base?,
                None => seq,
            };
            view.archives
                .iter()
                .find(|a| a.seq == base)
                .map(|a| a.name.clone())
        });
        let (root, now) = (self.thumbnails.clone(), when.is_none());
        let (name, size, mtime) = (row.entry.name.clone(), row.entry.size, row.entry.mtime);
        let daemon = self.lister.daemon();
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let lookup_path = path.clone();
            let source = gio::spawn_blocking(move || {
                cached_source(&root, &name, size, mtime, &lookup_path, true, now)
            })
            .await
            .unwrap_or(thumbnail::Source::Icon);
            debug!(file, ?source, archive, "Rewind is making a picture");
            let from = match &source {
                thumbnail::Source::Cache(_) => "the desktop's thumbnail",
                thumbnail::Source::Backup => "the backup",
                thumbnail::Source::Disk => "the disk",
                thumbnail::Source::Icon => "",
            };
            let texture = match source {
                thumbnail::Source::Icon => None,
                thumbnail::Source::Cache(file) => {
                    gio::spawn_blocking(move || gdk::Texture::from_filename(&file).ok())
                        .await
                        .ok()
                        .flatten()
                }
                thumbnail::Source::Disk => {
                    let file = gio::File::for_path(&path);
                    match file.read_future(glib::Priority::LOW).await {
                        Ok(stream) => picture_of(stream.upcast()).await,
                        Err(_) => None,
                    }
                }
                thumbnail::Source::Backup => match (daemon, archive) {
                    (Some(proxy), Some(archive)) => {
                        match proxy.preview_file(&archive, &file).await {
                            Ok(fd) => {
                                let read = gio::spawn_blocking(move || {
                                    crate::ui::preview::read_capped(fd.into())
                                })
                                .await;
                                match read {
                                    Ok(Ok(bytes)) => {
                                        let bytes = glib::Bytes::from_owned(bytes);
                                        let stream = gio::MemoryInputStream::from_bytes(&bytes);
                                        picture_of(stream.upcast()).await
                                    }
                                    _ => None,
                                }
                            }
                            Err(error) => {
                                debug!(%error, file, "a picture could not be fetched");
                                None
                            }
                        }
                    }
                    _ => None,
                },
            };
            let Some(texture) = texture else {
                return;
            };
            debug!(file, from, "Rewind made a picture");
            this.stage
                .pictures()
                .borrow_mut()
                .insert(key.clone(), texture.clone());
            if let Some(cells) = this.stage.front_cells() {
                folder::show_picture(&cells, &key.0, &texture);
            }
        });
    }
}

/// Where a file's picture comes from, its version's thumbnail looked for in
/// the cache at `root`.
fn cached_source(
    root: &std::path::Path,
    name: &str,
    size: i64,
    mtime: i64,
    path: &std::path::Path,
    front: bool,
    now: bool,
) -> thumbnail::Source {
    let cached = glib::filename_to_uri(path, None)
        .ok()
        .and_then(|uri| thumbnail::cached(root, &uri));
    thumbnail::source(
        thumbnail::Facts {
            name,
            file: true,
            size,
            mtime,
            front,
            now,
        },
        cached.as_ref(),
    )
}

/// A picture of the image read from `stream`, at a thumbnail's size.
async fn picture_of(stream: gio::InputStream) -> Option<gdk::Texture> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream_at_scale_future(&stream, 256, 256, true)
        .await
        .ok()?;
    let format = if pixbuf.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    Some(
        gdk::MemoryTexture::new(
            pixbuf.width(),
            pixbuf.height(),
            format,
            &pixbuf.read_pixel_bytes(),
            pixbuf.rowstride() as usize,
        )
        .upcast(),
    )
}

/// The keys, and the mouse wheel: Up and Down (and Page Up and Page Down) step
/// back and forward, Home goes to now and End to the oldest, Alt+Up goes up a
/// folder, Escape leaves. Left and Right are the folder's own, moving among
/// its icons (Keith, 2026-11-04).
fn keys(
    shell: &adw::Window,
    rewind: &Rc<Rewind>,
    say: &Rc<dyn Fn(usize)>,
    parent: &adw::ApplicationWindow,
) {
    // Keys held down: GTK reports each repeat as another press, and a press
    // of a key already down is a repeat.
    let held: Rc<RefCell<HashSet<u32>>> = Rc::default();
    let controller = gtk4::EventControllerKey::new();
    // Before the folder's icons, which would take Up and Down for their own.
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    {
        let (rewind, held, say) = (Rc::clone(rewind), Rc::clone(&held), Rc::clone(say));
        let (shell, parent) = (shell.clone(), parent.clone());
        controller.connect_key_pressed(move |_, key, code, modifiers| {
            let repeat = !held.borrow_mut().insert(code);
            let stage = &rewind.stage;
            // Ctrl+H, as in the window, and kept in step with it (S18-T11).
            let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                && !modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            if control && matches!(key, gdk::Key::h | gdk::Key::H) {
                if !repeat {
                    let on = !rewind.state.view().show_hidden;
                    rewind.state.set_show_hidden(on);
                    let _ = WidgetExt::activate_action(&parent, "win.show-hidden", None);
                }
                return glib::Propagation::Stop;
            }
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            if alt && matches!(key, gdk::Key::Up | gdk::Key::KP_Up) {
                if !repeat {
                    if let Some(up) = crate::path::parent(&rewind.state.view().folder) {
                        rewind.state.set_folder(up);
                    }
                }
                return glib::Propagation::Stop;
            }
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
                    shell.close();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            rewind.moved_to(to);
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
    shell.add_controller(controller);

    // Over the window in front, the wheel scrolls its folder instead.
    rewind.stage.add_controller(wheel(rewind, say));
    rewind.timeline.add_controller(wheel(rewind, say));
}

/// A notch of the wheel, or the same distance on a touchpad, is a step; like
/// a held key, it runs at most two backups ahead of the stack.
fn wheel(rewind: &Rc<Rewind>, say: &Rc<dyn Fn(usize)>) -> gtk4::EventControllerScroll {
    let wheel = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
    let travelled = Rc::new(Cell::new(0.0));
    let (weak, say) = (Rc::downgrade(rewind), Rc::clone(say));
    wheel.connect_scroll(move |_, _, dy| {
        let Some(rewind) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        travelled.set(travelled.get() + dy);
        if travelled.get().abs() >= 1.0 {
            let steps = if travelled.get() < 0.0 { 1 } else { -1 };
            travelled.set(0.0);
            let to = rewind.stage.step(steps, true);
            rewind.moved_to(to);
            say(to);
        }
        glib::Propagation::Stop
    });
    wheel
}

impl Rewind {
    /// The stack is on its way to `slot`: the state follows it to that
    /// backup, so that a folder walked into lands where the stack is.
    fn moved_to(&self, slot: usize) {
        let when = self.slots.borrow().get(slot).copied().flatten();
        if let Some(seq) = when {
            self.state.set_seq(seq);
        }
    }
}

/// Give the window Rewind's folder and selection (S18-T4).
fn leave(rewind: &Rewind, window: &Rc<AppState>) {
    let view = rewind.state.view();
    window.set_folder(view.folder.clone());
    window.set_selected(view.selected);
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
        let output = monitor
            .as_ref()
            .and_then(|m| m.connector())
            .map(|c| c.to_string());
        let background = match crate::wallpaper::find(screen, output).await {
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
