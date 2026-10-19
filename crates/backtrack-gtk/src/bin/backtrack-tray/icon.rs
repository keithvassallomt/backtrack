// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The icon's picture, as pixels (S12-T8).
//!
//! A StatusNotifierItem can name its icon or carry it. A name has to be found
//! in an icon theme by the panel, and a panel that read its theme before
//! Backtrack was installed may never see the icon: Quickshell, and with it
//! Caelestia's bar, keeps the lookups it made for as long as it runs, so an
//! icon installed after it started stays missing until the shell restarts.
//! The other programs in such a tray hand over the icon itself, as pixels or
//! a file's path, and this one does the same.
//!
//! The pixels are the application icon rendered at the sizes panels ask for
//! (`just tray-icons`), built into the program. That also serves a
//! development build, whose icon no theme has, and the Flatpak, whose files
//! a panel outside the sandbox cannot read.

use gtk4::{gdk_pixbuf, gio, glib};
use tracing::warn;

/// Each rendered size, with its PNG.
const RENDERED: [(i32, &[u8]); 7] = [
    (16, include_bytes!("icons/16.png")),
    (22, include_bytes!("icons/22.png")),
    (24, include_bytes!("icons/24.png")),
    (32, include_bytes!("icons/32.png")),
    (48, include_bytes!("icons/48.png")),
    (64, include_bytes!("icons/64.png")),
    (128, include_bytes!("icons/128.png")),
];

/// The icon at every rendered size, as a panel takes it. A size that cannot
/// be decoded is left out, and said in the log; the panel picks from the rest.
pub fn pixmaps() -> Vec<ksni::Icon> {
    RENDERED
        .iter()
        .filter_map(|&(size, png)| match decode(png) {
            Ok(icon) if icon.width == size && icon.height == size => Some(icon),
            Ok(icon) => {
                warn!(
                    size,
                    width = icon.width,
                    height = icon.height,
                    "a tray icon is not the size it is filed under"
                );
                None
            }
            Err(error) => {
                warn!(%error, size, "a tray icon could not be decoded");
                None
            }
        })
        .collect()
}

/// A PNG as a tray icon.
fn decode(png: &'static [u8]) -> Result<ksni::Icon, glib::Error> {
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(png));
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream(&stream, None::<&gio::Cancellable>)?;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let data = argb(
        &pixbuf.read_pixel_bytes(),
        width as usize,
        height as usize,
        pixbuf.rowstride() as usize,
        pixbuf.n_channels() as usize,
    );
    Ok(ksni::Icon {
        width,
        height,
        data,
    })
}

/// Rows of RGBA (or RGB) pixels, `rowstride` bytes apart, as the tray
/// protocol wants them: ARGB32 in network byte order, one row after another.
fn argb(pixels: &[u8], width: usize, height: usize, rowstride: usize, channels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let at = y * rowstride + x * channels;
            let alpha = if channels == 4 {
                pixels[at + 3]
            } else {
                u8::MAX
            };
            out.extend_from_slice(&[alpha, pixels[at], pixels[at + 1], pixels[at + 2]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_go_alpha_first_row_after_row() {
        // Two pixels per row, rows padded to 12 bytes as a pixbuf pads them.
        let rgba = [
            10, 20, 30, 40, 50, 60, 70, 80, 0, 0, 0, 0, //
            90, 100, 110, 120, 130, 140, 150, 160, 0, 0, 0, 0,
        ];
        assert_eq!(
            argb(&rgba, 2, 2, 12, 4),
            [40, 10, 20, 30, 80, 50, 60, 70, 120, 90, 100, 110, 160, 130, 140, 150]
        );
        // Without an alpha channel every pixel is opaque.
        assert_eq!(
            argb(&[1, 2, 3, 4, 5, 6], 2, 1, 6, 3),
            [255, 1, 2, 3, 255, 4, 5, 6]
        );
    }

    #[test]
    fn every_size_is_there_and_has_the_icon_in_it() {
        let icons = pixmaps();
        let sizes: Vec<i32> = icons.iter().map(|icon| icon.width).collect();
        assert_eq!(sizes, [16, 22, 24, 32, 48, 64, 128]);
        for icon in icons {
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            // The icon is drawn, not blank: most of it is opaque, with the
            // corners of its rounded square left clear.
            let opaque = icon.data.chunks(4).filter(|p| p[0] == u8::MAX).count();
            let pixels = (icon.width * icon.height) as usize;
            assert!(
                opaque * 2 > pixels,
                "{}px: {opaque} of {pixels} opaque",
                icon.width
            );
            assert_eq!(
                icon.data[0], 0,
                "{}px: the top-left corner is clear",
                icon.width
            );
        }
    }
}
