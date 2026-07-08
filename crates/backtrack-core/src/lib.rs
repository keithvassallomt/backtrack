// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Backtrack core library.
//!
//! Houses the Borg adapter, the SQLite index, the offline spool, the restore
//! engine, and shared configuration. The daemon and clients depend on this
//! crate; it has no knowledge of D-Bus or GTK.
//!
//! The index (Stage 1) is the first real subsystem; the Borg adapter, spool,
//! and restore engine arrive in later stages.

pub mod engine;
pub mod index;
pub mod logging;
pub mod secret;

/// The crate version, sourced from the workspace package version at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
