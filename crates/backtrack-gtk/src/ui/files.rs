// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The file pane: one folder, as it was at the selected backup.
//!
//! This is where the product's one idea shows up as behaviour. The pane
//! reloads when the folder changes and when the backup changes, and the two are
//! the same code path — stepping back through time is, to this pane, simply a
//! different answer to the same question. Files that were deleted later
//! reappear as you go back, which is what makes finding them a matter of
//! looking rather than searching.
//!
//! Both status badges come from the index, not from a comparison done here:
//! "deleted after this" and "changed since then" are columns of the query in
//! S01-T3, computed in SQL over the interval encoding.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use backtrack_core::index::{Entry, IndexReader, Kind};
use gtk4::prelude::*;
use gtk4::{
    gio, glib, Align, Box as GtkBox, Button, ColumnView, ColumnViewColumn, Image, Label, ListItem,
    Orientation, ScrolledWindow, SignalListItemFactory, SingleSelection, Stack, Widget,
};
use libadwaita as adw;
use tracing::{debug, warn};

use crate::daemon::Daemon1Proxy;
use crate::index::Index;
use crate::model::above;
use crate::model::empty::{self, Coverage, Empty};
use crate::path;
use crate::state::{AppState, Change, Selected};

/// A row of the pane: the indexed entry plus the strings it is drawn with.
#[derive(Clone)]
struct RowData {
    entry: Entry,
    size: String,
    modified: String,
    /// What the daemon said about this path being on the computer now: 0
    /// unknown, 1 absent, 2 present. Unknown when the daemon was not there to
    /// ask, which is why the default says nothing rather than "gone".
    on_disk: u8,
    /// A folder that is not in the backup itself but leads down to folders
    /// that are (see `model::above`): it has no size or date to show.
    partly: bool,
    /// For something in the Trash, the archive name its bytes are read
    /// through (Stage 16).
    trash: Option<String>,
}

/// The daemon's answers about a path being on the computer now. Three-valued,
/// because "the folder could not be read" is not "the file was deleted".
const UNKNOWN: u8 = 0;
const ABSENT: u8 = 1;

/// The pane and what it needs to refill itself.
pub struct Files {
    stack: Stack,
    rows: gio::ListStore,
    selection: SingleSelection,
    empty: adw::StatusPage,
    /// "Add to Backups…", on the empty page only when the folder is outside
    /// the backups.
    add: Button,
    /// The line above the list of a folder only partly backed up, and its
    /// text.
    notice: GtkBox,
    notice_text: Label,
    /// Each backup's top folders, by sequence number, as asked of the index:
    /// a folder outside the backups needs them on every visit, and they do
    /// not change until the backups do.
    roots: RefCell<HashMap<i64, Rc<Vec<String>>>>,
    state: Rc<AppState>,
    index: Index,
    /// `None` until the daemon answers, and if it never does. Browsing works
    /// without it; only the "not on your disk" status needs it, and the
    /// absence of an answer is itself an answer the status column can hold.
    daemon: Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
    /// Which load is current. A load that finishes after a newer one started is
    /// dropped: arrow-keying through time fires these faster than they return,
    /// and the last answer to arrive must not win over the last one asked for.
    generation: Cell<u64>,
    syncing: Cell<bool>,
}

/// Build the file pane for `state`. `add_to_backups` is given the folder
/// being shown when somebody asks for a folder outside the backups to be
/// backed up.
pub fn build(
    state: &Rc<AppState>,
    index: &Index,
    daemon: &Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
    add_to_backups: impl Fn(&str) + 'static,
) -> Rc<Files> {
    let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = SingleSelection::builder()
        .model(&rows)
        .autoselect(false)
        .can_unselect(true)
        .build();

    let view = ColumnView::builder()
        .model(&selection)
        .show_column_separators(false)
        .show_row_separators(true)
        .build();
    view.add_css_class("card");
    for column in columns() {
        view.append_column(&column);
    }

    let scroller = ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();

    let add_to_backups: Rc<dyn Fn(&str)> = Rc::new(add_to_backups);
    let add = Button::builder()
        .label("Add to Backups…")
        .halign(Align::Center)
        .visible(false)
        .build();
    add.add_css_class("pill");
    let adder = Rc::clone(state);
    let adding = Rc::clone(&add_to_backups);
    add.connect_clicked(move |_| adding(&adder.view().folder));

    let empty = adw::StatusPage::builder()
        .icon_name("folder-symbolic")
        .title("Nothing here")
        .child(&add)
        .build();
    empty.add_css_class("compact");

    // Above the list of a folder that leads down to backed-up folders without
    // being backed up itself: what is not there, and how to have it be.
    let notice_text = Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    let notice_add = Button::builder()
        .label("Add to Backups…")
        .valign(Align::Center)
        .build();
    notice_add.add_css_class("flat");
    let adder = Rc::clone(state);
    notice_add.connect_clicked(move |_| add_to_backups(&adder.view().folder));
    let notice = GtkBox::new(Orientation::Horizontal, 10);
    notice.add_css_class("partly-notice");
    notice.append(&Image::from_icon_name("dialog-information-symbolic"));
    notice.append(&notice_text);
    notice.append(&notice_add);
    notice.set_visible(false);

    let listing = GtkBox::new(Orientation::Vertical, 8);
    listing.append(&notice);
    listing.append(&scroller);

    let stack = Stack::new();
    stack.add_named(&listing, Some("files"));
    stack.add_named(&empty, Some("empty"));

    let files = Rc::new(Files {
        stack,
        rows,
        selection,
        empty,
        add,
        notice,
        notice_text,
        roots: RefCell::new(HashMap::new()),
        state: Rc::clone(state),
        index: index.clone(),
        daemon: Rc::clone(daemon),
        generation: Cell::new(0),
        syncing: Cell::new(false),
    });

    let watcher = Rc::clone(&files);
    state.subscribe(move |_, change| {
        if change == Change::Archives {
            // New backups, or fewer: the top folders are asked for again.
            watcher.roots.borrow_mut().clear();
        }
        let at_entry = watcher.state.view().at_trash_entry();
        if matches!(
            change,
            Change::Folder | Change::Seq | Change::Archives | Change::Disk
        ) || (change == Change::Trash && at_entry)
        {
            watcher.reload();
        }
    });

    let selector = Rc::clone(&files);
    files
        .selection
        .connect_selected_item_notify(move |selection| {
            if selector.syncing.get() {
                return;
            }
            let selected = row_of(selection.selected_item().as_ref()).map(|row| {
                let folder = selector.state.view().folder;
                Selected {
                    path: path::join(&folder, &row.entry.name),
                    name: row.entry.name.clone(),
                    is_dir: row.entry.kind == Kind::Dir,
                    size: row.entry.size,
                    mtime: row.entry.mtime,
                    on_disk: row.on_disk,
                    archive: row.trash.clone(),
                }
            });
            selector.state.set_selected(selected);
        });

    // Double-click, or Enter: a folder is somewhere to go, a file is something
    // to look at.
    let opener = Rc::clone(&files);
    view.connect_activate(move |view, position| {
        let Some(row) = view
            .model()
            .and_then(|model| model.item(position))
            .and_then(|item| row_of(Some(&item)))
        else {
            return;
        };
        if row.entry.kind == Kind::Dir {
            let folder = opener.state.view().folder;
            opener
                .state
                .set_folder(path::join(&folder, &row.entry.name));
        }
    });

    files.reload();
    files
}

impl Files {
    /// The widget to put in the window.
    pub fn widget(&self) -> Widget {
        self.stack.clone().upcast()
    }

    /// Re-read the folder at the current backup.
    pub fn reload(self: &Rc<Self>) {
        let view = self.state.view();
        let Some(seq) = view.seq else {
            debug!(folder = view.folder, "no backups catalogued yet");
            self.show_empty(
                "No backups yet",
                "Backtrack has not catalogued any backups.",
            );
            return;
        };
        if crate::model::trash::is_entry(seq) {
            self.reload_entry(&view, seq);
            return;
        }
        let folder = view.folder.clone();
        let newest = view.newer().is_none();
        let wanted = self.generation.get() + 1;
        self.generation.set(wanted);

        let this = Rc::clone(self);
        let query_folder = folder.clone();
        crate::ui::spawn(async move {
            let answer = this
                .index
                .query(move |reader| {
                    let entries = reader.folder_at(&query_folder, seq)?;
                    // Whether the folder itself is in the backup: if it is, an
                    // empty list means it was empty then, and it is inside the
                    // backups, with nothing above them to lead down to.
                    let here = present_at(reader, &query_folder, seq)?;
                    Ok((entries, here))
                })
                .await;
            if this.generation.get() != wanted {
                // Superseded while the query was in flight.
                return;
            }
            // A folder outside the backup may lead down to folders in it.
            let (leading, partly) = match &answer {
                Ok((entries, false)) => {
                    let roots = this.roots_at(seq).await;
                    if this.generation.get() != wanted {
                        return;
                    }
                    let leading: Vec<String> = above::leading_down(&folder, &roots)
                        .into_iter()
                        .filter(|name| !entries.iter().any(|e| &e.name == name))
                        .collect();
                    (leading, above::partly(&folder, &roots))
                }
                _ => (Vec::new(), false),
            };
            let answer = answer.map(|(entries, here)| (entries, here, leading));
            match answer {
                Ok((entries, in_backup, leading)) if entries.is_empty() && leading.is_empty() => {
                    let coverage = if in_backup || !newest {
                        // Settled already: see `empty::why`.
                        Coverage::Unknown
                    } else {
                        this.coverage(&folder, seq).await
                    };
                    if this.generation.get() != wanted {
                        return;
                    }
                    let why = empty::why(in_backup, newest, coverage);
                    debug!(folder, seq, ?why, "nothing to show");
                    this.show_why(why, &folder);
                }
                Ok((mut entries, _, leading)) => {
                    // The folders leading down are entries the catalogue does
                    // not have: in no backup, so with no size, date or badge
                    // of their own.
                    entries.extend(leading.iter().map(|name| Entry {
                        path_id: 0,
                        name: name.clone(),
                        kind: Kind::Dir,
                        size: 0,
                        mtime: 0,
                        deleted_after: false,
                        changed_since: false,
                    }));
                    // Asked before painting rather than after, so the status
                    // column is right the first time it is drawn. It is one
                    // directory read on the other side of a local socket, and
                    // a column that corrects itself a moment later is worse
                    // than one that waits.
                    let on_disk = this.ask_the_disk(&entries, &folder).await;
                    if this.generation.get() != wanted {
                        return;
                    }
                    this.show_notice(partly.then_some(folder.as_str()));
                    debug!(
                        folder,
                        seq,
                        entries = entries.len(),
                        deleted_after = entries.iter().filter(|e| e.deleted_after).count(),
                        changed_since = entries.iter().filter(|e| e.changed_since).count(),
                        gone_from_disk = on_disk.iter().filter(|a| **a == ABSENT).count(),
                        leading_down = leading.len(),
                        partly,
                        "folder loaded"
                    );
                    this.show(entries, on_disk, &leading, &HashMap::new())
                }
                Err(error) => {
                    warn!(%error, folder, seq, "the folder could not be read");
                    this.show_empty("This folder could not be read", &error);
                }
            }
        });
    }

    /// Read the folder at a Trash entry (Stage 16): the backup before its gap
    /// with what was moved to the Trash in the gap put back, or, inside a
    /// folder that went to the Trash whole, the Trash's copy of it.
    fn reload_entry(self: &Rc<Self>, view: &crate::state::View, seq: i64) {
        use crate::model::trash;
        let folder = view.folder.clone();
        let archives = Rc::clone(&view.archives);
        let items = Rc::clone(&view.trash);
        let wanted = self.generation.get() + 1;
        self.generation.set(wanted);
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let rows: Result<Vec<trash::Row>, String> = match trash::trashed_folder(
                &folder, seq, &archives, &items,
            )
            .cloned()
            {
                Some(item) => {
                    let proxy = this.daemon.borrow().clone();
                    match proxy {
                            None => Err(
                                "Backtrack's background service reads the Trash, and it is not running."
                                    .to_string(),
                            ),
                            Some(proxy) => proxy
                                .list_trashed_folder(&item.archive, &folder)
                                .await
                                .map_err(|e| e.to_string())
                                .map(|listed| {
                                    listed
                                        .into_iter()
                                        .map(|(name, kind, size, mtime)| trash::Row {
                                            entry: Entry {
                                                path_id: 0,
                                                name,
                                                kind: Kind::from_token(&kind),
                                                size,
                                                mtime,
                                                deleted_after: false,
                                                changed_since: false,
                                            },
                                            trash: Some(item.archive.clone()),
                                        })
                                        .collect()
                                }),
                        }
                }
                None => {
                    let base = match trash::base_of(seq).flatten() {
                        Some(base) => {
                            let asked = folder.clone();
                            this.index
                                .query(move |reader| reader.folder_at(&asked, base))
                                .await
                        }
                        None => Ok(Vec::new()),
                    };
                    base.map(|base| trash::overlay(&folder, seq, &archives, &items, base))
                }
            };
            if this.generation.get() != wanted {
                return;
            }
            match rows {
                Ok(rows) if rows.is_empty() => this.show_empty(
                    "Nothing from the Trash here",
                    "Nothing moved to the Trash at this time came from this folder.",
                ),
                Ok(rows) => {
                    let trashed: HashMap<String, String> = rows
                        .iter()
                        .filter_map(|r| r.trash.clone().map(|a| (r.entry.name.clone(), a)))
                        .collect();
                    let entries: Vec<Entry> = rows.into_iter().map(|r| r.entry).collect();
                    let on_disk = this.ask_the_disk(&entries, &folder).await;
                    if this.generation.get() != wanted {
                        return;
                    }
                    this.show_notice(None);
                    debug!(
                        folder,
                        seq,
                        entries = entries.len(),
                        trashed = trashed.len(),
                        "Trash entry loaded"
                    );
                    this.show(entries, on_disk, &[], &trashed);
                }
                Err(error) => {
                    warn!(%error, folder, seq, "the folder could not be read");
                    this.show_empty("This folder could not be read", &error);
                }
            }
        });
    }

    /// What the daemon says about each of these entries being on the computer.
    ///
    /// An empty answer means nobody was asked — the daemon is not there, or it
    /// refused — and every entry is then unknown. Browsing has never required
    /// the daemon and must not start to.
    async fn ask_the_disk(&self, entries: &[Entry], folder: &str) -> Vec<u8> {
        let unknown = vec![UNKNOWN; entries.len()];
        let Some(proxy) = self.daemon.borrow().clone() else {
            return unknown;
        };
        let paths: Vec<String> = entries
            .iter()
            .map(|entry| path::join(folder, &entry.name))
            .collect();
        match proxy.paths_on_disk(&paths).await {
            Ok(answers) if answers.len() == entries.len() => answers,
            Ok(answers) => {
                warn!(
                    asked = entries.len(),
                    answered = answers.len(),
                    "the daemon answered for a different number of paths than were asked about"
                );
                unknown
            }
            Err(error) => {
                debug!(%error, folder, "the daemon could not say what is on disk");
                unknown
            }
        }
    }

    /// Where `folder` stands with the folders Backtrack backs up, and whether
    /// the one it is in is in backup `seq`.
    async fn coverage(&self, folder: &str, seq: i64) -> Coverage {
        let Some(roots) = self.roots().await else {
            return Coverage::Unknown;
        };
        let Some(root) = empty::root_of(folder, &roots) else {
            return Coverage::Outside;
        };
        let root = path::to_archive(root);
        if root.is_empty() {
            // The filesystem root is in every backup there is.
            return Coverage::Inside {
                root_in_backup: true,
            };
        }
        match self
            .index
            .query(move |reader| present_at(reader, &root, seq))
            .await
        {
            Ok(root_in_backup) => Coverage::Inside { root_in_backup },
            Err(error) => {
                debug!(%error, folder, "the backed-up folder could not be looked up");
                Coverage::Unknown
            }
        }
    }

    /// The folders Backtrack backs up, or `None` with no daemon to ask.
    async fn roots(&self) -> Option<Vec<PathBuf>> {
        let proxy = self.daemon.borrow().clone()?;
        let text = match proxy.get_config().await {
            Ok(text) => text,
            Err(error) => {
                debug!(%error, "the backed-up folders could not be read");
                return None;
            }
        };
        let (config, _) = backtrack_core::config::Config::parse(&text).ok()?;
        Some(backtrack_core::roots::Roots::of(&config).roots)
    }

    /// Put `entries` on screen, keeping the selection if it survived the
    /// change. `leading` names the ones that lead down to backed-up folders.
    fn show(
        self: &Rc<Self>,
        entries: Vec<Entry>,
        on_disk: Vec<u8>,
        leading: &[String],
        trashed: &HashMap<String, String>,
    ) {
        let previously = self.state.view().selected.map(|s| s.name);

        self.syncing.set(true);
        self.rows.remove_all();
        let tz = glib::TimeZone::local();
        // Paired before sorting, because the answers came back in the order
        // the entries were asked about and the sort is about to destroy it.
        let mut sorted: Vec<(Entry, u8)> = entries
            .into_iter()
            .zip(on_disk.into_iter().chain(std::iter::repeat(UNKNOWN)))
            .collect();
        // Folders first, then by name, the way every file manager does it. The
        // index returns plain name order, which mixes them.
        sorted.sort_by(|(a, _), (b, _)| {
            (b.kind == Kind::Dir)
                .cmp(&(a.kind == Kind::Dir))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        for (entry, on_disk) in sorted {
            let partly = leading.contains(&entry.name);
            let data = RowData {
                size: if partly {
                    "—".to_string()
                } else {
                    crate::model::format::size(entry.size, entry.kind)
                },
                modified: if partly {
                    "—".to_string()
                } else {
                    crate::model::format::modified(entry.mtime, &tz)
                },
                trash: trashed.get(&entry.name).cloned(),
                entry,
                on_disk,
                partly,
            };
            self.rows.append(&glib::BoxedAnyObject::new(data));
        }
        self.stack.set_visible_child_name("files");
        self.syncing.set(false);

        // Stepping through time keeps you on the file you were looking at, so
        // long as it existed then.
        match previously.and_then(|name| self.position_of(&name)) {
            Some(position) => self.selection.set_selected(position),
            None => {
                self.selection.set_selected(gtk4::INVALID_LIST_POSITION);
                self.state.set_selected(None);
            }
        }
    }

    fn show_empty(&self, title: &str, description: &str) {
        self.rows.remove_all();
        self.empty.set_title(title);
        self.empty.set_description(Some(description));
        self.add.set_visible(false);
        self.stack.set_visible_child_name("empty");
    }

    fn show_why(&self, why: Empty, folder: &str) {
        let name = label(folder).unwrap_or_else(|| path::name(folder).to_string());
        self.show_empty(why.title(), &why.description(&name));
        self.add.set_visible(why.offers_to_add());
    }

    /// Say above the list that only some of `folder` is backed up, or, with
    /// `None`, say nothing.
    fn show_notice(&self, folder: Option<&str>) {
        if let Some(folder) = folder {
            self.notice_text
                .set_text(&above::notice(label(folder).as_deref()));
        }
        self.notice.set_visible(folder.is_some());
    }

    /// The top folders of backup `seq`, from the index, asked once per
    /// backup. Empty when the index cannot say, which leads nowhere.
    async fn roots_at(&self, seq: i64) -> Rc<Vec<String>> {
        if let Some(roots) = self.roots.borrow().get(&seq) {
            return Rc::clone(roots);
        }
        let roots = match self.index.query(move |r| r.backed_up_roots(seq)).await {
            Ok(roots) => Rc::new(roots),
            Err(error) => {
                debug!(%error, seq, "the backup's top folders could not be read");
                return Rc::new(Vec::new());
            }
        };
        self.roots.borrow_mut().insert(seq, Rc::clone(&roots));
        roots
    }

    fn position_of(&self, name: &str) -> Option<u32> {
        (0..self.rows.n_items()).find(|index| {
            row_of(self.rows.item(*index).as_ref()).is_some_and(|row| row.entry.name == name)
        })
    }
}

/// What the breadcrumb calls `folder` ("Home" for the home folder), or `None`
/// for the filesystem root, which has no crumb.
fn label(folder: &str) -> Option<String> {
    let home = crate::ui::breadcrumb::home_archive_path();
    path::breadcrumbs(folder, home.as_deref())
        .pop()
        .map(|crumb| crumb.label)
}

/// Whether `path` itself, rather than only something inside it, is in backup
/// `seq`.
fn present_at(reader: &IndexReader, path: &str, seq: i64) -> backtrack_core::index::Result<bool> {
    Ok(reader
        .file_history(path)?
        .iter()
        .any(|span| span.first_seq <= seq && seq <= span.last_seq))
}

/// The row behind a list item.
fn row_of(item: Option<&glib::Object>) -> Option<RowData> {
    let boxed = item?.downcast_ref::<glib::BoxedAnyObject>()?;
    let row = boxed.borrow::<RowData>().clone();
    Some(row)
}

/// Name, Size, Modified, Status — the mockup's columns, in its order.
fn columns() -> Vec<ColumnViewColumn> {
    vec![
        // Widths chosen so all four columns fit a pane of about 500 pixels,
        // which is what the window's natural size leaves after the sidebar and
        // the preview. Narrower than that and the view scrolls sideways rather
        // than dropping a column — visibly imperfect, and better than silently
        // hiding the one that says a file was deleted.
        //
        // A fixed width on the Name column as well as `expand`. Expanding
        // shares out *spare* room; when there is none the expanding column is
        // the one that gives way, and the name — the only thing on the row
        // that identifies the file — collapsed to an ellipsis while the date
        // beside it kept every pixel it asked for.
        ColumnViewColumn::builder()
            .title("Name")
            .expand(true)
            .resizable(true)
            .fixed_width(160)
            .factory(&name_factory())
            .build(),
        ColumnViewColumn::builder()
            .title("Size")
            .fixed_width(76)
            .resizable(true)
            .factory(&text_factory(|row| row.size.clone(), Align::End))
            .build(),
        ColumnViewColumn::builder()
            .title("Modified")
            .fixed_width(132)
            .resizable(true)
            .factory(&text_factory(|row| row.modified.clone(), Align::Start))
            .build(),
        ColumnViewColumn::builder()
            .title("Status")
            .fixed_width(124)
            .resizable(true)
            .factory(&status_factory())
            .build(),
    ]
}

/// An icon and the entry's name.
fn name_factory() -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let content = GtkBox::new(Orientation::Horizontal, 10);
        content.append(&Image::new());
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        content.append(&label);
        item.set_child(Some(&content));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(content), Some(row)) = (
            item.child().and_then(|c| c.downcast::<GtkBox>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        if let Some(image) = content
            .first_child()
            .and_then(|c| c.downcast::<Image>().ok())
        {
            image.set_from_gicon(&icon_for(&row.entry));
        }
        if let Some(label) = content
            .last_child()
            .and_then(|c| c.downcast::<Label>().ok())
        {
            label.set_text(&row.entry.name);
        }
    });
    factory
}

/// A plain text column.
fn text_factory(text: fn(&RowData) -> String, align: Align) -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let label = Label::new(None);
        label.set_halign(align);
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(label), Some(row)) = (
            item.child().and_then(|c| c.downcast::<Label>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        label.set_text(&text(&row));
    });
    factory
}

/// The badges. Both are words first and colour second: the pane has to work in
/// a screenshot printed in black and white.
fn status_factory() -> SignalListItemFactory {
    let factory = SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let label = Label::new(None);
        label.set_halign(Align::Start);
        label.add_css_class("badge");
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let (Some(label), Some(row)) = (
            item.child().and_then(|c| c.downcast::<Label>().ok()),
            row_of(item.item().as_ref()),
        ) else {
            return;
        };
        label.remove_css_class("deleted");
        label.remove_css_class("changed");
        label.remove_css_class("partly");
        label.remove_css_class("in-trash");
        label.set_tooltip_text(None);
        if row.trash.is_some() {
            label.set_text("in the Trash");
            label.add_css_class("in-trash");
            label.set_tooltip_text(Some(crate::model::group::IN_TRASH_TOOLTIP));
            label.set_visible(true);
        } else if row.partly {
            label.set_text(above::BADGE);
            label.add_css_class("partly");
            label.set_visible(true);
        } else if row.entry.deleted_after {
            label.set_text("deleted after this");
            label.add_css_class("deleted");
            label.set_visible(true);
        } else if row.on_disk == ABSENT {
            // The catalogue believes this file is current and the computer
            // disagrees, which is the one question the other two badges cannot
            // answer and the one a person usually arrives with. Only ever
            // shown for a *known* absence: an unknown says nothing.
            label.set_text("not on your disk");
            label.add_css_class("deleted");
            label.set_visible(true);
        } else if row.entry.changed_since {
            label.set_text("changed since then");
            label.add_css_class("changed");
            label.set_visible(true);
        } else {
            // An em dash rather than nothing, so the column reads as answered.
            label.set_text("—");
            label.set_visible(true);
        }
    });
    factory
}

/// The themed icon for an entry, chosen from its name the way a file manager
/// does — the index records no content type, and guessing from the name is what
/// gives a PDF a PDF icon.
fn icon_for(entry: &Entry) -> gio::Icon {
    match entry.kind {
        Kind::Dir => gio::ThemedIcon::new("folder").upcast(),
        Kind::Symlink => gio::ThemedIcon::new("emblem-symbolic-link").upcast(),
        _ => {
            let (content_type, _) = gio::content_type_guess(Some(&entry.name), None);
            gio::content_type_get_icon(&content_type)
        }
    }
}
