// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Search across every backup: Charlie's story.
//!
//! He downloaded something last week, it is not where he left it, and he does
//! not remember which folder that was. Browsing cannot help him — browsing
//! needs you to know where to look — so this is the one part of the window
//! that answers a question about the whole history at once.
//!
//! Not a separate window. Searching is not a place you go, it is something you
//! do to the thing already in front of you, and the answer is a list you act on
//! and leave. The entry takes the header bar's title position and the results
//! take the body, which is mockup 20's layout: a wide box to type in and the
//! full width of the window to read the answers in.
//!
//! The stage file says "revealer"; two stacks do the same job here and do it
//! without the panes resizing underneath as the results arrive. The mockup is
//! the normative reference and this is what it shows.
//!
//! Typing is debounced rather than sent through as it happens. The daemon
//! refuses a query shorter than two characters, and the point of the debounce
//! is the same one: a person typing "contract" would otherwise ask eight
//! questions, seven of which they never wanted answered.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backtrack_core::dbus::SearchResult;
use backtrack_core::index::Kind;
use gtk4::prelude::*;
use gtk4::{
    gio, glib, Align, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, SearchEntry,
    SelectionMode, Widget,
};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{debug, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::search as copy;
use crate::state::AppState;

/// How long to wait after the last keystroke before asking.
///
/// Long enough that a word typed at speed is one question rather than eight,
/// short enough that it still feels like the list is following the typing.
const DEBOUNCE_MS: u32 = 150;

/// The shortest query the daemon will answer, repeated here only so the empty
/// state can explain the refusal before it happens.
const MIN_QUERY_CHARS: usize = 2;

/// The daemon, as this pane holds it.
type Service = Rc<RefCell<Option<Daemon1Proxy<'static>>>>;

/// Told when search mode opens or closes, so the window can swap both halves.
type Watcher = RefCell<Option<Box<dyn Fn(bool)>>>;

/// What a card's buttons do. Stage 8's actions live outside this module so the
/// pane does not need to know how to navigate or how to restore.
#[derive(Clone)]
pub struct Actions {
    /// Close search and show this path at the last backup that had it.
    pub view_in_timeline: Rc<dyn Fn(&SearchResult)>,
    /// Put the newest backed-up version back where it came from.
    pub restore_latest: Rc<dyn Fn(&SearchResult)>,
}

/// Search mode: the entry, the caption, and the results under them.
pub struct Search {
    /// The results, for the window's content area.
    column: GtkBox,
    /// Whether search mode is showing. Held here rather than read off a widget
    /// because the window has two things to swap and they must agree.
    open: Cell<bool>,
    entry: SearchEntry,
    caption: Label,
    results: ListBox,
    empty: adw::StatusPage,
    stack: gtk4::Stack,
    state: Rc<AppState>,
    daemon: Service,
    actions: Actions,
    /// The debounce timer, cancelled whenever another key arrives.
    pending: RefCell<Option<glib::SourceId>>,
    /// Which query is current, so a slow answer to an old one is dropped.
    generation: Cell<u64>,
    /// Told when search mode opens or closes.
    watcher: Watcher,
}

pub fn build(state: &Rc<AppState>, daemon: &Service, actions: Actions) -> Rc<Search> {
    let entry = SearchEntry::new();
    entry.set_placeholder_text(Some("Search every backup"));
    entry.set_hexpand(true);

    let caption = Label::new(None);
    caption.add_css_class("dim-label");
    caption.set_halign(Align::Start);
    caption.set_margin_start(6);
    caption.set_margin_top(10);
    caption.set_margin_bottom(4);

    let results = ListBox::new();
    results.set_selection_mode(SelectionMode::None);
    results.add_css_class("boxed-list");

    let empty = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .vexpand(true)
        .build();

    let stack = gtk4::Stack::new();
    stack.add_named(
        &ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&results)
            .vexpand(true)
            .build(),
        Some("results"),
    );
    stack.add_named(&empty, Some("empty"));

    let column = GtkBox::new(Orientation::Vertical, 0);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.set_margin_bottom(12);
    column.append(&caption);
    column.append(&stack);

    let search = Rc::new(Search {
        column,
        open: Cell::new(false),
        entry: entry.clone(),
        caption,
        results,
        empty,
        stack,
        state: Rc::clone(state),
        daemon: Rc::clone(daemon),
        actions,
        pending: RefCell::new(None),
        generation: Cell::new(0),
        watcher: RefCell::new(None),
    });

    let typing = Rc::clone(&search);
    entry.connect_search_changed(move |entry| typing.schedule(&entry.text()));

    // Escape leaves search the way it leaves every other transient mode.
    let escaping = Rc::clone(&search);
    entry.connect_stop_search(move |_| escaping.hide());

    search.show_empty("");
    search
}

impl Search {
    /// The results, which belong in the window's content area.
    pub fn widget(&self) -> Widget {
        self.column.clone().upcast()
    }

    /// The entry, which belongs in the header bar's title position.
    pub fn entry(&self) -> Widget {
        self.entry.clone().upcast()
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Open search and put the caret in the box.
    pub fn show(self: &Rc<Self>) {
        self.open.set(true);
        self.announce();
        self.entry.grab_focus();
    }

    pub fn hide(self: &Rc<Self>) {
        self.cancel_pending();
        self.open.set(false);
        self.entry.set_text("");
        self.show_empty("");
        self.announce();
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.is_open() {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Tell the window that search mode opened or closed, so it can swap the
    /// header's title and its content to match.
    fn announce(&self) {
        if let Some(watcher) = self.watcher.borrow().as_ref() {
            watcher(self.open.get());
        }
    }

    /// Set the one callback that keeps the window in step with this pane.
    pub fn on_toggle(&self, watcher: impl Fn(bool) + 'static) {
        *self.watcher.borrow_mut() = Some(Box::new(watcher));
    }

    /// Ask, once the typing stops.
    fn schedule(self: &Rc<Self>, query: &str) {
        self.cancel_pending();

        let query = query.trim().to_string();
        if query.chars().count() < MIN_QUERY_CHARS {
            // Said immediately rather than after the debounce: the answer does
            // not depend on anything being fetched, and a hint that arrives a
            // beat late reads as a result that failed to appear.
            self.show_empty(&query);
            return;
        }

        let this = Rc::clone(self);
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(DEBOUNCE_MS as u64),
            move || {
                *this.pending.borrow_mut() = None;
                this.run(query.clone());
            },
        );
        *self.pending.borrow_mut() = Some(id);
    }

    fn cancel_pending(&self) {
        if let Some(id) = self.pending.borrow_mut().take() {
            id.remove();
        }
    }

    /// Put the question to the daemon.
    fn run(self: &Rc<Self>, query: String) {
        let Some(proxy) = self.daemon.borrow().clone() else {
            self.show_message(
                "Search needs the background service",
                "Backtrack's background service is not running, so there is nothing to ask.",
            );
            return;
        };

        let wanted = self.generation.get() + 1;
        self.generation.set(wanted);
        let this = Rc::clone(self);
        crate::ui::spawn(async move {
            let answer = proxy.search_files(&query).await;
            if this.generation.get() != wanted {
                // Superseded: somebody kept typing.
                return;
            }
            match answer {
                Ok(hits) => {
                    debug!(query, hits = hits.len(), "search answered");
                    this.show_hits(&query, hits);
                }
                Err(error) => {
                    warn!(%error, query, "the search could not be run");
                    this.show_message("Nothing found", &error.to_string());
                }
            }
        });
    }

    fn show_hits(self: &Rc<Self>, query: &str, hits: Vec<SearchResult>) {
        if hits.is_empty() {
            self.show_empty(query);
            return;
        }

        let tz = glib::TimeZone::local();
        let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
        let home = crate::ui::breadcrumb::home_archive_path();
        let cards = copy::cards(&hits, home.as_deref(), now, &tz);

        self.caption.set_text(&copy::caption(
            cards.len(),
            self.state.view().archives.len(),
        ));
        self.caption.set_visible(true);

        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        for (card, hit) in cards.iter().zip(hits) {
            self.results.append(&self.row(card, hit));
        }
        self.stack.set_visible_child_name("results");
    }

    /// One result card.
    fn row(self: &Rc<Self>, card: &copy::Card, hit: SearchResult) -> adw::ActionRow {
        let row = adw::ActionRow::new();
        row.set_title(&glib::markup_escape_text(&card.name));
        row.set_subtitle(&glib::markup_escape_text(&format!(
            "{}\n{}",
            card.breadcrumb, card.lifespan
        )));
        row.set_subtitle_lines(2);
        row.add_prefix(&gtk4::Image::from_gicon(&icon_for(card)));

        if card.gone {
            let tag = Label::new(Some("no longer on your disk"));
            tag.add_css_class("badge");
            tag.add_css_class("deleted");
            tag.set_valign(Align::Center);
            row.add_suffix(&tag);
        }

        let buttons = GtkBox::new(Orientation::Horizontal, 8);
        buttons.set_valign(Align::Center);

        let view = gtk4::Button::with_label("View in Timeline");
        let viewer = Rc::clone(self);
        let viewed = hit.clone();
        view.connect_clicked(move |_| {
            (viewer.actions.view_in_timeline)(&viewed);
            viewer.hide();
        });
        buttons.append(&view);

        // Offered only where it is the answer. A file still sitting on the
        // computer does not need putting back, and a button that restores it
        // over itself is an invitation to a conflict dialog about nothing.
        if card.gone {
            let restore = gtk4::Button::with_label(match card.kind {
                Kind::Dir => "Restore Folder",
                _ => "Restore Latest Version",
            });
            restore.add_css_class("suggested-action");
            let restorer = Rc::clone(self);
            let restored = hit.clone();
            restore.connect_clicked(move |_| {
                (restorer.actions.restore_latest)(&restored);
                restorer.hide();
            });
            buttons.append(&restore);
        }

        row.add_suffix(&buttons);
        row
    }

    fn show_empty(&self, query: &str) {
        let (title, body) = copy::empty_state(query, MIN_QUERY_CHARS);
        self.show_message(title, &body);
    }

    fn show_message(&self, title: &str, body: &str) {
        self.caption.set_visible(false);
        self.empty.set_title(title);
        self.empty.set_description(Some(body));
        self.stack.set_visible_child_name("empty");
    }
}

/// The themed icon for a result, chosen the way the file pane chooses one:
/// from the name, because the catalogue records no content type.
fn icon_for(card: &copy::Card) -> gio::Icon {
    match card.kind {
        Kind::Dir => gio::ThemedIcon::new("folder").upcast(),
        Kind::Symlink => gio::ThemedIcon::new("emblem-symbolic-link").upcast(),
        _ => {
            let (content_type, _) = gio::content_type_guess(Some(&card.name), None);
            gio::content_type_get_icon(&content_type)
        }
    }
}
