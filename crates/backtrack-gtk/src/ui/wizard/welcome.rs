// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Step 1, mockup 10: the four promises, Get Started, and the way in for
//! somebody who already has backups.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};
use libadwaita as adw;

use super::Wizard;

/// What Backtrack does, in the mockup's words, each beside its icon.
const PROMISES: &[(&str, &str)] = &[
    ("alarm-symbolic", "Backs up every hour, automatically"),
    (
        "media-seek-backward-symbolic",
        "Slide back in time to any version",
    ),
    (
        "system-lock-screen-symbolic",
        "Encrypted, storage-efficient backups",
    ),
    ("folder-symbolic", "Restore right from your file manager"),
];

pub fn build(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let body = GtkBox::new(Orientation::Vertical, 12);
    body.set_valign(Align::Center);
    body.set_margin_top(24);
    body.set_margin_bottom(24);
    body.set_margin_start(24);
    body.set_margin_end(24);

    let icon = gtk4::Image::from_icon_name(crate::ui::app_icon_name(&wizard.window));
    icon.set_pixel_size(128);
    icon.set_margin_bottom(12);
    body.append(&icon);

    let title = Label::new(Some("Welcome to Backtrack"));
    title.add_css_class("title-1");
    title.set_wrap(true);
    body.append(&title);
    let tagline = Label::new(Some("Browse your files as they were."));
    tagline.add_css_class("title-4");
    tagline.add_css_class("dim-label");
    tagline.set_wrap(true);
    body.append(&tagline);

    let promises = GtkBox::new(Orientation::Vertical, 12);
    promises.set_halign(Align::Center);
    promises.set_margin_top(18);
    promises.set_margin_bottom(18);
    for (icon, text) in PROMISES {
        let row = GtkBox::new(Orientation::Horizontal, 18);
        let image = gtk4::Image::from_icon_name(icon);
        image.add_css_class("wizard-promise");
        row.append(&image);
        let label = Label::new(Some(text));
        label.set_xalign(0.0);
        label.set_wrap(true);
        row.append(&label);
        promises.append(&row);
    }
    body.append(&promises);

    let start = Button::with_label("Get Started");
    start.add_css_class("pill");
    start.add_css_class("suggested-action");
    start.set_halign(Align::Center);
    start.set_width_request(280);
    let nav = wizard.nav.clone();
    start.connect_clicked(move |_| nav.push_by_tag("what"));
    body.append(&start);

    // A second run is changing a computer that already has backups; bringing
    // somebody else's in is the Welcome page's job on a new computer, and
    // Storage → Change… is the way to point this one elsewhere.
    if wizard.before.is_none() {
        let label = Label::new(Some("Already have backups? Import…"));
        label.add_css_class("accent");
        let import = Button::builder()
            .child(&label)
            .halign(Align::Center)
            .build();
        import.add_css_class("flat");
        let importer = Rc::clone(wizard);
        import.connect_clicked(move |_| {
            *importer.import.borrow_mut() = None;
            importer.nav.push_by_tag("import");
        });
        body.append(&import);
    }

    let clamp = adw::Clamp::builder().maximum_size(520).child(&body).build();
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));

    adw::NavigationPage::builder()
        .title("Backtrack")
        .tag("welcome")
        .child(&view)
        .build()
}
