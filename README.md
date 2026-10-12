# Flashcards

Flashcards is a GNOME application for creating and studying portable JSON decks.
It combines a native GTK4/libadwaita editor with direct access to the underlying
human-readable file.

## Features

- One versioned JSON file per deck
- Native card editing, tags, search, duplication, and ordering
- Offline LaTeX equations on both sides of study cards
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

The GUI regression test uses a temporary library and requires a display:

```sh
cargo test --bin flashcards gui_editing_and_study_flow -- --ignored --test-threads=1
# On a headless system with Xvfb and D-Bus installed:
xvfb-run -a dbus-run-session -- cargo test --bin flashcards gui_editing_and_study_flow -- --ignored --test-threads=1
```

Set `FLASHCARDS_SCREENSHOTS` to an existing directory to capture the test's screens.

The default library is `~/Documents/Flashcards`. It can be changed in
Preferences.

Write equations in the Front or Back field using `$x^2$` or `\(x^2\)` for
inline math, and `$$\frac{1}{2}$$` or `\[\frac{1}{2}\]` for display math.
They render when studying, alongside ordinary text. Invalid or unsupported
LaTeX stays visible as its original source. See
[equation examples](docs/deck-format.md#equations) for more details.

## Installation

Meson installs the release binary and GNOME metadata:

```sh
meson setup build
meson compile -C build
meson install -C build
```

The Flatpak development manifest can be installed locally:

```sh
flatpak-builder --user --install --force-clean flatpak-build \
  build-aux/page.polli.Flashcards.Devel.json
```

Build an offline release bundle and update repository with:

```sh
./build-aux/package-flatpak.sh
```

See [docs/distribution.md](docs/distribution.md) for direct-download and
self-hosted repository instructions.

See [docs/deck-format.md](docs/deck-format.md) and
[data/deck.schema.json](data/deck.schema.json) for the stable deck format.
