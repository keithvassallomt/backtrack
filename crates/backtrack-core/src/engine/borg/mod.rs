// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@icemalta.com>

//! The real [`BackupEngine`](super::BackupEngine): `BorgCli`. Spawns
//! `borg --log-json` and parses its stderr JSONL into [`JobEvent`](super::JobEvent)s.

mod classify;
mod logjson;
