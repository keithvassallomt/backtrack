#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>
#
# Remakes the compiled fixtures from the keyfiles and schema beside them, with
# the tools GNOME itself uses: dconf's databases (each folder under dconf/
# becomes <folder>.gvdb, its locks/ folder the database's locks) and the
# compiled schemas, with a distribution's override applied.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
for keyfiles in "$here"/dconf/*/; do
    name="$(basename "$keyfiles")"
    dconf compile "$here/dconf/$name.gvdb" "$keyfiles"
done
glib-compile-schemas "$here/schemas"
