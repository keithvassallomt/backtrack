# Vendored Flatpak source generators

Unmodified copies from [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools)
at commit `74697c75b630d7330e77250fc13cb5ea688d9479` (MIT, as each file's
`__license__` says):

| File | Upstream path | Writes |
|---|---|---|
| `flatpak-cargo-generator.py` | `cargo/flatpak-cargo-generator.py` | `../cargo-sources.json`, every crate in `Cargo.lock` |
| `flatpak-pip-generator.py` | `pip/flatpak-pip-generator.py` | `../python3-requirements.json`, Borg and its Python packages |

`just flatpak-sources` runs both. Each script declares its own Python
dependencies inline, and `uv run` supplies them, so nothing is installed
system-wide.

To update: copy the two files from a newer commit, change the hash above, run
`just flatpak-sources`, and check that the Flatpak still builds offline.
