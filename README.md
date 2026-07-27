# Flashcards

Flashcards is a GNOME application for creating and studying portable JSON decks.
It combines a native GTK4/libadwaita editor with direct access to the underlying
human-readable file.

## Features

- One versioned JSON file per deck
- Native card editing, tags, search, duplication, and ordering
- Sequential or shuffled study sessions with Again/Good ratings
- Review history and summaries stored in the deck itself
- External text-editor integration through the desktop
- Explicit handling of malformed files and concurrent edits

## Development

The development packages for GTK 4 and libadwaita are required.

```sh
cargo run
cargo test
cargo clippy --all-targets -- -D warnings
```

The default library is `~/Documents/Flashcards`. It can be changed in
Preferences.

## Installation

Meson installs the release binary and GNOME metadata:

```sh
meson setup build
meson compile -C build
meson install -C build
```

The Flatpak manifest is intended for local development builds:

```sh
flatpak-builder --user --install --force-clean flatpak-build \
  build-aux/page.polli.Flashcards.Devel.json
```

See [docs/deck-format.md](docs/deck-format.md) and
[data/deck.schema.json](data/deck.schema.json) for the stable deck format.
