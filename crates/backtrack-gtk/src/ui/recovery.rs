// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Keeping the recovery key: saved to a file, or printed on paper.
//!
//! Used by the wizard's last page, where one of the two is required before
//! anything else happens, and by Preferences → Security, where both stay
//! available for as long as the backups exist.
//!
//! Each returns whether the key was actually kept. A dialog cancelled halfway
//! is not an error, but it is not a kept key either, and the wizard's gate
//! depends on telling the two apart.

use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, pango};
use tracing::{info, warn};

use crate::model::wizard::{recovery_file_name, recovery_sheet, Sheet};

/// Offer the key as a file. The file is exactly what `borg key export`
/// wrote, so `borg key import` reads it back without editing: anything added
/// to it, even a friendly first line, would break that.
///
/// Written readable by its owner only. The key is useless without the
/// passphrase, but there is no reason for anybody else on the machine to
/// have half of what they need.
pub async fn save(parent: &gtk4::Window, key: &str, host: &str) -> Result<bool, String> {
    // The portal's own chooser under Flatpak, and the toolkit's outside it.
    let dialog = gtk4::FileDialog::builder()
        .title("Save Recovery Key")
        .initial_name(recovery_file_name(host))
        .accept_label("Save")
        .modal(true)
        .build();
    let Ok(file) = dialog.save_future(Some(parent)).await else {
        info!("saving the recovery key was cancelled at the file chooser");
        return Ok(false);
    };
    let flags = gio::FileCreateFlags::REPLACE_DESTINATION | gio::FileCreateFlags::PRIVATE;
    file.replace_contents_future(key.as_bytes().to_vec(), None, false, flags)
        .await
        .map_err(|(_, error)| {
            warn!(%error, "the recovery key could not be written");
            format!("The recovery key could not be saved: {error}")
        })?;
    info!(uri = %file.uri(), "recovery key saved");
    Ok(true)
}

/// Offer the key on paper, through the print dialog. "Print to File" counts:
/// a PDF somebody keeps is as kept as a sheet in a drawer.
pub async fn print(
    parent: &gtk4::Window,
    key: &str,
    repository: &str,
    host: &str,
) -> Result<bool, String> {
    let printed = glib::DateTime::now_local()
        .ok()
        .map(|now| crate::model::format::at(now.to_unix(), &glib::TimeZone::local(), "%e %B %Y"))
        .unwrap_or_default();
    let sheet = recovery_sheet(key, repository, host, &printed);

    let operation = gtk4::PrintOperation::new();
    operation.set_job_name("Backtrack recovery key");
    operation.set_n_pages(1);
    // Asynchronous, so the window keeps drawing while the dialog is up.
    operation.set_allow_async(true);
    operation.connect_draw_page(move |_, context, _| draw(context, &sheet));

    let (done, answer) = async_channel::bounded(1);
    operation.connect_done(move |operation, result| {
        let outcome = match result {
            gtk4::PrintOperationResult::Apply => Ok(true),
            gtk4::PrintOperationResult::Error => Err(operation
                .error()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "the printer reported a problem".into())),
            _ => Ok(false),
        };
        let _ = done.try_send(outcome);
    });

    match operation.run(gtk4::PrintOperationAction::PrintDialog, Some(parent)) {
        // The dialog is still up; `done` answers when it closes.
        Ok(gtk4::PrintOperationResult::InProgress) => {}
        Ok(gtk4::PrintOperationResult::Apply) => return Ok(true),
        Ok(_) => return Ok(false),
        Err(error) => return Err(format!("The recovery key could not be printed: {error}")),
    }
    let kept = answer.recv().await.unwrap_or(Ok(false));
    match &kept {
        Ok(true) => info!("recovery key printed"),
        Ok(false) => info!("printing the recovery key was cancelled"),
        Err(error) => warn!(%error, "printing the recovery key failed"),
    }
    kept.map_err(|error| format!("The recovery key could not be printed: {error}"))
}

/// Lay the sheet out on the page.
///
/// Drawn through a GTK snapshot rather than with Pango's Cairo functions,
/// which would be a dependency of their own for the sake of one call: the
/// snapshot turns the same layout into a render node, and a render node can
/// draw itself onto the print context's Cairo surface.
fn draw(context: &gtk4::PrintContext, sheet: &Sheet) {
    let layout = context.create_pango_layout();
    layout.set_width((context.width() * f64::from(pango::SCALE)) as i32);
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_markup(&markup(sheet));

    let snapshot = gtk4::Snapshot::new();
    snapshot.append_layout(&layout, &gdk::RGBA::BLACK);
    if let Some(node) = snapshot.to_node() {
        node.draw(&context.cairo_context());
    }
}

fn markup(sheet: &Sheet) -> String {
    let escape = |text: &str| glib::markup_escape_text(text).to_string();
    let mut out = format!(
        "<span size=\"x-large\" weight=\"bold\">{}</span>\n\n",
        escape(sheet.title)
    );
    for (label, value) in &sheet.facts {
        out.push_str(&format!("<b>{}:</b> {}\n", escape(label), escape(value)));
    }
    out.push_str(&format!(
        "\n<span font_family=\"monospace\" size=\"small\">{}</span>\n\n",
        escape(&sheet.key)
    ));
    for line in &sheet.advice {
        out.push_str(&escape(line));
        out.push_str("\n\n");
    }
    out
}
