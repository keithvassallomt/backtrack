// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

// GNOME Shell answers its screenshot interface (org.gnome.Shell.Screenshot)
// and Eval only for callers on its own list, unless unsafe mode is on.
// scripts/screenshots needs both, on the development VM, and nowhere else.
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export default class UnsafeMode extends Extension {
    enable() {
        global.context.unsafe_mode = true;
    }

    disable() {
        global.context.unsafe_mode = false;
    }
}
