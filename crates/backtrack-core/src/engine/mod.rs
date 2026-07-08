// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The Borg adapter: one trait, [`BackupEngine`], behind which every
//! Borg-specific operation lives, plus its typed error taxonomy and streamed
//! job events. Borg 2 later is a second implementation, not a rewrite.

mod error;

pub use error::{EngineError, HealthFailure, Result};
