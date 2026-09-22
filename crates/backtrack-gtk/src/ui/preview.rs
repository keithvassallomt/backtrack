// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The preview pane: this file, as it was then.
//!
//! Everything else in the window is answered from the local catalogue, which
//! is why it is instant. A preview is the one thing that cannot be: the bytes
//! live in the repository and only the daemon can get them out, which means a
//! `borg extract` and, if the destination is a slow disk or a network share,
//! a wait.
//!
//! So the pane is built in two halves. What the index already knows — the
//! name, the size, when it was last modified — appears the moment you select
//! something, because it is already in hand. The contents arrive afterwards,
//! and while they do there is a spinner and a way to stop.
//!
//! Two things keep a fast stroll through a folder from turning into a queue of
//! extractions. The fetch waits a moment before it starts, so arrow-keying
//! past twenty files asks for none of them; and a fetch that is superseded is
//! dropped rather than left to finish into a pane that has moved on.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{
    gdk, gio, glib, Align, Box as GtkBox, Button, Label, Orientation, Picture, ScrolledWindow,
    Spinner, Stack, TextView, Widget, WrapMode,
};
use libadwaita as adw;
use tracing::{debug, warn};

use crate::daemon::Daemon1Proxy;
use crate::model::format;
use crate::state::{AppState, Change, Selected};

/// How long to wait for the selection to settle before asking the daemon for
/// anything. Long enough that stepping through a folder costs no extractions,
/// short enough to read as immediate when you stop.
const SETTLE: Duration = Duration::from_millis(180);

/// How much of a file is read. Beyond this a preview is not a preview.
const FETCH_LIMIT: usize = 8 * 1024 * 1024;

/// How much of a text file is shown. The rest is a document, not a preview.
const TEXT_LIMIT: usize = 64 * 1024;

/// The pane, and the fetch it currently has in flight.
pub struct Preview {
    container: GtkBox,
    title: Label,
    subtitle: Label,
    stack: Stack,
    text: TextView,
    picture: Picture,
    status: adw::StatusPage,
    retry: Button,
    /// The fetch in flight, and which one it is.
    current: Current,
    state: Rc<AppState>,
    daemon: Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
}

/// Build the preview pane.
pub fn build(
    state: &Rc<AppState>,
    daemon: &Rc<RefCell<Option<Daemon1Proxy<'static>>>>,
) -> Rc<Preview> {
    let title = Label::builder()
        .halign(Align::Start)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .build();
    title.add_css_class("heading");

    let subtitle = Label::builder().halign(Align::Start).build();
    subtitle.add_css_class("dim-label");
    subtitle.add_css_class("caption");

    let text = TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    let text_scroller = ScrolledWindow::builder().child(&text).vexpand(true).build();

    let picture = Picture::builder()
        .content_fit(gtk4::ContentFit::ScaleDown)
        .vexpand(true)
        .build();

    let status = adw::StatusPage::builder().vexpand(true).build();
    status.add_css_class("compact");

    let retry = Button::with_label("Show preview");
    retry.set_halign(Align::Center);
    status.set_child(Some(&retry));

    let spinner = Spinner::builder().spinning(true).build();
    spinner.set_size_request(32, 32);
    let cancel = Button::with_label("Stop");
    cancel.set_halign(Align::Center);
    let loading = GtkBox::new(Orientation::Vertical, 12);
    loading.set_valign(Align::Center);
    loading.append(&spinner);
    loading.append(&Label::new(Some("Fetching this version…")));
    loading.append(&cancel);

    let stack = Stack::new();
    stack.add_named(&status, Some("status"));
    stack.add_named(&loading, Some("loading"));
    stack.add_named(&text_scroller, Some("text"));
    stack.add_named(&picture, Some("image"));
    stack.set_vexpand(true);

    let container = GtkBox::new(Orientation::Vertical, 6);
    container.add_css_class("card");
    container.set_margin_top(0);
    let heading = GtkBox::new(Orientation::Vertical, 2);
    heading.set_margin_top(10);
    heading.set_margin_start(12);
    heading.set_margin_end(12);
    heading.append(&title);
    heading.append(&subtitle);
    container.append(&heading);
    container.append(&stack);

    let preview = Rc::new(Preview {
        container,
        title,
        subtitle,
        stack,
        text,
        picture,
        status,
        retry,
        current: Current::default(),
        state: Rc::clone(state),
        daemon: Rc::clone(daemon),
    });

    let stopper = Rc::clone(&preview);
    cancel.connect_clicked(move |_| {
        stopper.cancel();
        stopper.offer_retry("Preview stopped", "Nothing was changed.");
    });

    let again = Rc::clone(&preview);
    preview.retry.connect_clicked(move |_| again.refresh(true));

    let watcher = Rc::clone(&preview);
    state.subscribe(move |_, change| {
        if matches!(change, Change::Selection | Change::Seq | Change::Folder) {
            watcher.refresh(false);
        }
    });

    preview.refresh(false);
    preview
}

impl Preview {
    /// The widget to put in the window.
    pub fn widget(&self) -> Widget {
        self.container.clone().upcast()
    }

    /// Try again now — used when the service arrives after the window did.
    pub fn refresh_now(self: &Rc<Self>) {
        self.refresh(false);
    }

    /// Show what the index knows, then go and get the rest.
    fn refresh(self: &Rc<Self>, forced: bool) {
        self.cancel();

        let view = self.state.view();
        let Some(selected) = view.selected.clone() else {
            self.title.set_text("Nothing selected");
            self.subtitle.set_text("");
            self.show_status(
                "edit-select-all-symbolic",
                "Select a file to preview it",
                "",
            );
            return;
        };

        self.title.set_text(&selected.name);
        self.subtitle.set_text(&describe(&selected));

        if selected.is_dir {
            self.show_status(
                "folder-symbolic",
                "Folder",
                "Select a file inside it to see what it held.",
            );
            return;
        }

        let Some(archive) = view.archive().map(|a| a.name.clone()) else {
            self.show_status("dialog-information-symbolic", "No backup selected", "");
            return;
        };

        let this = Rc::clone(self);
        let wanted = self.current.begin();
        self.stack.set_visible_child_name("loading");

        self.current.run(async move {
            // Let the selection settle. A fetch cancelled during this wait
            // never reaches the daemon at all, which is what keeps arrowing
            // through a folder from queueing an extraction per file.
            if !forced {
                glib::timeout_future(SETTLE).await;
            }
            this.fetch(wanted, archive, selected).await;
        });
    }

    /// Ask the daemon for the bytes and decide what they are.
    async fn fetch(self: &Rc<Self>, wanted: u64, archive: String, selected: Selected) {
        let proxy = self.daemon.borrow().clone();
        let Some(proxy) = proxy else {
            self.offer_retry(
                "Preview needs the Backtrack service",
                "Browsing works without it, but reading a file out of a backup does not.",
            );
            return;
        };

        debug!(archive, path = selected.path, "fetching a preview");
        let descriptor = match proxy.preview_file(&archive, &selected.path).await {
            Ok(descriptor) => descriptor,
            Err(error) => {
                if !self.current.is_current(wanted) {
                    return;
                }
                warn!(%error, path = selected.path, "the preview could not be fetched");
                self.offer_retry("This version could not be read", &clean(&error.to_string()));
                return;
            }
        };

        debug!(path = selected.path, "the service handed over a descriptor");

        // Reading the descriptor is ordinary blocking I/O, so it happens on
        // GLib's worker pool rather than on the frame clock.
        let read = gio::spawn_blocking(move || read_capped(descriptor.into())).await;

        if !self.current.is_current(wanted) {
            debug!(
                path = selected.path,
                "preview superseded before it was shown"
            );
            return;
        }

        match read {
            Ok(Ok(bytes)) => {
                debug!(path = selected.path, bytes = bytes.len(), "preview ready");
                self.show_bytes(&selected, bytes)
            }
            Ok(Err(error)) => {
                warn!(%error, path = selected.path, "the preview could not be read");
                self.offer_retry("This version could not be read", &error);
            }
            Err(_) => self.offer_retry("This version could not be read", "The read was dropped."),
        }
    }

    /// Text if it reads as text, a picture if it decodes as one, and an honest
    /// "not here" otherwise.
    ///
    /// The decision is made on the bytes rather than on the file name, because
    /// what matters is whether the pane can show the thing — and because the
    /// name is often wrong about it.
    fn show_bytes(self: &Rc<Self>, selected: &Selected, bytes: Vec<u8>) {
        if let Some(text) = as_text(&bytes) {
            self.text.buffer().set_text(&text);
            self.stack.set_visible_child_name("text");
            return;
        }
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
            self.picture.set_paintable(Some(&texture));
            self.stack.set_visible_child_name("image");
            return;
        }
        // PDFs and office documents land here. Rendering a PDF page needs
        // poppler, which is a dependency this stage does not take on for one
        // thumbnail; the file restores perfectly well without it.
        self.show_status(
            "document-open-symbolic",
            "No preview for this kind of file",
            &format!("Restore “{}” to open it.", selected.name),
        );
    }

    /// Stop whatever is in flight.
    fn cancel(&self) {
        self.current.cancel();
    }

    fn show_status(&self, icon: &str, title: &str, description: &str) {
        self.status.set_icon_name(Some(icon));
        self.status.set_title(title);
        self.status
            .set_description((!description.is_empty()).then_some(description));
        self.retry.set_visible(false);
        self.stack.set_visible_child_name("status");
    }

    fn offer_retry(&self, title: &str, description: &str) {
        self.show_status("view-refresh-symbolic", title, description);
        self.retry.set_visible(true);
    }
}

/// The one fetch a pane is allowed to have in flight.
///
/// Two mechanisms, because they cover different halves of the problem.
/// Aborting the task drops the D-Bus call with it, so a superseded fetch stops
/// costing anything; and the token says whether an answer that was already on
/// its way still belongs to the file on screen. Without the second, a reply
/// that crossed the cancellation would paint the wrong file's contents into
/// the pane.
#[derive(Default)]
struct Current {
    generation: Cell<u64>,
    inflight: RefCell<Option<glib::JoinHandle<()>>>,
}

impl Current {
    /// Abandon whatever was running and take a token for the new fetch.
    fn begin(&self) -> u64 {
        self.cancel();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        generation
    }

    /// Whether `token` is still the fetch the pane is waiting for.
    fn is_current(&self, token: u64) -> bool {
        self.generation.get() == token
    }

    /// Run the fetch on the main loop, keeping its handle so it can be dropped.
    fn run(&self, task: impl std::future::Future<Output = ()> + 'static) {
        *self.inflight.borrow_mut() = Some(glib::spawn_future_local(task));
    }

    fn cancel(&self) {
        if let Some(handle) = self.inflight.borrow_mut().take() {
            handle.abort();
        }
    }
}

/// The metadata line: what the index already knows about the selected entry.
fn describe(selected: &Selected) -> String {
    let tz = glib::TimeZone::local();
    match (selected.is_dir, selected.size >= 0, selected.mtime > 0) {
        (true, _, _) => "Folder".to_string(),
        (false, true, true) => format!(
            "{} · {}",
            format::size(selected.size, backtrack_core::index::Kind::File),
            format::modified(selected.mtime, &tz)
        ),
        (false, true, false) => format::size(selected.size, backtrack_core::index::Kind::File),
        _ => String::new(),
    }
}

/// Read at most [`FETCH_LIMIT`] bytes from the descriptor the daemon handed over.
pub(crate) fn read_capped(descriptor: std::os::fd::OwnedFd) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::from(descriptor);
    let mut bytes = Vec::new();
    file.take(FETCH_LIMIT as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// The leading text of `bytes`, if they are text at all.
///
/// A NUL byte anywhere in the sample settles it — that is how `file`, `grep`
/// and every other tool decides, and it is right far more often than a file
/// extension is.
pub(crate) fn as_text(bytes: &[u8]) -> Option<String> {
    let sample = &bytes[..bytes.len().min(TEXT_LIMIT)];
    if sample.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(sample) {
        Ok(text) => text.to_string(),
        // A cut in the middle of a multi-byte character is a truncation, not a
        // reason to refuse the file.
        Err(error) if error.valid_up_to() > 0 && sample.len() == TEXT_LIMIT => {
            String::from_utf8_lossy(&sample[..error.valid_up_to()]).to_string()
        }
        Err(_) => return None,
    };
    let truncated = bytes.len() > TEXT_LIMIT;
    Some(if truncated {
        format!("{text}\n\n… preview truncated at 64 KB …\n")
    } else {
        text
    })
}

/// Strip the D-Bus error-name prefix, which is addressed to programs.
fn clean(message: &str) -> String {
    message
        .rsplit_once(": ")
        .map_or(message, |(_, tail)| tail)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beginning_a_fetch_retires_the_one_before_it() {
        let current = Current::default();
        let first = current.begin();
        let second = current.begin();
        assert!(
            !current.is_current(first),
            "the first answer is no longer wanted"
        );
        assert!(current.is_current(second));
    }

    #[test]
    fn a_superseded_fetch_never_runs() {
        // Twenty files arrowed past in a moment: each one retires the last
        // before it gets anywhere near the daemon, and only the file the
        // selection settles on is fetched at all.
        let context = glib::MainContext::new();
        let reached = Rc::new(Cell::new(0));
        let counted = Rc::clone(&reached);

        // The tasks are spawned onto the thread-default context, so the test
        // has to make its own context that, or nothing it spawns is ever
        // polled.
        context
            .with_thread_default(|| {
                let current = Rc::new(Current::default());
                for _ in 0..20 {
                    let token = current.begin();
                    let guard = Rc::clone(&current);
                    let counter = Rc::clone(&counted);
                    current.run(async move {
                        glib::timeout_future(SETTLE).await;
                        if guard.is_current(token) {
                            counter.set(counter.get() + 1);
                        }
                    });
                }
                // Long enough for every one of them to have got past the settle.
                context.block_on(glib::timeout_future(SETTLE * 3));
            })
            .unwrap();

        assert_eq!(
            reached.get(),
            1,
            "only the last selection should have reached the daemon"
        );
    }

    #[test]
    fn plain_text_is_shown_as_text() {
        let text = as_text(b"report final signed version!!").unwrap();
        assert_eq!(text, "report final signed version!!");
    }

    #[test]
    fn binary_is_not_mistaken_for_text() {
        // A PNG header: the NUL in the signature is what gives it away.
        assert!(as_text(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").is_none());
    }

    #[test]
    fn a_long_file_is_truncated_and_says_so() {
        let long = vec![b'a'; TEXT_LIMIT + 10];
        let text = as_text(&long).unwrap();
        assert!(text.contains("truncated"));
        assert!(text.len() < long.len() + 100);
    }

    #[test]
    fn a_character_cut_in_half_by_the_limit_does_not_lose_the_file() {
        // The last character straddles the cut, which is a truncation rather
        // than a reason to call a text file binary.
        let mut bytes = vec![b'a'; TEXT_LIMIT - 1];
        bytes.extend_from_slice("é".as_bytes());
        let text = as_text(&bytes).expect("still text");
        assert!(text.starts_with("aaa"));
    }

    #[test]
    fn invalid_encoding_well_inside_the_sample_is_not_text() {
        assert!(as_text(b"abc\xff\xfe def").is_none());
    }

    #[test]
    fn an_error_is_shown_without_its_dbus_name() {
        assert_eq!(
            clean("org.backtrack.Error.NotFound: no such path in that archive"),
            "no such path in that archive"
        );
        assert_eq!(clean("something went wrong"), "something went wrong");
    }
}
