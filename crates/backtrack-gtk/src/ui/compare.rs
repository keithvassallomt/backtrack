// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! "Compare with Today": the backed-up version beside the one on the computer.
//!
//! Bob's check before he commits to anything. A restore is reversible — that
//! is what the safety stash is for — but reversible is not the same as free,
//! and the question "what would I actually get back" deserves an answer that
//! does not involve carrying it out first.
//!
//! Both sides arrive as descriptors from the daemon: the backup through
//! `PreviewFile`, out of its extraction cache, and today's through `LiveFile`.
//! Neither is opened by this application, which has no path to either.
//!
//! What the window can say depends on what the file is, and it says the most
//! it honestly can. Text gets a line diff. Images get both versions drawn,
//! because "these pixels differ" is not something a person can read but two
//! pictures side by side is. Anything else gets its size and date and the
//! plain fact of whether the bytes are the same, which is less than a diff and
//! is still the question being asked.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, glib, Align, Box as GtkBox, Button, Label, Orientation, ScrolledWindow, Widget};
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::warn;

use crate::daemon::Daemon1Proxy;
use crate::model::compare::{self, Mark};
use crate::state::Selected;
use crate::ui::preview::{as_text, read_capped};

/// The daemon, as this window holds it.
type Service = Rc<RefCell<Option<Daemon1Proxy<'static>>>>;
use std::cell::RefCell;

/// What the two sides turned out to be.
enum Kind {
    /// Both read as text: a line diff, and a count of the sections that differ.
    Text(compare::TextComparison),
    /// Both decode as images: draw them and let the eye do it.
    Images(Box<(gdk::Texture, gdk::Texture)>),
    /// Neither. The bytes still answer whether anything changed.
    Opaque { differ: bool },
}

/// Open the comparison for `selected` as of `archive`.
///
/// `restore` is called if the person decides they want the backed-up version,
/// so the window does not need to know how a restore is carried out — that is
/// Stage 7's, and it already knows how to ask about conflicts.
pub fn present(
    parent: &adw::ApplicationWindow,
    daemon: &Service,
    archive: String,
    selected: Selected,
    restore: impl Fn() + 'static,
) {
    let window = adw::Window::builder()
        .transient_for(parent)
        .title(format!("Compare: {}", selected.name))
        .default_width(960)
        .default_height(680)
        .build();

    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());

    let body = GtkBox::new(Orientation::Vertical, 0);
    body.append(&legend());

    let loading = adw::StatusPage::builder()
        .icon_name("view-dual-symbolic")
        .title("Reading both versions")
        .vexpand(true)
        .build();
    body.append(&loading);
    content.set_content(Some(&body));
    window.set_content(Some(&content));
    window.present();

    let Some(proxy) = daemon.borrow().clone() else {
        replace_body(
            &body,
            &unavailable("Backtrack cannot compare without its background service"),
        );
        return;
    };

    let restore = Rc::new(restore);
    let window_ref = window.clone();
    crate::ui::spawn(async move {
        match fetch(&proxy, &archive, &selected).await {
            Ok((kind, today_size, today_mtime)) => {
                let view = build(
                    &kind,
                    &selected,
                    today_size,
                    today_mtime,
                    {
                        let window = window_ref.clone();
                        let restore = Rc::clone(&restore);
                        move || {
                            restore();
                            window.close();
                        }
                    },
                    {
                        let window = window_ref.clone();
                        move || window.close()
                    },
                );
                replace_body(&body, &view);
            }
            Err(error) => {
                warn!(%error, path = selected.path, "the two versions could not be compared");
                replace_body(&body, &unavailable(&error));
            }
        }
    });
}

/// Swap whatever is under the legend for `view`.
fn replace_body(body: &GtkBox, view: &Widget) {
    while let Some(child) = body.last_child() {
        if child.has_css_class("compare-legend") {
            break;
        }
        body.remove(&child);
    }
    body.append(view);
}

/// Read both sides and work out what they are.
///
/// Returns today's size and modification time alongside, taken from the
/// descriptor rather than from the catalogue: the catalogue describes the file
/// as the backup found it, and the whole point of this window is that the one
/// on the computer is no longer that.
async fn fetch(
    proxy: &Daemon1Proxy<'static>,
    archive: &str,
    selected: &Selected,
) -> Result<(Kind, i64, i64), String> {
    let backup_fd = proxy
        .preview_file(archive, &selected.path)
        .await
        .map_err(|e| format!("The backed-up version could not be read: {e}"))?;
    let today_fd = proxy
        .live_file(&selected.path)
        .await
        .map_err(|e| format!("The version on this computer could not be read: {e}"))?;

    let today_file = std::fs::File::from(std::os::fd::OwnedFd::from(today_fd));
    let (size, mtime) = match today_file.metadata() {
        Ok(meta) => (
            meta.len() as i64,
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_micros() as i64)
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    };

    let backup = read_capped(std::os::fd::OwnedFd::from(backup_fd)).map_err(|e| e.to_string())?;
    let today = read_capped(today_file.into()).map_err(|e| e.to_string())?;
    Ok((classify(backup, today), size, mtime))
}

/// Decide what kind of comparison these two files support.
///
/// By content, never by name — the same rule the preview pane uses, and for
/// the same reason: what matters is whether the window can show the thing.
fn classify(backup: Vec<u8>, today: Vec<u8>) -> Kind {
    if let (Some(left), Some(right)) = (as_text(&backup), as_text(&today)) {
        return Kind::Text(compare::text(&left, &right));
    }
    let texture = |bytes: Vec<u8>| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok();
    if let (Some(left), Some(right)) = (texture(backup.clone()), texture(today.clone())) {
        return Kind::Images(Box::new((left, right)));
    }
    Kind::Opaque {
        differ: backup != today,
    }
}

/// The coloured key above the panes, which is what makes red and green mean
/// something rather than merely look like something.
fn legend() -> Widget {
    let row = GtkBox::new(Orientation::Horizontal, 24);
    row.add_css_class("compare-legend");
    row.set_halign(Align::Center);
    row.set_margin_top(12);
    row.set_margin_bottom(12);
    for (class, text) in [
        ("added", "added since backup"),
        ("removed", "removed since backup"),
    ] {
        let item = GtkBox::new(Orientation::Horizontal, 8);
        let swatch = GtkBox::new(Orientation::Horizontal, 0);
        swatch.add_css_class("compare-swatch");
        swatch.add_css_class(class);
        swatch.set_size_request(20, 20);
        swatch.set_valign(Align::Center);
        item.append(&swatch);
        item.append(&Label::new(Some(text)));
        row.append(&item);
    }
    row.upcast()
}

/// The whole of the window below the legend: two panes and a footer.
fn build(
    kind: &Kind,
    selected: &Selected,
    today_size: i64,
    today_mtime: i64,
    on_restore: impl Fn() + 'static,
    on_keep: impl Fn() + 'static,
) -> Widget {
    let tz = glib::TimeZone::local();
    let column = GtkBox::new(Orientation::Vertical, 0);

    let panes = GtkBox::new(Orientation::Horizontal, 0);
    panes.set_homogeneous(true);
    panes.add_css_class("compare-panes");
    panes.set_vexpand(true);
    panes.set_margin_start(12);
    panes.set_margin_end(12);

    let backup_heading = compare::pane_heading("Backup", selected.mtime, selected.size, &tz);
    let today_heading = compare::pane_heading("Today", today_mtime, today_size, &tz);

    match kind {
        Kind::Text(comparison) => {
            panes.append(&pane(
                "document-open-recent-symbolic",
                &backup_heading,
                &lines(&comparison.backup, "removed"),
            ));
            panes.append(&pane(
                "text-x-generic-symbolic",
                &today_heading,
                &lines(&comparison.today, "added"),
            ));
        }
        Kind::Images(both) => {
            panes.append(&pane(
                "document-open-recent-symbolic",
                &backup_heading,
                &picture(&both.0),
            ));
            panes.append(&pane(
                "image-x-generic-symbolic",
                &today_heading,
                &picture(&both.1),
            ));
        }
        Kind::Opaque { .. } => {
            panes.append(&pane(
                "document-open-recent-symbolic",
                &backup_heading,
                &nothing_to_show(),
            ));
            panes.append(&pane(
                "text-x-generic-symbolic",
                &today_heading,
                &nothing_to_show(),
            ));
        }
    }
    column.append(&panes);
    column.append(&footer(kind, on_restore, on_keep));
    column.upcast()
}

/// One side: an icon, a heading, and whatever there is to show.
fn pane(icon: &str, heading: &str, body: &Widget) -> Widget {
    let side = GtkBox::new(Orientation::Vertical, 0);
    side.add_css_class("compare-pane");

    let head = GtkBox::new(Orientation::Horizontal, 10);
    head.set_margin_top(14);
    head.set_margin_bottom(10);
    head.set_margin_start(14);
    head.set_margin_end(14);
    head.append(&gtk4::Image::from_icon_name(icon));
    let label = Label::new(Some(heading));
    label.add_css_class("heading");
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    head.append(&label);
    side.append(&head);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(body)
        .vexpand(true)
        .build();
    side.append(&scroller);
    side.upcast()
}

/// One pane's worth of diffed text.
///
/// Labels in a box rather than a text view with tags: every line is its own
/// widget, which is what lets a changed line carry a background the width of
/// the pane and a gap occupy the height of a line without holding any text.
fn lines(rows: &[compare::Line], changed_class: &str) -> Widget {
    let column = GtkBox::new(Orientation::Vertical, 0);
    column.add_css_class("compare-text");
    for row in rows {
        let label = Label::builder()
            .label(if row.mark == Mark::Gap {
                " "
            } else {
                &row.text
            })
            .halign(Align::Fill)
            .xalign(0.0)
            .wrap(false)
            .selectable(row.mark != Mark::Gap)
            // A selectable GtkLabel takes keyboard focus, and a focused one
            // selects the whole of its own text. The first line of the pane
            // therefore arrived highlighted, in a colour that is not one of the
            // two the legend explains — which is the worst thing a window whose
            // entire job is "this bit changed" can do. It also made every line
            // in a long document its own tab stop. Dragging across the text
            // with the pointer still selects it.
            .can_focus(false)
            .build();
        label.add_css_class("compare-line");
        if row.mark == Mark::Changed {
            label.add_css_class(changed_class);
        }
        column.append(&label);
    }
    column.upcast()
}

fn picture(texture: &gdk::Texture) -> Widget {
    let picture = gtk4::Picture::for_paintable(texture);
    picture.set_content_fit(gtk4::ContentFit::ScaleDown);
    picture.set_margin_start(14);
    picture.set_margin_end(14);
    picture.set_margin_bottom(14);
    picture.upcast()
}

fn nothing_to_show() -> Widget {
    let label = Label::new(Some("Not a kind of file that can be shown side by side"));
    label.add_css_class("dim-label");
    label.set_wrap(true);
    label.set_margin_start(14);
    label.set_margin_end(14);
    label.set_margin_bottom(14);
    label.upcast()
}

/// The summary and the two ways out.
fn footer(kind: &Kind, on_restore: impl Fn() + 'static, on_keep: impl Fn() + 'static) -> Widget {
    let bar = GtkBox::new(Orientation::Horizontal, 12);
    bar.add_css_class("toolbar");
    bar.set_margin_top(6);

    let summary = Label::new(Some(&match kind {
        Kind::Text(comparison) => compare::sections_differ(comparison.sections),
        Kind::Images(_) => "Both versions are shown above".to_string(),
        Kind::Opaque { differ: true } => "These files differ".to_string(),
        Kind::Opaque { differ: false } => "These files are identical".to_string(),
    }));
    summary.add_css_class("dim-label");
    bar.append(&summary);

    let buttons = GtkBox::new(Orientation::Horizontal, 8);
    buttons.set_hexpand(true);
    buttons.set_halign(Align::End);

    let keep = Button::with_label("Keep Current Version");
    keep.connect_clicked(move |_| on_keep());
    buttons.append(&keep);

    let restore = Button::with_label("Restore This Version");
    restore.add_css_class("suggested-action");
    restore.connect_clicked(move |_| on_restore());
    buttons.append(&restore);

    bar.append(&buttons);
    bar.upcast()
}

fn unavailable(message: &str) -> Widget {
    adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title("Nothing to compare")
        .description(message)
        .vexpand(true)
        .build()
        .upcast()
}
