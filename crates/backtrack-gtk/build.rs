// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Compile the window's resources (its own icons) into a GResource bundle,
//! which `main` includes and registers. `glib-compile-resources` comes with
//! GLib's development files, which building against GTK needs anyway.

use std::path::Path;
use std::process::Command;

fn main() {
    let icons = "../../packaging/icons/hicolor/symbolic/actions";
    let manifest = "resources/backtrack.gresource.xml";
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let target = Path::new(&out).join("backtrack.gresource");
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir")
        .arg(icons)
        .arg("--target")
        .arg(&target)
        .arg(manifest)
        .status()
        .expect("glib-compile-resources, from GLib's development files, could not be run");
    assert!(status.success(), "glib-compile-resources failed");
    println!("cargo:rerun-if-changed={manifest}");
    println!("cargo:rerun-if-changed={icons}");
}
