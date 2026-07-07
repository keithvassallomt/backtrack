// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! Backtrack core library.
//!
//! Houses the Borg adapter, the SQLite index, the offline spool, the restore
//! engine, and shared configuration. The daemon and clients depend on this
//! crate; it has no knowledge of D-Bus or GTK.
//!
//! This is the Stage 0 skeleton — real modules arrive in later stages.

pub mod logging;

/// The crate version, sourced from the workspace package version at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
